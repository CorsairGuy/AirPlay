//! Source-session supervision, independent of device routes and shared clocks.
use anyhow::Result;
use sender::continuous_source::End;
use std::future::Future;
use tokio::sync::watch;
struct StopOnDrop(watch::Sender<bool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
/// Observe media, metadata/source lifetime and command forwarding together.
/// First completion stops this session, then awaits the other branches. The
/// caller must pass the same stop domain used to construct its media group.
/// This does not stop a shared clock daemon or mute/restore hardware routes.
/// Cancelling this future broadcasts stop; async cleanup still requires the
/// outer host to await the task rather than simply aborting it.
pub async fn run_source(
    media: impl Future<Output = Result<End>>,
    observations: impl Future<Output = Result<()>>,
    commands: impl Future<Output = Result<()>>,
    stop: watch::Sender<bool>,
) -> Result<End> {
    let stop = StopOnDrop(stop);
    if *stop.0.borrow() {
        return Ok(End::Stopped);
    }
    tokio::pin!(media, observations, commands);
    tokio::select! {
        biased;
        result = &mut media => {
            stop.0.send_replace(true);
            let (observed, commanded) = tokio::join!(observations, commands);
            let end = result?;
            observed?;
            commanded?;
            Ok(end)
        }
        result = &mut observations => {
            stop.0.send_replace(true);
            let (ended, commanded) = tokio::join!(media, commands);
            result?;
            ended?;
            commanded?;
            Ok(End::Stopped)
        }
        result = &mut commands => {
            stop.0.send_replace(true);
            let (ended, observed) = tokio::join!(media, observations);
            result?;
            ended?;
            observed?;
            Ok(End::Stopped)
        }
    }
}

/// Serialize complete source/output configurations through one session owner.
/// Identical notifications do not restart media. A completed session is not
/// retried until configuration changes (include a generation for explicit
/// retries). Factories must use the supplied per-session stop, keep shared clock
/// ownership outside it, and finish cooperative cleanup before returning.
pub async fn run_reconfigured<C, F, S>(
    mut desired: watch::Receiver<Option<C>>,
    mut shutdown: watch::Receiver<bool>,
    mut start: F,
) -> Result<()>
where
    C: Clone + PartialEq,
    F: FnMut(C, watch::Sender<bool>) -> S,
    S: Future<Output = Result<End>>,
{
    let mut completed = None;
    'reconcile: loop {
        if *shutdown.borrow() || shutdown.has_changed().is_err() {
            return Ok(());
        }
        let wanted = desired.borrow_and_update().clone();
        if wanted.is_none() {
            completed = None;
        }
        let selected = match wanted {
            Some(value) if completed.as_ref() != Some(&value) => value,
            _ => {
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { return Ok(()); } }
                    changed = desired.changed() => { if changed.is_err() { return Ok(()); } }
                }
                continue;
            }
        };
        if desired.has_changed().is_err() {
            return Ok(());
        }
        let (stop, _) = watch::channel(false);
        let stop = StopOnDrop(stop);
        let session = start(selected.clone(), stop.0.clone());
        tokio::pin!(session);
        loop {
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        stop.0.send_replace(true);
                        session.await?;
                        return Ok(());
                    }
                }
                changed = desired.changed() => {
                    if changed.is_err() {
                        stop.0.send_replace(true);
                        session.await?;
                        return Ok(());
                    }
                    let next = desired.borrow_and_update().clone();
                    if next.as_ref() != Some(&selected) {
                        stop.0.send_replace(true);
                        // Do not start a replacement if cleanup returned an error.
                        session.await?;
                        completed = None;
                        continue 'reconcile;
                    }
                }
                result = &mut session => {
                    stop.0.send_replace(true);
                    result?;
                    completed = Some(selected);
                    continue 'reconcile;
                }
            }
        }
    }
}
