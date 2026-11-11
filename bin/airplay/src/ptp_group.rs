//! Explicit outgoing local-master lifecycle; no clock/discovery/capture startup.
use crate::{
    output_control::OutputRequest,
    output_worker::{self, SessionPreparer},
};
use anyhow::{Result, bail};
use sender::{
    AudioPacket, MediaClock, Session,
    outputs::{OutputCatalog, ReceiverEndpoint},
    pool::SessionPool,
};
use std::sync::Arc;
use timesync::media::{MasterLease, MasterTimeline};
use tokio::sync::{mpsc, watch};

/// Own this alongside the clock service. A new epoch requires a new group.
/// Dropping it cancels the worker, producer and any local renderer subscribed
/// to `stop()`. Graceful shutdown must await `run`, not abort its future.
pub struct PtpGroup {
    timeline: Arc<MasterTimeline>,
    group: u128,
    lease: MasterLease,
}
impl PtpGroup {
    pub fn new(
        timeline: Arc<MasterTimeline>,
        group: u128,
        upstream: watch::Receiver<bool>,
    ) -> Result<Self> {
        if group == 0 {
            bail!("empty outgoing group identity");
        }
        timeline.reading_at(std::time::Instant::now())?;
        let lease = MasterLease::spawn(timeline.clone(), upstream);
        Ok(Self {
            timeline,
            group,
            lease,
        })
    }
    /// Use this same cancellation receiver for the local renderer. The clock
    /// service is externally owned and is not stopped by removing one group.
    pub fn stop(&self) -> watch::Receiver<bool> {
        self.lease.subscribe()
    }
    /// One application-paced producer broadcasts each returned immutable packet
    /// to local/remote consumers. No receiver samples its own clock here.
    pub fn producer(&self, clock: MediaClock) -> PtpProducer {
        PtpProducer {
            timeline: self.timeline.clone(),
            clock,
            stop: self.stop(),
        }
    }
    /// Consumes the lifecycle owner, so queue closure/worker completion also
    /// cancels producer and renderer. Worker awaits bounded pool cleanup.
    pub async fn run(
        self,
        pool: SessionPool,
        requests: mpsc::Receiver<OutputRequest>,
        discovery: watch::Receiver<Arc<OutputCatalog>>,
        local_ids: Vec<String>,
        preparer: impl SessionPreparer,
        updates: watch::Sender<output_worker::Snapshot>,
    ) -> Result<()> {
        self.run_worker(pool, requests, discovery, local_ids, preparer, updates)
            .await
    }
    /// Supervise the existing renderer and selection worker in one task.
    /// Local EOF drains before stopping the group; local failure cancels it
    /// immediately. Both futures are awaited, including pool teardown. The
    /// platform adapter alone restores direct routing after this returns.
    pub async fn run_with_local(
        self,
        pool: SessionPool,
        requests: mpsc::Receiver<OutputRequest>,
        discovery: watch::Receiver<Arc<OutputCatalog>>,
        local_ids: Vec<String>,
        preparer: impl SessionPreparer,
        updates: watch::Sender<output_worker::Snapshot>,
        sink: impl sender::LocalSink,
        packets: tokio::sync::broadcast::Receiver<Arc<AudioPacket>>,
        latency: std::time::Duration,
    ) -> Result<()> {
        let worker = self.run_worker(pool, requests, discovery, local_ids, preparer, updates);
        let local = sender::run_local(sink, packets, latency, self.stop());
        tokio::pin!(worker, local);
        tokio::select! {
            result = &mut worker => {
                self.lease.cancel();
                let local_result = local.await;
                result.and(local_result)
            },
            result = &mut local => {
                self.lease.cancel();
                let worker_result = worker.await;
                result.and(worker_result)
            },
        }
    }
    async fn run_worker(
        &self,
        mut pool: SessionPool,
        requests: mpsc::Receiver<OutputRequest>,
        discovery: watch::Receiver<Arc<OutputCatalog>>,
        local_ids: Vec<String>,
        preparer: impl SessionPreparer,
        updates: watch::Sender<output_worker::Snapshot>,
    ) -> Result<()> {
        // Existing sessions may belong to another epoch or use NTP. Only this
        // owner's qualified preparer may admit members to the new pool.
        if !pool.is_empty() {
            pool.shutdown().await;
            bail!("new PTP group requires an empty session pool");
        }
        let preparer = QualifiedPreparer {
            inner: preparer,
            timeline: self.timeline.clone(),
            group: self.group,
        };
        output_worker::run_discovered(
            pool,
            requests,
            discovery,
            local_ids,
            preparer,
            updates,
            self.stop(),
        )
        .await;
        // self's lease drops only after pool teardown has completed.
        Ok(())
    }
}

pub struct PtpProducer {
    timeline: Arc<MasterTimeline>,
    clock: MediaClock,
    stop: watch::Receiver<bool>,
}
impl PtpProducer {
    pub fn packet(&self, frame_index: u64, pcm: &[u8]) -> Result<AudioPacket> {
        if *self.stop.borrow() || self.stop.has_changed().is_err() {
            bail!("outgoing group stopped");
        }
        self.clock
            .packet_with_ptp(frame_index, pcm, self.timeline.domain(), |instant| {
                Ok(self.timeline.reading_at(instant)?)
            })
    }
}

struct QualifiedPreparer<P> {
    inner: P,
    timeline: Arc<MasterTimeline>,
    group: u128,
}
impl<P: SessionPreparer> SessionPreparer for QualifiedPreparer<P> {
    fn selection(&mut self, receivers: &[ReceiverEndpoint]) -> Result<()> {
        self.timeline.reading_at(std::time::Instant::now())?;
        self.inner.selection(receivers)
    }
    async fn prepare(&mut self, receiver: ReceiverEndpoint) -> Result<Session> {
        self.timeline.reading_at(std::time::Instant::now())?;
        let session = self.inner.prepare(receiver).await?;
        self.timeline.reading_at(std::time::Instant::now())?;
        let control = session
            .ptp_peer_control()
            .ok_or_else(|| anyhow::anyhow!("NTP session in PTP group"))?;
        let identity = control.requested();
        if identity.domain() != self.timeline.domain() || identity.group() != self.group {
            bail!("prepared session has another PTP epoch/group");
        }
        Ok(session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clock::{
        domain::{ClockMap, Domain, LocalClock},
        role::{Role, Selection},
    };
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::{Duration, Instant},
    };
    use timesync::{
        engine::{Quality, Snapshot},
        media::InstantSource,
    };
    fn fixture() -> (
        watch::Sender<Snapshot>,
        watch::Sender<bool>,
        PtpGroup,
        Instant,
    ) {
        let origin = Instant::now();
        let domain = Domain::new(7, 1).unwrap();
        let local = LocalClock::new(1).unwrap();
        let source = Arc::new(InstantSource::new(origin, local, domain, 1_000_000_000));
        let (updates, status) = watch::channel(Snapshot {
            selection: Selection {
                role: Role::Master,
                domain: Some(domain),
            },
            mapping: Some(
                ClockMap::new(local.at(0), domain.reading(1_000_000_000), 0, 5_000_000_000)
                    .unwrap(),
            ),
            quality: Some(Quality::Local),
            link: None,
        });
        let timeline =
            Arc::new(MasterTimeline::new(source, status, Duration::from_millis(100)).unwrap());
        let (user, upstream) = watch::channel(false);
        (
            updates,
            user,
            PtpGroup::new(timeline, 42, upstream).unwrap(),
            origin,
        )
    }
    #[tokio::test]
    async fn producer_uses_pinned_clock_and_rejects_epoch_loss_and_owner_drop() {
        let (updates, _user, group, origin) = fixture();
        let producer = group.producer(MediaClock::at(origin, 0));
        let pcm = [0; sender::PACKET_BYTES];
        let packet = producer.packet(0, &pcm).unwrap();
        assert_eq!(
            packet.ptp_reading().unwrap(),
            Domain::new(7, 1).unwrap().reading(1_000_000_000)
        );
        let initial = *updates.borrow();
        updates.send_replace(Snapshot::inactive());
        assert!(
            producer.packet(0, &pcm).is_err(),
            "producer rejects loss without waiting for monitor tick"
        );
        updates.send_replace(initial);
        drop(group);
        assert!(producer.packet(0, &pcm).is_err());
    }
    struct Blocked {
        entered: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }
    #[tokio::test]
    async fn closed_control_queue_stops_producer_and_renderer_without_clock_loss() {
        let (_updates, _user, group, origin) = fixture();
        let producer = group.producer(MediaClock::at(origin, 0));
        let renderer_stop = group.stop();
        let (_pcm, audio) = tokio::sync::broadcast::channel(4);
        let (control, requests) = crate::output_control::OutputControl::channel(4);
        drop(control);
        let (_catalog, discovery) = watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
        let (state, _) = watch::channel(output_worker::Snapshot::default());
        let entered = Arc::new(AtomicBool::new(false));
        tokio::time::timeout(
            Duration::from_secs(1),
            group.run(
                SessionPool::new(audio),
                requests,
                discovery,
                vec![],
                Blocked {
                    entered: entered.clone(),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                state,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!entered.load(Ordering::SeqCst));
        assert!(*renderer_stop.borrow());
        assert!(producer.packet(0, &[0; sender::PACKET_BYTES]).is_err());
    }
    struct Pending(Arc<AtomicBool>);
    struct TestSink {
        entered: Arc<AtomicBool>,
        written: Arc<tokio::sync::Notify>,
        dropped: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        expected: Arc<AudioPacket>,
        fail: bool,
    }
    impl Drop for TestSink {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    impl sender::LocalSink for TestSink {
        async fn write(&mut self, packet: Arc<AudioPacket>, play_at: Instant) -> Result<()> {
            assert!(Arc::ptr_eq(&packet, &self.expected));
            assert_eq!(play_at, packet.play_at(Duration::from_millis(500)).unwrap());
            while !self.entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            self.written.notify_one();
            if self.fail {
                bail!("offline local sink failure");
            }
            std::future::pending().await
        }
        async fn finish(&mut self) -> Result<()> {
            self.finished.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    #[tokio::test]
    async fn supervised_local_failure_and_clock_loss_release_sink_and_pending_auth() {
        for fail in [true, false] {
            let (updates, _user, group, origin) = fixture();
            let producer = group.producer(MediaClock::at(origin, 0));
            let packet = Arc::new(producer.packet(0, &[0; sender::PACKET_BYTES]).unwrap());
            let (pcm, audio) = tokio::sync::broadcast::channel(4);
            let local_audio = pcm.subscribe();
            pcm.send(packet.clone()).unwrap();
            let (control, requests) = crate::output_control::OutputControl::channel(4);
            let catalog = OutputCatalog::new(vec![sender::outputs::OutputRoute {
                uid: "row".into(),
                name: "speaker".into(),
                model: String::new(),
                aliases: vec![],
                receivers: vec![ReceiverEndpoint {
                    uid: "peer".into(),
                    address: ([127, 0, 0, 1], 7000).into(),
                }],
            }])
            .unwrap();
            let (_catalog, discovery) = watch::channel(Arc::new(catalog));
            let (state, _) = watch::channel(output_worker::Snapshot::default());
            let entered = Arc::new(AtomicBool::new(false));
            let auth_dropped = Arc::new(AtomicBool::new(false));
            let sink_dropped = Arc::new(AtomicBool::new(false));
            let finished = Arc::new(AtomicBool::new(false));
            let written = Arc::new(tokio::sync::Notify::new());
            let reply = control
                .submit(mrp::wire::Node::default().text(2, "row").clone())
                .unwrap();
            let task = tokio::spawn(group.run_with_local(
                SessionPool::new(audio),
                requests,
                discovery,
                vec![],
                Blocked {
                    entered: entered.clone(),
                    dropped: auth_dropped.clone(),
                },
                state,
                TestSink {
                    entered,
                    written: written.clone(),
                    dropped: sink_dropped.clone(),
                    finished: finished.clone(),
                    expected: packet,
                    fail,
                },
                local_audio,
                Duration::from_millis(500),
            ));
            tokio::time::timeout(Duration::from_secs(1), written.notified())
                .await
                .unwrap();
            if !fail {
                updates.send_replace(Snapshot::inactive());
            }
            let result = tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
            if fail {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("offline local sink failure")
                );
            } else {
                result.unwrap();
            }
            assert!(auth_dropped.load(Ordering::SeqCst));
            assert!(sink_dropped.load(Ordering::SeqCst));
            assert!(
                !finished.load(Ordering::SeqCst),
                "cancellation must not drain the sink"
            );
            assert!(reply.await.is_err());
            assert!(producer.packet(0, &[0; sender::PACKET_BYTES]).is_err());
        }
    }
    #[tokio::test]
    async fn local_eof_drains_and_stops_worker_even_with_live_controller() {
        let (_updates, _user, group, origin) = fixture();
        let producer = group.producer(MediaClock::at(origin, 0));
        let expected = Arc::new(producer.packet(0, &[0; sender::PACKET_BYTES]).unwrap());
        let (pcm, audio) = tokio::sync::broadcast::channel(4);
        let local_audio = pcm.subscribe();
        drop(pcm);
        let (_control, requests) = crate::output_control::OutputControl::channel(4);
        let (_catalog, discovery) = watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
        let (state, _) = watch::channel(output_worker::Snapshot::default());
        let dropped = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(AtomicBool::new(false));
        tokio::time::timeout(
            Duration::from_secs(1),
            group.run_with_local(
                SessionPool::new(audio),
                requests,
                discovery,
                vec![],
                Blocked {
                    entered: entered.clone(),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                state,
                TestSink {
                    entered,
                    written: Arc::new(tokio::sync::Notify::new()),
                    dropped: dropped.clone(),
                    finished: finished.clone(),
                    expected,
                    fail: false,
                },
                local_audio,
                Duration::from_millis(500),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(finished.load(Ordering::SeqCst));
        assert!(dropped.load(Ordering::SeqCst));
        assert!(producer.packet(0, &[0; sender::PACKET_BYTES]).is_err());
    }
    impl Drop for Pending {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl SessionPreparer for Blocked {
        async fn prepare(&mut self, _: ReceiverEndpoint) -> Result<Session> {
            let _pending = Pending(self.dropped.clone());
            self.entered.store(true, Ordering::SeqCst);
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn clock_loss_cancels_pending_auth_worker_and_shared_renderer_stop() {
        let (updates, _user, group, origin) = fixture();
        let producer = group.producer(MediaClock::at(origin, 0));
        let mut renderer_stop = group.stop();
        let (_pcm, audio) = tokio::sync::broadcast::channel(4);
        let (control, requests) = crate::output_control::OutputControl::channel(4);
        let catalog = OutputCatalog::new(vec![sender::outputs::OutputRoute {
            uid: "row".into(),
            name: "speaker".into(),
            model: String::new(),
            aliases: vec![],
            receivers: vec![ReceiverEndpoint {
                uid: "peer".into(),
                address: ([127, 0, 0, 1], 7000).into(),
            }],
        }])
        .unwrap();
        let (_catalog, discovery) = watch::channel(Arc::new(catalog));
        let (state, final_state) = watch::channel(output_worker::Snapshot::default());
        let entered = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(group.run(
            SessionPool::new(audio),
            requests,
            discovery,
            vec![],
            Blocked {
                entered: entered.clone(),
                dropped: dropped.clone(),
            },
            state,
        ));
        let reply = control
            .submit(mrp::wire::Node::default().text(2, "row").clone())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !entered.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        updates.send_replace(Snapshot::inactive());
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(dropped.load(Ordering::SeqCst));
        assert!(reply.await.is_err());
        assert!(*renderer_stop.borrow_and_update());
        assert!(final_state.borrow().selected.is_empty());
        assert!(producer.packet(0, &[0; sender::PACKET_BYTES]).is_err());
    }
}
