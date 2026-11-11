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
use std::sync::{Arc, Weak};
use timesync::media::{MasterLease, MasterTimeline};
use tokio::sync::{mpsc, watch};

/// Own this alongside the clock service. A new epoch requires a new group.
/// Dropping it cancels the worker, producer and any local renderer subscribed
/// to `stop()`. Graceful shutdown must await `run`, not abort its future.
pub struct PtpGroup {
    timeline: Arc<MasterTimeline>,
    group: u128,
    lease: MasterLease,
    clock_peers: Option<ClockPeers>,
}
#[derive(Clone)]
struct ClockPeers {
    service: Weak<timesync::service::Service>,
    owner: u128,
    ports: timesync::sender::Ports,
}
impl ClockPeers {
    async fn apply(&self, ips: &[std::net::IpAddr]) -> Result<()> {
        let Some(service) = self.service.upgrade() else {
            if ips.is_empty() {
                return Ok(());
            }
            bail!("shared clock service ended");
        };
        let peers = ips
            .iter()
            .map(|&ip| timesync::sender::Peer {
                ip,
                ports: self.ports,
            })
            .collect();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            service.set_group_peers(self.owner, peers),
        )
        .await??;
        Ok(())
    }
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
            clock_peers: None,
        })
    }
    /// Bind to the EXISTING shared service; never start/stop a service here.
    /// Caller supplies a fresh ownership token and negotiated timing ports.
    pub fn with_clock_service(
        mut self,
        service: &Arc<timesync::service::Service>,
        owner: u128,
        ports: timesync::sender::Ports,
    ) -> Result<Self> {
        if owner == 0
            || ports.event == 0
            || ports.general == 0
            || !self.timeline.follows(&service.subscribe())
        {
            bail!("invalid timing ownership/ports or another clock service");
        }
        self.clock_peers = Some(ClockPeers {
            service: Arc::downgrade(service),
            owner,
            ports,
        });
        Ok(self)
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
            clock_peers: self.clock_peers.clone(),
            last_members: None,
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
    clock_peers: Option<ClockPeers>,
    last_members: Option<Vec<std::net::IpAddr>>,
}
impl<P: SessionPreparer> SessionPreparer for QualifiedPreparer<P> {
    async fn active(&mut self, pool: &SessionPool) -> Result<()> {
        self.timeline.reading_at(std::time::Instant::now())?;
        self.inner.active(pool).await?;
        let members = pool.sync_ptp_members()?;
        if self.last_members.as_ref() != Some(&members) {
            if let Some(clock) = &self.clock_peers {
                clock.apply(&members).await?;
            }
            self.last_members = Some(members);
        }
        Ok(())
    }
    async fn finish(&mut self) -> Result<()> {
        let inner = self.inner.finish().await;
        // Queue release even after a timed-out/cancelled update: the shared
        // service serializes both commands, preserving release-after-update.
        let release = match &self.clock_peers {
            Some(clock) => clock.apply(&[]).await,
            None => Ok(()),
        };
        inner.and(release)
    }
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
    async fn shared_clock_binding_releases_only_its_owner_after_worker_exit() {
        use timesync::{
            sender::{Config as Network, Peer, Ports},
            service::{Config, Service},
        };
        let origin = Instant::now();
        let domain = Domain::new(7, 1).unwrap();
        let source = Arc::new(InstantSource::new(
            origin,
            LocalClock::new(1).unwrap(),
            domain,
            1_000_000_000,
        ));
        let service = Arc::new(
            Service::bind(
                Config {
                    network: Network {
                        domain,
                        bind_ip: "127.0.0.1".parse().unwrap(),
                        ports: Ports {
                            event: 0,
                            general: 0,
                        },
                        peers: vec![],
                    },
                    election: clock::role::Config {
                        policy: clock::role::Policy::Elect,
                        settle_ns: 1_000_000,
                        announce_lease_ns: 200_000_000,
                        min_announces: 2,
                    },
                    follow: clock::follower::Config {
                        pairing_lease_ns: 100_000_000,
                        mapping_lease_ns: 150_000_000,
                        warmup_samples: 2,
                        step_threshold_ns: 50_000_000,
                    },
                    probe: None,
                },
                source.clone(),
            )
            .await
            .unwrap(),
        );
        let addresses = service.addresses();
        let mut status = service.subscribe();
        tokio::time::timeout(Duration::from_secs(1), async {
            while status.borrow_and_update().selection.role != Role::Master {
                status.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let selected = status.borrow().selection;
        let timeline =
            Arc::new(MasterTimeline::new(source, status, Duration::from_millis(100)).unwrap());
        let (_user, upstream) = watch::channel(false);
        let event = tokio::net::UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let general = tokio::net::UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let ports = Ports {
            event: event.local_addr().unwrap().port(),
            general: general.local_addr().unwrap().port(),
        };
        let group = PtpGroup::new(timeline, 42, upstream)
            .unwrap()
            .with_clock_service(&service, 400, ports)
            .unwrap();
        let (_updates, _other_user, unrelated, _) = fixture();
        assert!(unrelated.with_clock_service(&service, 401, ports).is_err());
        let peer_ip = event.local_addr().unwrap().ip();
        let binding = group.clock_peers.as_ref().unwrap();
        binding.apply(&[peer_ip]).await.unwrap();
        service
            .set_group_peers(900, vec![Peer { ip: peer_ip, ports }])
            .await
            .unwrap();
        let (_pcm, audio) = tokio::sync::broadcast::channel(4);
        let (control, requests) = crate::output_control::OutputControl::channel(4);
        drop(control);
        let (_catalog, discovery) = watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
        let (state, _) = watch::channel(output_worker::Snapshot::default());
        group
            .run(
                SessionPool::new(audio),
                requests,
                discovery,
                vec![],
                Blocked {
                    entered: Arc::new(AtomicBool::new(false)),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                state,
            )
            .await
            .unwrap();
        let mut buffer = [0; 1024];
        while event.try_recv(&mut buffer).is_ok() {}
        tokio::time::timeout(Duration::from_secs(1), event.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(service.addresses(), addresses);
        assert_eq!(service.subscribe().borrow().selection, selected);
        service.set_group_peers(900, vec![]).await.unwrap();
        while event.try_recv(&mut buffer).is_ok() {}
        assert!(
            tokio::time::timeout(Duration::from_millis(150), event.recv_from(&mut buffer))
                .await
                .is_err(),
            "worker must release owner400; otherwise traffic survives owner900 release"
        );
        Arc::try_unwrap(service)
            .ok()
            .unwrap()
            .shutdown()
            .await
            .unwrap();
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
