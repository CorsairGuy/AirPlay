//! Application adapter that applies coordinator selection to prepared sessions.
//! No Station paths or capture policy.
use ::coordinator::{
    outputs::{self, ReceiverEndpoint, SelectionPlan},
    runtime::{ApplyResult, PhysicalSessionBackend, Runtime},
};
use ::discovery::catalog::OutputCatalog;
#[cfg(test)]
use ::media::tx::Session;
use ::media::tx::pool::SessionPool;
use anyhow::Result;
use remote::{
    output_control::{OutputRequest, VolumeBackend},
    output_selection,
};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::{mpsc, watch};

pub use crate::SessionPreparer;
pub use ::coordinator::runtime::Snapshot;
pub type AppliedSelection =
    ApplyResult<::media::tx::pool::SessionHandle, Result<::media::tx::StreamStats>>;

#[cfg(test)]
mod selection_safety {
    use super::*;
    use mrp::wire::Node;

    #[tokio::test]
    async fn invalid_request_does_not_modify_pool() {
        let (_source, audio) = tokio::sync::broadcast::channel(4);
        let pool = SessionPool::new(audio);
        let catalog = OutputCatalog::new(vec![::discovery::catalog::OutputRoute {
            uid: "row".into(),
            name: "speaker".into(),
            model: String::new(),
            aliases: vec!["group".into()],
            receivers: vec![::discovery::catalog::ReceiverEndpoint {
                uid: "physical".into(),
                address: ([127, 0, 0, 1], 7000).into(),
            }],
        }])
        .unwrap();
        let invalid = Node::default().text(5, "group").clone();
        let desired = output_selection::selection(&invalid, &catalog, &[], &[]).unwrap();
        let mut runtime = Runtime::default();
        let plan = plan_selection(&pool, &runtime, &catalog, &desired).unwrap();
        let mut pool = pool;
        assert!(
            apply_selection(&mut runtime, &mut pool, plan, vec![])
                .await
                .is_err()
        );
        assert!(pool.is_empty());
        let local = Node::default().text(4, "local").clone();
        let desired =
            output_selection::selection(&local, &catalog, &["local".into()], &[]).unwrap();
        let mut runtime = Runtime::default();
        let plan = plan_selection(&pool, &runtime, &catalog, &desired).unwrap();
        let result = apply_selection(&mut runtime, &mut pool, plan, vec![])
            .await
            .unwrap();
        assert!(result.requested.is_empty() && result.started.is_empty());
    }
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() || stop.changed().await.is_err() {
            return;
        }
    }
}
pub fn plan_selection(
    pool: &SessionPool,
    runtime: &Runtime,
    catalog: &OutputCatalog,
    desired: &[String],
) -> Result<SelectionPlan> {
    let members = pool.members();
    let current = members
        .iter()
        .filter(|member| member.running)
        .map(|member| ReceiverEndpoint {
            uid: member.uid.clone(),
            address: member.address,
        })
        .collect::<Vec<_>>();
    let desired_members = catalog.receivers(desired)?;
    let mut plan = runtime.plan(catalog, &current, desired)?;
    for member in members.into_iter().filter(|member| !member.running) {
        let endpoint = ReceiverEndpoint {
            uid: member.uid,
            address: member.address,
        };
        if desired_members.iter().any(|desired| desired == &endpoint)
            && !plan.adding.contains(&endpoint)
        {
            plan.adding.push(endpoint.clone());
        }
        plan.removing.push(endpoint);
    }
    Ok(plan)
}

fn confirmed(
    pool: &SessionPool,
    runtime: &Runtime,
    catalog: &OutputCatalog,
    desired: &[String],
) -> Vec<String> {
    desired
        .iter()
        .filter(|uid| {
            plan_selection(pool, runtime, catalog, &[(*uid).clone()])
                .is_ok_and(|plan| plan.adding.is_empty())
        })
        .cloned()
        .collect()
}

fn apply_route_volumes(
    pool: &SessionPool,
    catalog: &OutputCatalog,
    levels: &[(String, ::media::tx::Volume)],
) -> Result<()> {
    let expanded = outputs::expand_route_values(catalog, levels)?;
    let members = pool
        .members()
        .into_iter()
        .map(|member| (member.uid.clone(), member))
        .collect::<BTreeMap<_, _>>();
    let mut physical = Vec::new();
    for (receiver, gain) in expanded {
        let member = members
            .get(&receiver.uid)
            .ok_or_else(|| anyhow::anyhow!("inactive output member"))?;
        if !member.running || member.address != receiver.address {
            anyhow::bail!("output member is inactive or changed endpoint");
        }
        physical.push((receiver.uid, gain));
    }
    pool.set_member_volumes(&physical)
}

struct RuntimeVolumeBackend<'a> {
    pool: &'a SessionPool,
    catalog: &'a OutputCatalog,
}
impl VolumeBackend for RuntimeVolumeBackend<'_> {
    fn set_routes(
        &self,
        catalog: &OutputCatalog,
        levels: &[(String, ::media::tx::Volume)],
    ) -> Result<()> {
        anyhow::ensure!(
            std::ptr::eq(catalog, self.catalog),
            "catalog changed during operation"
        );
        apply_route_volumes(self.pool, catalog, levels)
    }
}

struct PoolBackend<'a>(&'a mut SessionPool);
impl PhysicalSessionBackend<::media::tx::Session> for PoolBackend<'_> {
    type Handle = ::media::tx::pool::SessionHandle;
    type Removal = Result<::media::tx::StreamStats>;

    fn endpoint(&self, uid: &str, prepared: &::media::tx::Session) -> ReceiverEndpoint {
        ReceiverEndpoint {
            uid: uid.to_owned(),
            address: prepared.endpoint(),
        }
    }

    fn remove<'a>(
        &'a mut self,
        uid: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Self::Removal> + Send + 'a>> {
        Box::pin(async move { self.0.remove(uid).await })
    }

    fn add(&mut self, uid: &str, prepared: ::media::tx::Session) -> Result<Self::Handle> {
        self.0.add(uid, prepared)
    }
}

pub async fn apply_selection(
    runtime: &mut Runtime,
    pool: &mut SessionPool,
    plan: SelectionPlan,
    prepared: Vec<(String, ::media::tx::Session)>,
) -> Result<AppliedSelection> {
    runtime
        .apply_plan(plan, prepared, &mut PoolBackend(pool))
        .await
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
/// awaits pool cleanup. Interactive PIN requires the per-request authorization
/// context supplied by the MRP reader and an explicitly interactive preparer.
/// Never retain/share one controller's context across unrelated requests.
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
    let mut desired = Runtime::default();
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
                match desired.preserve_selected(discovered.as_ref(), catalog.as_ref()) {
                    Ok(next) => catalog=Arc::new(next),
                    Err(_) => last_error=Some(39),
                }
                continue;
            }
            request = requests.recv() => match request { Some(request) => request, None => break },
            _ = tick.tick() => {
                let result = tokio::select! {
                    biased;
                    _ = stopped(&mut stop) => break,
                    result = preparer.active(&pool) => result,
                };
                if result.is_err() { last_error=Some(100); break; }
                publish(
                    &updates,
                    Snapshot {
                        selected: confirmed(&pool, &desired, &catalog, desired.selected()),
                        last_error,
                    },
                );
                continue;
            }
        };
        if request.reply.is_closed() {
            request.finish(100);
            continue;
        }
        if let Some(operation) = request.operation.take() {
            let backend = RuntimeVolumeBackend {
                pool: &pool,
                catalog: &catalog,
            };
            let code = operation(&backend, &catalog);
            request.finish(code);
            continue;
        }
        if let Some(levels) = request.batch.take() {
            let gains: Result<Vec<_>> = levels
                .into_iter()
                .map(|(uid, value)| ::media::tx::Volume::normalized(value).map(|gain| (uid, gain)))
                .collect();
            let code = match gains.and_then(|gains| apply_route_volumes(&pool, &catalog, &gains)) {
                Ok(()) => 0,
                Err(_) => 39,
            };
            request.finish(code);
            continue;
        }
        if let Some((uid, value)) = request.volume.take() {
            let code = match ::media::tx::Volume::normalized(value)
                .and_then(|gain| apply_route_volumes(&pool, &catalog, &[(uid, gain)]))
            {
                Ok(()) => 0,
                Err(_) => 39,
            };
            request.finish(code);
            continue;
        }
        let selection_request = match output_selection::decode(&request.body) {
            Ok(selection) => selection,
            Err(_) => {
                request.finish(39);
                last_error = Some(39);
                continue;
            }
        };
        let current = confirmed(&pool, &desired, &catalog, desired.selected());
        let next = match desired.resolve(&selection_request, catalog.as_ref(), &local_ids, &current)
        {
            Ok(next) => next,
            Err(_) => {
                request.finish(39);
                last_error = Some(39);
                continue;
            }
        };
        let plan = match plan_selection(&pool, &desired, &catalog, &next) {
            Ok(plan) => plan,
            Err(_) => {
                request.finish(39);
                last_error = Some(39);
                continue;
            }
        };
        let selection_ready = catalog
            .receivers(&next)
            .and_then(|receivers| preparer.selection(&receivers));
        if selection_ready.is_err() {
            request.finish(100);
            last_error = Some(100);
            continue;
        }
        let prepare = async {
            let mut prepared = Vec::new();
            for receiver in plan.adding.iter().cloned() {
                let uid = receiver.uid.clone();
                prepared.push((
                    uid,
                    preparer
                        .prepare_authorized(receiver, request.authorization.as_ref())
                        .await?,
                ));
            }
            Ok::<_, anyhow::Error>(prepared)
        };
        let prepared = tokio::select! {
            biased;
            _ = stopped(&mut stop) => break,
            _ = request.reply.closed() => None,
            result = prepare => Some(result),
        };
        let Some(prepared) = prepared else {
            request.finish(100);
            continue;
        };
        // Authentication may have taken seconds. Do not start a receiver at a
        // stale/reassigned address or apply an alias that now means another row.
        if discovery.has_changed().unwrap_or(true) {
            // A closed watch can still contain a final, unseen snapshot.
            discovered = discovery.borrow_and_update().clone();
            let refreshed = desired.preserve_selected(discovered.as_ref(), catalog.as_ref());
            let still_matches = refreshed.as_ref().is_ok_and(|fresh| {
                desired
                    .resolve(&selection_request, fresh, &local_ids, &current)
                    .is_ok_and(|value| value == next)
                    && fresh.receivers(&next).ok() == catalog.receivers(&next).ok()
            });
            if let Ok(fresh) = refreshed {
                catalog = Arc::new(fresh);
            }
            if !still_matches {
                request.finish(100);
                last_error = Some(100);
                continue;
            }
        }
        let code = match prepared {
            Err(_) => 100,
            Ok(prepared) => match apply_selection(&mut desired, &mut pool, plan, prepared).await {
                Ok(result) => {
                    // Release deselected pins immediately, even if discovery
                    // sends no further update before the next selection.
                    if let Ok(fresh) =
                        desired.preserve_selected(discovered.as_ref(), catalog.as_ref())
                    {
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
        let reconciled = tokio::select! {
            biased;
            _ = stopped(&mut stop) => break,
            result = preparer.active(&pool) => result,
        };
        if reconciled.is_err() {
            request.finish(100);
            last_error = Some(100);
            break;
        }
        last_error = (code != 0).then_some(code);
        publish(
            &updates,
            Snapshot {
                selected: confirmed(&pool, &desired, &catalog, desired.selected()),
                last_error,
            },
        );
        request.finish(code);
    }
    pool.shutdown().await;
    if preparer.finish().await.is_err() {
        last_error = Some(100);
    }
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
    use ::discovery::catalog::OutputRoute;
    struct FailedReconciliation;
    impl SessionPreparer for FailedReconciliation {
        async fn active(&mut self, _: &SessionPool) -> Result<()> {
            anyhow::bail!("offline reconciliation failure")
        }
        async fn prepare(&mut self, _: ReceiverEndpoint) -> Result<Session> {
            panic!("no authentication expected")
        }
    }
    #[tokio::test]
    async fn reconciliation_failure_ends_worker_and_publishes_empty_error_state() {
        let (_source, audio) = tokio::sync::broadcast::channel(4);
        let (_control, requests) = remote::output_control::OutputControl::channel(4);
        let (updates, state) = watch::channel(Snapshot::default());
        let (_stop, stopped) = watch::channel(false);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            run(
                SessionPool::new(audio),
                requests,
                catalog(),
                vec![],
                FailedReconciliation,
                updates,
                stopped,
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            *state.borrow(),
            Snapshot {
                selected: vec![],
                last_error: Some(100)
            }
        );
    }
    struct RejectSelection(Arc<std::sync::atomic::AtomicBool>);
    impl SessionPreparer for RejectSelection {
        fn selection(&mut self, receivers: &[ReceiverEndpoint]) -> Result<()> {
            assert_eq!(receivers.len(), 1);
            anyhow::bail!("invalid offline selection")
        }
        async fn prepare(&mut self, _: ReceiverEndpoint) -> Result<Session> {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            anyhow::bail!("authentication must not run")
        }
    }
    #[tokio::test]
    async fn selection_configuration_is_rejected_before_authentication() {
        let (_source, audio) = tokio::sync::broadcast::channel(4);
        let (control, requests) = remote::output_control::OutputControl::channel(4);
        let (updates, _) = watch::channel(Snapshot::default());
        let (stop, stopped) = watch::channel(false);
        let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = tokio::spawn(run(
            SessionPool::new(audio),
            requests,
            catalog(),
            vec![],
            RejectSelection(entered.clone()),
            updates,
            stopped,
        ));
        let reply = control
            .submit(mrp::wire::Node::default().text(2, "row").clone())
            .unwrap();
        assert_eq!(reply.await.unwrap(), 100);
        assert!(!entered.load(std::sync::atomic::Ordering::SeqCst));
        stop.send_replace(true);
        task.await.unwrap();
    }
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
        let (control, requests) = remote::output_control::OutputControl::channel(4);
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
        let (control, requests) = remote::output_control::OutputControl::channel(4);
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
        let (control, requests) = remote::output_control::OutputControl::channel(4);
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
