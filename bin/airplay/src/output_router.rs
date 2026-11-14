//! Persistent Card queue routed to the current short-lived source session.
//! Intent is never confirmed topology, audible readiness or pairing approval.
use crate::{
    output_control::{OutputControl, OutputRequest},
    relay_selection,
};
use anyhow::{Context, Result};
use sender::outputs::OutputCatalog;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Intent {
    pub revision: u64,
    pub outputs: Vec<String>,
}
#[derive(Clone)]
pub struct Binding {
    pub revision: u64,
    /// None is an explicit retirement acknowledgement, published only after
    /// the host has awaited old-session cleanup. Publish a live queue before
    /// receiver preparation, so the original request carries its PIN context.
    pub control: Option<OutputControl>,
    token: Arc<()>,
}
/// Scoped publication; an old session cannot clear its successor's binding.
pub struct ActiveBinding {
    slot: watch::Sender<Option<Binding>>,
    token: Arc<()>,
}
impl ActiveBinding {
    pub fn publish(
        slot: watch::Sender<Option<Binding>>,
        revision: u64,
        control: OutputControl,
    ) -> Result<Self> {
        anyhow::ensure!(revision != 0, "invalid output binding revision");
        let token = Arc::new(());
        let mut accepted = false;
        slot.send_if_modified(|current| {
            if current
                .as_ref()
                .is_some_and(|binding| binding.revision > revision)
            {
                return false;
            }
            if current
                .as_ref()
                .is_some_and(|binding| binding.control.is_some())
            {
                return false;
            }
            *current = Some(Binding {
                revision,
                control: Some(control),
                token: token.clone(),
            });
            accepted = true;
            true
        });
        anyhow::ensure!(accepted, "previous output binding still active");
        Ok(Self { slot, token })
    }
}
impl Drop for ActiveBinding {
    fn drop(&mut self) {
        self.slot.send_if_modified(|current| {
            if current
                .as_ref()
                .is_some_and(|binding| Arc::ptr_eq(&binding.token, &self.token))
            {
                *current = None;
                true
            } else {
                false
            }
        });
    }
}
/// Call only from the session owner's idle/awaited-cleanup path. Loss of a
/// queue alone is not retirement proof. Never retire a nonempty new intent.
pub fn publish_retired(slot: &watch::Sender<Option<Binding>>, intent: &Intent) -> Result<()> {
    if intent.revision == 0 || !intent.outputs.is_empty() {
        return Ok(());
    }
    let mut accepted = true;
    slot.send_if_modified(|current| {
        if current
            .as_ref()
            .is_some_and(|binding| binding.revision > intent.revision)
        {
            return false;
        }
        if current
            .as_ref()
            .is_some_and(|binding| binding.control.is_some())
        {
            accepted = false;
            return false;
        }
        if current
            .as_ref()
            .is_some_and(|binding| binding.revision == intent.revision)
        {
            return false;
        }
        *current = Some(Binding {
            revision: intent.revision,
            control: None,
            token: Arc::new(()),
        });
        true
    });
    anyhow::ensure!(accepted, "cannot retire an active output binding");
    Ok(())
}

pub async fn run(
    mut requests: mpsc::Receiver<OutputRequest>,
    mut catalog: watch::Receiver<Arc<OutputCatalog>>,
    intent: watch::Sender<Intent>,
    mut binding: watch::Receiver<Option<Binding>>,
    local_ids: Vec<String>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    loop {
        if *stop.borrow() || stop.has_changed().is_err() {
            return Ok(());
        }
        let mut request = tokio::select! {
            biased;
            changed = stop.changed() => { if changed.is_err() || *stop.borrow() { return Ok(()); } continue; }
            request = requests.recv() => match request { Some(value) => value, None => return Ok(()) },
        };
        if request.reply.is_closed() {
            request.finish(100);
            continue;
        }
        let previous = intent.borrow().clone();
        let selection =
            request.volume.is_none() && request.batch.is_none() && request.operation.is_none();
        let revision = if selection {
            let next = match relay_selection::selection(
                &request.body,
                &catalog.borrow(),
                &local_ids,
                &previous.outputs,
            ) {
                Ok(next) => next,
                Err(_) => {
                    request.finish(39);
                    continue;
                }
            };
            if next != previous.outputs {
                let revision = previous
                    .revision
                    .checked_add(1)
                    .context("output intent revision exhausted")?;
                // Rollback only this still-current intent on a rejected request;
                // never overwrite a later controller's selection.
                let rollback = intent.clone();
                request.after_result(move |code| {
                    if code != 0 {
                        rollback.send_modify(|current| {
                            if current.revision == revision {
                                if let Some(next) = current.revision.checked_add(1) {
                                    current.revision = next;
                                    current.outputs = previous.outputs;
                                }
                            }
                        });
                    }
                });
                intent.send_replace(Intent {
                    revision,
                    outputs: next,
                });
                revision
            } else {
                previous.revision
            }
        } else {
            previous.revision
        };
        // Clearing all remotes is handled by the source owner, not a new
        // receiver session. Wait for its explicit completion binding as well.
        loop {
            if request.reply.is_closed() {
                request.finish(100);
                break;
            }
            if *stop.borrow() || stop.has_changed().is_err() {
                request.finish(100);
                return Ok(());
            }
            let active = binding.borrow_and_update().clone();
            if let Some(active) = active.filter(|active| active.revision == revision) {
                if let Some(control) = active.control {
                    if let Err(request) = control.forward(request) {
                        request.finish(100);
                    }
                } else {
                    let cleared = selection && intent.borrow().outputs.is_empty();
                    request.finish(if cleared { 0 } else { 100 });
                }
                break;
            }
            tokio::select! {
                biased;
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() { request.finish(100); return Ok(()); }
                }
                _ = request.reply.closed() => { request.finish(100); break; }
                changed = binding.changed() => {
                    if changed.is_err() { request.finish(100); return Ok(()); }
                }
                changed = catalog.changed() => {
                    if changed.is_err() { request.finish(100); return Ok(()); }
                }
            }
        }
    }
}
