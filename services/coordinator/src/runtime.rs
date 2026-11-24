//! Protocol- and transport-neutral output runtime policy.
//!
//! The application owns the async request loop, discovery feed, authentication
//! and concrete session pool. This runtime owns the desired-selection state and
//! resolves catalog requests into physical session plans through interfaces.
use crate::{
    outputs::{self, OutputSnapshot, ReceiverEndpoint, SelectionPlan},
    selection::{self, Request, State},
};
use anyhow::Result;
use std::collections::BTreeMap;
use std::{future::Future, pin::Pin};

/// Host adapter for physical session operations. Prepared session and handle
/// types stay with the application/media crate; coordinator only sees endpoints.
pub trait PhysicalSessionBackend<P>: Send {
    type Handle: Send;
    type Removal: Send;

    fn endpoint(&self, uid: &str, prepared: &P) -> ReceiverEndpoint;
    fn remove<'a>(
        &'a mut self,
        uid: &'a str,
    ) -> Pin<Box<dyn Future<Output = Self::Removal> + Send + 'a>>;
    fn add(&mut self, uid: &str, prepared: P) -> Result<Self::Handle>;
}

pub struct ApplyResult<H, R> {
    pub requested: Vec<String>,
    pub started: Vec<(String, H)>,
    pub removed: Vec<(String, R)>,
    pub start_errors: Vec<(String, anyhow::Error)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Logical rows whose required physical workers are running.
    pub selected: Vec<String>,
    pub last_error: Option<u64>,
}

#[derive(Default)]
pub struct Runtime {
    selection: State,
}

impl Runtime {
    pub fn selected(&self) -> &[String] {
        self.selection.selected()
    }

    pub fn resolve<C: OutputSnapshot>(
        &self,
        request: &Request,
        catalog: &C,
        local_ids: &[String],
        current: &[String],
    ) -> Result<Vec<String>> {
        selection::resolve(request, catalog, local_ids, current)
    }

    pub fn plan<C: OutputSnapshot>(
        &self,
        catalog: &C,
        current: &[ReceiverEndpoint],
        selected: &[String],
    ) -> Result<SelectionPlan> {
        outputs::plan(catalog, current, selected)
    }

    pub fn preserve_selected<C: OutputSnapshot>(&self, fresh: &C, previous: &C) -> Result<C> {
        let mut seen = std::collections::BTreeSet::new();
        let mut pinned = Vec::new();
        for id in self.selected() {
            let uid = previous.resolve_uid(id)?;
            if seen.insert(uid.clone()) {
                pinned.push(previous.route(&uid)?);
            }
        }
        fresh.merge_pinned(pinned)
    }

    pub fn commit(&mut self, selected: Vec<String>) {
        self.selection.commit(selected);
    }

    /// Validate prepared additions and apply a physical plan through the host's
    /// session interface. Existing workers are removed before replacements.
    pub async fn apply_plan<P, B>(
        &mut self,
        plan: SelectionPlan,
        prepared: Vec<(String, P)>,
        backend: &mut B,
    ) -> Result<ApplyResult<B::Handle, B::Removal>>
    where
        B: PhysicalSessionBackend<P>,
        P: Send,
    {
        if prepared.len() != plan.adding.len() {
            anyhow::bail!("prepared sessions do not match selection additions");
        }
        let mut sessions = BTreeMap::new();
        for (uid, session) in prepared {
            let endpoint = backend.endpoint(&uid, &session);
            if !plan
                .adding
                .iter()
                .any(|receiver| receiver.uid == uid && receiver == &endpoint)
                || sessions.insert(uid, session).is_some()
            {
                anyhow::bail!("duplicate or mismatched prepared session");
            }
        }

        let mut result = ApplyResult {
            requested: plan.selected.clone(),
            started: Vec::new(),
            removed: Vec::new(),
            start_errors: Vec::new(),
        };
        for receiver in plan.removing {
            result
                .removed
                .push((receiver.uid.clone(), backend.remove(&receiver.uid).await));
        }
        for (uid, session) in sessions {
            match backend.add(&uid, session) {
                Ok(handle) => result.started.push((uid, handle)),
                Err(error) => result.start_errors.push((uid, error)),
            }
        }
        self.commit(plan.selected);
        Ok(result)
    }
}
