//! Serialized owner of outgoing selection. No Station paths or capture policy.
use crate::{output_control::OutputRequest, relay_selection};
use anyhow::Result;
use sender::{
    Session,
    outputs::{OutputCatalog, ReceiverEndpoint},
    pool::SessionPool,
};
use std::{future::Future, sync::Arc};
use tokio::sync::{mpsc, watch};

/// Application supplies authentication/configuration. Dropping this future
/// must release pending pairing resources; do not detach network work here.
pub trait SessionPreparer: Send {
    fn prepare(
        &mut self,
        receiver: ReceiverEndpoint,
    ) -> impl Future<Output = Result<Session>> + Send;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Logical rows whose required physical workers are currently running.
    /// Running is task state, not proof of audible output/group adoption.
    pub selected: Vec<String>,
    pub last_error: Option<u64>,
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() || stop.changed().await.is_err() {
            return;
        }
    }
}
fn confirmed(pool: &SessionPool, catalog: &OutputCatalog, desired: &[String]) -> Vec<String> {
    desired
        .iter()
        .filter(|uid| {
            pool.plan_selection(catalog, &[(*uid).clone()])
                .is_ok_and(|plan| plan.adding.is_empty())
        })
        .cloned()
        .collect()
}
fn publish(updates: &watch::Sender<Snapshot>, value: Snapshot) {
    updates.send_if_modified(|previous| {
        if *previous == value {
            false
        } else {
            *previous = value;
            true
        }
    });
}

/// Own one pool/queue pair with a static catalog. For live discovery use
/// run_discovered; the PCM producer remains owned by the application.
///
/// Preparation is cancellable on controller loss/stop; applying a validated
/// change finishes bounded teardown even if the controller leaves. Shutdown
/// awaits pool cleanup. Use preauthorized/transient session preparation here:
/// interactive PIN still requires per-request controller authorization context,
/// which this shared preparer interface does not yet carry. Never share one
/// controller's PIN channels across requests from different controllers.
pub async fn run(
    pool: SessionPool,
    requests: mpsc::Receiver<OutputRequest>,
    catalog: Arc<OutputCatalog>,
    local_ids: Vec<String>,
    preparer: impl SessionPreparer,
    updates: watch::Sender<Snapshot>,
    stop: watch::Receiver<bool>,
) {
    let (_catalog_owner, discovery) = watch::channel(catalog);
    run_discovered(
        pool, requests, discovery, local_ids, preparer, updates, stop,
    )
    .await;
}

/// Observe catalog updates without restarting active sessions. Selected rows
/// stay pinned; inactive rows use fresh discovery. Revalidate pending additions
/// against the newest catalog before any pool mutation. No automatic reconnect.
pub async fn run_discovered(
    mut pool: SessionPool,
    mut requests: mpsc::Receiver<OutputRequest>,
    mut discovery: watch::Receiver<Arc<OutputCatalog>>,
    local_ids: Vec<String>,
    mut preparer: impl SessionPreparer,
    updates: watch::Sender<Snapshot>,
    mut stop: watch::Receiver<bool>,
) {
    let mut discovered = discovery.borrow_and_update().clone();
    let mut catalog = discovered.clone();
    let mut discovery_open = true;
    let mut desired = Vec::new();
    let mut last_error = None;
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    publish(&updates, Snapshot::default());
    loop {
        let mut request = tokio::select! {
            biased;
            _ = stopped(&mut stop) => break,
            changed = discovery.changed(), if discovery_open => {
                if changed.is_err() { discovery_open=false; continue; }
                discovered = discovery.borrow_and_update().clone();
                match discovered.preserving_selected(&catalog,&desired) {
                    Ok(next) => catalog=Arc::new(next),
                    Err(_) => last_error=Some(39),
                }
                continue;
            }
            request = requests.recv() => match request { Some(request) => request, None => break },
            _ = tick.tick() => {
                publish(&updates,Snapshot {selected: confirmed(&pool,&catalog,&desired),last_error});
                continue;
            }
        };
        if request.reply.is_closed() {
            continue;
        }
        let current = confirmed(&pool, &catalog, &desired);
        let selection = relay_selection::selection(&request.body, &catalog, &local_ids, &current);
        let next = match selection {
            Ok(next) => next,
            Err(_) => {
                let _ = request.reply.send(39);
                last_error = Some(39);
                continue;
            }
        };
        let plan = match pool.plan_selection(&catalog, &next) {
            Ok(plan) => plan,
            Err(_) => {
                let _ = request.reply.send(39);
                last_error = Some(39);
                continue;
            }
        };
        let prepare = async {
            let mut prepared = Vec::new();
            for receiver in plan.adding {
                let uid = receiver.uid.clone();
                prepared.push((uid, preparer.prepare(receiver).await?));
            }
            Ok::<_, anyhow::Error>(prepared)
        };
        let prepared = tokio::select! {
            biased;
            _ = stopped(&mut stop) => break,
            _ = request.reply.closed() => continue,
            result = prepare => result,
        };
        // Authentication may have taken seconds. Do not start a receiver at a
        // stale/reassigned address or apply an alias that now means another row.
        if discovery.has_changed().unwrap_or(true) {
            // A closed watch can still contain a final, unseen snapshot.
            discovered = discovery.borrow_and_update().clone();
            let refreshed = discovered.preserving_selected(&catalog, &desired);
            let still_matches = refreshed.as_ref().is_ok_and(|fresh| {
                relay_selection::selection(&request.body, fresh, &local_ids, &current)
                    .is_ok_and(|value| value == next)
                    && fresh.receivers(&next).ok() == catalog.receivers(&next).ok()
            });
            if let Ok(fresh) = refreshed {
                catalog = Arc::new(fresh);
            }
            if !still_matches {
                let _ = request.reply.send(100);
                last_error = Some(100);
                continue;
            }
        }
        let code = match prepared {
            Err(_) => 100,
            Ok(prepared) => match pool.apply_selection(&catalog, &next, prepared).await {
                Ok(result) => {
                    desired = next;
                    // Release deselected pins immediately, even if discovery
                    // sends no further update before the next selection.
                    if let Ok(fresh) = discovered.preserving_selected(&catalog, &desired) {
                        catalog = Arc::new(fresh);
                    }
                    if result.start_errors.is_empty() {
                        0
                    } else {
                        100
                    }
                }
                Err(_) => 100,
            },
        };
        last_error = (code != 0).then_some(code);
        publish(
            &updates,
            Snapshot {
                selected: confirmed(&pool, &catalog, &desired),
                last_error,
            },
        );
        let _ = request.reply.send(code);
    }
    pool.shutdown().await;
    publish(
        &updates,
        Snapshot {
            selected: Vec::new(),
            last_error,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use sender::outputs::OutputRoute;
    struct RecordFailure(Arc<std::sync::Mutex<Vec<std::net::SocketAddr>>>);
    impl SessionPreparer for RecordFailure {
        async fn prepare(&mut self, receiver: ReceiverEndpoint) -> Result<Session> {
            self.0.lock().unwrap().push(receiver.address);
            anyhow::bail!("offline test: no network connection permitted")
        }
    }
    #[tokio::test]
    async fn live_discovery_changes_are_used_without_restarting_worker() {
        let (_source, audio) = tokio::sync::broadcast::channel(4);
        let (control, requests) = crate::output_control::OutputControl::channel(4);
        let (updates, _) = watch::channel(Snapshot::default());
        let (stop, stopped) = watch::channel(false);
        let (discovery, catalogs) = watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
        let recorded = Arc::new(std::sync::Mutex::new(Vec::new()));
        let task = tokio::spawn(run_discovered(
            SessionPool::new(audio),
            requests,
            catalogs,
            vec![],
            RecordFailure(recorded.clone()),
            updates,
            stopped,
        ));
        let select = || mrp::wire::Node::default().text(2, "row").clone();
        assert_eq!(control.submit(select()).unwrap().await.unwrap(), 39);
        discovery.send_replace(catalog());
        assert_eq!(control.submit(select()).unwrap().await.unwrap(), 100);
        assert_eq!(recorded.lock().unwrap().len(), 1);
        // A changed inactive endpoint must be used by the same worker.
        let next = OutputCatalog::new(vec![OutputRoute {
            uid: "row".into(),
            name: "speaker".into(),
            model: "".into(),
            aliases: vec![],
            receivers: vec![ReceiverEndpoint {
                uid: "peer".into(),
                address: ([127, 0, 0, 1], 7001).into(),
            }],
        }])
        .unwrap();
        discovery.send_replace(Arc::new(next));
        drop(discovery);
        assert_eq!(control.submit(select()).unwrap().await.unwrap(), 100);
        assert_eq!(recorded.lock().unwrap().last().unwrap().port(), 7001);
        stop.send_replace(true);
        task.await.unwrap();
    }
    struct Blocked(Arc<std::sync::atomic::AtomicUsize>);
    struct Guard(Arc<std::sync::atomic::AtomicUsize>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    impl SessionPreparer for Blocked {
        async fn prepare(&mut self, _: ReceiverEndpoint) -> Result<Session> {
            let _guard = Guard(self.0.clone());
            std::future::pending().await
        }
    }
    fn catalog() -> Arc<OutputCatalog> {
        Arc::new(
            OutputCatalog::new(vec![OutputRoute {
                uid: "row".into(),
                name: "speaker".into(),
                model: "".into(),
                aliases: vec![],
                receivers: vec![ReceiverEndpoint {
                    uid: "peer".into(),
                    address: ([127, 0, 0, 1], 7000).into(),
                }],
            }])
            .unwrap(),
        )
    }
    #[tokio::test]
    async fn controller_disconnect_cancels_prepare_and_next_request_still_works() {
        let (_source, audio) = tokio::sync::broadcast::channel(4);
        let (control, requests) = crate::output_control::OutputControl::channel(4);
        let (updates, mut state) = watch::channel(Snapshot::default());
        let (stop, stopped) = watch::channel(false);
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task = tokio::spawn(run(
            SessionPool::new(audio),
            requests,
            catalog(),
            vec!["local".into()],
            Blocked(dropped.clone()),
            updates,
            stopped,
        ));
        let reply = control
            .submit(mrp::wire::Node::default().text(2, "row").clone())
            .unwrap();
        tokio::task::yield_now().await;
        drop(reply);
        let reply = control
            .submit(mrp::wire::Node::default().text(4, "local").clone())
            .unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), reply)
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(state.borrow_and_update().selected.is_empty());
        stop.send_replace(true);
        task.await.unwrap();
    }
    #[tokio::test]
    async fn stop_cancels_pending_prepare_and_releases_reply() {
        let (_source, audio) = tokio::sync::broadcast::channel(4);
        let (control, requests) = crate::output_control::OutputControl::channel(4);
        let (updates, _) = watch::channel(Snapshot::default());
        let (stop, stopped) = watch::channel(false);
        let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task = tokio::spawn(run(
            SessionPool::new(audio),
            requests,
            catalog(),
            vec![],
            Blocked(dropped.clone()),
            updates,
            stopped,
        ));
        let reply = control
            .submit(mrp::wire::Node::default().text(2, "row").clone())
            .unwrap();
        tokio::task::yield_now().await;
        stop.send_replace(true);
        task.await.unwrap();
        assert!(reply.await.is_err());
        assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
