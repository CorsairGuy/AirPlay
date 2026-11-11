//! Discovery event stream -> validated catalog watch, without sender restarts.
use crate::discovery_catalog;
use mdns::discovery::{Browser, Kind, ResolvedService, ServiceEvent, ServiceTable};
use sender::outputs::OutputCatalog;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::{mpsc, watch};

pub enum Input {
    /// Browser construction is an explicit application action, never a default.
    Browser(Browser),
    Offline(mpsc::Receiver<ServiceEvent>),
}
impl Input {
    async fn recv(&mut self) -> Option<ServiceEvent> {
        match self {
            Self::Browser(browser) => browser.events().recv_async().await.ok(),
            Self::Offline(events) => events.recv().await,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    RejectedSnapshot,
    Stopped,
}

fn apply(
    table: &mut ServiceTable,
    event: ServiceEvent,
    local_ids: &[String],
    endpoint: &mut impl FnMut(&ResolvedService) -> Option<SocketAddr>,
    updates: &watch::Sender<Arc<OutputCatalog>>,
    status: &watch::Sender<Status>,
) {
    if !table.apply(event) {
        return;
    }
    match discovery_catalog::from_services(table, local_ids, endpoint) {
        Ok(next) => {
            updates.send_if_modified(|old| {
                if old.routes() == next.routes() {
                    false
                } else {
                    *old = Arc::new(next);
                    true
                }
            });
            status.send_if_modified(|old| {
                if *old == Status::Ready {
                    false
                } else {
                    *old = Status::Ready;
                    true
                }
            });
        }
        Err(_) => {
            // Keep the last valid catalog through conflicting/partial records.
            status.send_if_modified(|old| {
                if *old == Status::RejectedSnapshot {
                    false
                } else {
                    *old = Status::RejectedSnapshot;
                    true
                }
            });
        }
    }
}

/// Supply a fresh empty catalog watch. Invalid snapshots keep the last valid
/// value and report rejection separately, avoiding empty-list UI churn.
/// This task only discovers; it never authenticates or starts an audio session.
pub async fn run(
    mut input: Input,
    local_ids: Vec<String>,
    mut endpoint: impl FnMut(&ResolvedService) -> Option<SocketAddr> + Send,
    updates: watch::Sender<Arc<OutputCatalog>>,
    status: watch::Sender<Status>,
    mut stop: watch::Receiver<bool>,
) {
    let mut table = ServiceTable::new(Kind::AirPlay);
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            biased;
            changed=stop.changed() => {if changed.is_err() || *stop.borrow() {break;}},
            event=input.recv() => match event {
                Some(event)=>apply(&mut table,event,&local_ids,&mut endpoint,&updates,&status),
                None=>break,
            }
        }
    }
    status.send_replace(Status::Stopped);
    // Input owns Browser; dropping it stops browsing/shuts down its daemon.
    // Keep the last catalog available rather than deselect running outputs.
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(name: &str, uid: &str, port: u16) -> ServiceEvent {
        let info = mdns::discovery::ServiceInfo::new(
            mdns::AIRPLAY,
            name,
            "host.local.",
            "127.0.0.1",
            port,
            &[("psi", uid), ("features", "0x200")][..],
        )
        .unwrap();
        ServiceEvent::ServiceResolved(Box::new(info.as_resolved_service()))
    }
    #[test]
    fn duplicate_and_invalid_snapshots_do_not_reset_catalog() {
        let mut table = ServiceTable::new(Kind::AirPlay);
        let (updates, mut catalog) = watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
        let (status, state) = watch::channel(Status::Ready);
        let mut endpoint = |service: &ResolvedService| Some(([127, 0, 0, 1], service.port).into());
        apply(
            &mut table,
            event("speaker", "a", 7000),
            &[],
            &mut endpoint,
            &updates,
            &status,
        );
        assert_eq!(catalog.borrow_and_update().routes().len(), 1);
        apply(
            &mut table,
            event("speaker", "a", 7000),
            &[],
            &mut endpoint,
            &updates,
            &status,
        );
        assert!(!catalog.has_changed().unwrap());
        apply(
            &mut table,
            event("conflict", "a", 7001),
            &[],
            &mut endpoint,
            &updates,
            &status,
        );
        assert_eq!(*state.borrow(), Status::RejectedSnapshot);
        assert!(!catalog.has_changed().unwrap());
        assert_eq!(catalog.borrow().routes().len(), 1);
        apply(
            &mut table,
            ServiceEvent::ServiceRemoved(
                mdns::AIRPLAY.into(),
                "conflict._airplay._tcp.local.".into(),
            ),
            &[],
            &mut endpoint,
            &updates,
            &status,
        );
        assert_eq!(*state.borrow(), Status::Ready);
        assert!(!catalog.has_changed().unwrap());
        apply(
            &mut table,
            ServiceEvent::ServiceRemoved(
                mdns::AIRPLAY.into(),
                "speaker._airplay._tcp.local.".into(),
            ),
            &[],
            &mut endpoint,
            &updates,
            &status,
        );
        assert!(catalog.has_changed().unwrap());
        assert!(catalog.borrow().routes().is_empty());
    }
    #[tokio::test]
    async fn offline_event_stream_updates_watch_and_stops_without_clearing_it() {
        let (events, input) = mpsc::channel(4);
        let (updates, mut catalog) = watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
        let (status, state) = watch::channel(Status::Ready);
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(run(
            Input::Offline(input),
            vec![],
            |s| Some(([127, 0, 0, 1], s.port).into()),
            updates,
            status,
            stopped,
        ));
        events.send(event("speaker", "a", 7000)).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), catalog.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(catalog.borrow().routes().len(), 1);
        stop.send_replace(true);
        task.await.unwrap();
        assert_eq!(*state.borrow(), Status::Stopped);
        assert_eq!(catalog.borrow().routes().len(), 1);
    }
}
