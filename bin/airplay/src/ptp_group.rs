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

enum GroupTimeline {
    Local(Arc<MasterTimeline>),
    External(Arc<timesync::external_media::Timeline>),
}
impl GroupTimeline {
    fn domain(&self) -> clock::domain::Domain {
        match self {
            Self::Local(t) => t.domain(),
            Self::External(t) => t.domain(),
        }
    }
    fn reading_at(&self, at: std::time::Instant) -> std::io::Result<clock::domain::Reading> {
        match self {
            Self::Local(t) => t.reading_at(at),
            Self::External(t) => t.reading_at(at),
        }
    }
    fn follows(&self, status: &watch::Receiver<timesync::engine::Snapshot>) -> bool {
        match self {
            Self::Local(t) => t.follows(status),
            Self::External(_) => false,
        }
    }
}
/// Own this alongside the clock service. A new epoch requires a new group.
/// Dropping it cancels the worker, producer and any local renderer subscribed
/// to `stop()`. Graceful shutdown must await `run`, not abort its future.
pub struct PtpGroup {
    timeline: Arc<GroupTimeline>,
    group: u128,
    lease: MasterLease,
    clock_peers: Option<ClockPeers>,
}
/// Source adapters own PCM acquisition and publish state before new-generation
/// PCM. Timeline and clock must share one epoch; broadcast validates it.
pub struct ContinuousSource {
    pub timeline: sender::continuous::Timeline,
    pub clock: MediaClock,
    pub input: mpsc::Receiver<sender::continuous_source::SourceFrame>,
    pub controls: watch::Receiver<sender::continuous_source::State>,
}
/// Inject a tap already owned by the adapter; no process or device is opened.
pub struct RawPcmSource<R> {
    pub reader: R,
    pub epoch: std::time::Instant,
    pub latency: std::time::Duration,
    pub queue_capacity: usize,
}
impl ContinuousSource {
    pub fn for_card(
        card: &crate::card::Card,
        timeline: sender::continuous::Timeline,
        clock: MediaClock,
        input: mpsc::Receiver<sender::continuous_source::SourceFrame>,
    ) -> Self {
        Self {
            timeline,
            clock,
            input,
            controls: card.continuous_controls(),
        }
    }
}
pub struct GroupOutputs<P> {
    pub requests: mpsc::Receiver<OutputRequest>,
    pub discovery: watch::Receiver<Arc<OutputCatalog>>,
    pub local_ids: Vec<String>,
    pub preparer: P,
    pub updates: watch::Sender<output_worker::Snapshot>,
}
/// Host-owned, serialized peer control for an external physical-clock daemon.
/// The controller must keep per-owner interests, reject stale domains, and
/// serialize withdrawal after any partially applied/cancelled replacement.
/// Dropping a group never grants permission to stop the daemon or other owners.
pub type PeerControlUpdate<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;
pub trait ExternalPeerControl: Send + Sync {
    fn replace(
        &self,
        domain: clock::domain::Domain,
        owner: u128,
        peers: Vec<std::net::IpAddr>,
    ) -> PeerControlUpdate<'_>;
}
#[derive(Clone)]
enum ClockPeers {
    Service {
        service: Weak<timesync::service::Service>,
        owner: u128,
        ports: timesync::sender::Ports,
    },
    External {
        control: Arc<dyn ExternalPeerControl>,
        owner: u128,
        domain: clock::domain::Domain,
    },
}
impl ClockPeers {
    async fn apply(&self, ips: &[std::net::IpAddr]) -> Result<()> {
        match self {
            Self::Service {
                service,
                owner,
                ports,
            } => {
                let Some(service) = service.upgrade() else {
                    if ips.is_empty() {
                        return Ok(());
                    }
                    bail!("shared clock service ended");
                };
                let peers = ips
                    .iter()
                    .map(|&ip| timesync::sender::Peer { ip, ports: *ports })
                    .collect();
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    service.set_group_peers(*owner, peers),
                )
                .await??;
            }
            Self::External {
                control,
                owner,
                domain,
            } => {
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    control.replace(*domain, *owner, ips.to_vec()),
                )
                .await??;
            }
        }
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
            timeline: Arc::new(GroupTimeline::Local(timeline)),
            group,
            lease,
            clock_peers: None,
        })
    }
    /// Explicit qualified external clock. Caller owns attested daemon control
    /// and timing peers through its preparer; this never starts a second PTP
    /// service or manufactures a Service snapshot. Publish first, then construct.
    pub fn new_external(
        timeline: Arc<timesync::external_media::Timeline>,
        group: u128,
        upstream: watch::Receiver<bool>,
    ) -> Result<Self> {
        if group == 0 {
            bail!("empty outgoing group identity");
        }
        timeline.reading_at(std::time::Instant::now())?;
        let lease = MasterLease::spawn_external(timeline.clone(), upstream);
        Ok(Self {
            timeline: Arc::new(GroupTimeline::External(timeline)),
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
        self.clock_peers = Some(ClockPeers::Service {
            service: Arc::downgrade(service),
            owner,
            ports,
        });
        Ok(self)
    }
    /// Supply the existing external daemon's owner. It must implement the
    /// serialized per-owner contract above; no direct SHM/control-port inference.
    pub fn with_external_peer_control(
        mut self,
        owner: u128,
        control: Arc<dyn ExternalPeerControl>,
    ) -> Result<Self> {
        if owner == 0
            || self.clock_peers.is_some()
            || !matches!(&*self.timeline, GroupTimeline::External(_))
        {
            bail!("external peer owner requires an unbound external clock group");
        }
        self.timeline.reading_at(std::time::Instant::now())?;
        self.clock_peers = Some(ClockPeers::External {
            control,
            owner,
            domain: self.timeline.domain(),
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
    /// Own the Card observer and the continuous runtime together. A card/source
    /// mismatch fails before startup; any observer failure stops and awaits the
    /// pipeline instead of dropping a still-running pool. No hidden tasks.
    pub async fn run_card_continuous_with_local(
        self,
        card: Arc<crate::card::Card>,
        source: ContinuousSource,
        outputs: GroupOutputs<impl SessionPreparer>,
        sink: impl sender::LocalSink,
        latency: std::time::Duration,
    ) -> Result<sender::continuous_source::End> {
        if !source.controls.same_channel(&card.continuous_controls()) {
            bail!("continuous source belongs to another Card");
        }
        let cancel = self.lease.cancellation();
        let observer = card.follow_continuous_source(self.stop());
        let pipeline = self.run_continuous_with_local(source, outputs, sink, latency);
        tokio::pin!(observer, pipeline);
        tokio::select! {
            result = &mut pipeline => {
                cancel.cancel();
                let observed = observer.await;
                let end = result?;
                observed?;
                Ok(end)
            },
            observed = &mut observer => {
                cancel.cancel();
                let cleanup = pipeline.await;
                observed?;
                cleanup
            },
        }
    }
    /// Library host composition: raw tap, Card observer, qualified PTP pool and
    /// local sink share one cancellation owner. EOF drains; errors cancel all
    /// branches, whose cleanup is awaited. The adapter retains route ownership.
    pub async fn run_card_pcm_with_local(
        self,
        card: Arc<crate::card::Card>,
        source: RawPcmSource<impl tokio::io::AsyncRead + Unpin>,
        outputs: GroupOutputs<impl SessionPreparer>,
        sink: impl sender::LocalSink,
    ) -> Result<sender::continuous_source::End> {
        let local_lead = source.latency;
        self.run_card_pcm_with_local_lead(card, source, outputs, sink, local_lead)
            .await
    }
    /// Calibrate local presentation without changing network scheduling tolerance,
    /// source epoch, RTP timestamps or receiver lead. The adapter owns calibration.
    pub async fn run_card_pcm_with_local_lead(
        self,
        card: Arc<crate::card::Card>,
        source: RawPcmSource<impl tokio::io::AsyncRead + Unpin>,
        outputs: GroupOutputs<impl SessionPreparer>,
        sink: impl sender::LocalSink,
        local_lead: std::time::Duration,
    ) -> Result<sender::continuous_source::End> {
        use sender::continuous_source::{self, End};
        if !(std::time::Duration::from_millis(250)..=std::time::Duration::from_secs(10))
            .contains(&local_lead)
        {
            bail!("invalid calibrated local lead");
        }
        if !(1..=512).contains(&source.queue_capacity) {
            bail!("invalid PCM input capacity");
        }
        let timeline = sender::continuous::Timeline::new(source.epoch, source.latency)?;
        let (pcm, input) = mpsc::channel(source.queue_capacity);
        let controls = card.continuous_controls();
        let cancel = self.lease.cancellation();
        let reader = continuous_source::read_pcm(source.reader, pcm, controls, self.stop());
        let source_control =
            ContinuousSource::for_card(&card, timeline, MediaClock::at(source.epoch, 0), input);
        let pipeline =
            self.run_card_continuous_with_local(card, source_control, outputs, sink, local_lead);
        tokio::pin!(reader, pipeline);
        tokio::select! {
            biased;
            result = &mut pipeline => {
                cancel.cancel();
                let read = reader.await;
                let end = result?;
                read?;
                Ok(end)
            },
            read = &mut reader => {
                if !matches!(read, Ok(End::Eof)) { cancel.cancel(); }
                let result = pipeline.await;
                read?;
                result
            },
        }
    }
    /// Compose the existing source, pool, selection and local supervisors.
    /// Final EOF drains; any failure/stop cancels and awaits both branches.
    /// No discovery/clock/device startup, source decoding or route changes.
    pub async fn run_continuous_with_local(
        self,
        source: ContinuousSource,
        outputs: GroupOutputs<impl SessionPreparer>,
        sink: impl sender::LocalSink,
        latency: std::time::Duration,
    ) -> Result<sender::continuous_source::End> {
        use sender::continuous_source::{self, End};
        if latency < std::time::Duration::from_millis(250)
            || latency > std::time::Duration::from_secs(10)
        {
            bail!("invalid continuous local lead");
        }
        let frames = (latency.as_nanos() * u128::from(sender::SAMPLE_RATE))
            .div_ceil(1_000_000_000 * sender::FRAMES_PER_PACKET as u128);
        let (audio, packets) = tokio::sync::broadcast::channel(frames as usize + 32);
        let local = audio.subscribe();
        let producer = self.producer(source.clock);
        let stop = self.stop();
        let cancel = self.lease.cancellation();
        let pump = continuous_source::broadcast(
            source.timeline,
            source.input,
            source.controls,
            audio,
            stop,
            move |index, pcm| producer.packet(index, pcm),
        );
        let runtime = self.run_with_local(
            SessionPool::new(packets),
            outputs.requests,
            outputs.discovery,
            outputs.local_ids,
            outputs.preparer,
            outputs.updates,
            sink,
            local,
            latency,
        );
        tokio::pin!(pump, runtime);
        tokio::select! {
            result = &mut pump => {
                if !matches!(result, Ok(End::Eof)) { cancel.cancel(); }
                let cleanup = runtime.await;
                let end = result?;
                cleanup?;
                Ok(end)
            },
            result = &mut runtime => {
                cancel.cancel();
                let source_end = pump.await;
                result?;
                source_end
            },
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
    #[allow(
        clippy::too_many_arguments,
        reason = "Preserve existing host API; new composition uses GroupOutputs"
    )]
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
    timeline: Arc<GroupTimeline>,
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
    timeline: Arc<GroupTimeline>,
    group: u128,
    clock_peers: Option<ClockPeers>,
    last_members: Option<Vec<std::net::IpAddr>>,
}
impl<P: SessionPreparer> SessionPreparer for QualifiedPreparer<P> {
    async fn active(&mut self, pool: &SessionPool) -> Result<()> {
        self.timeline.reading_at(std::time::Instant::now())?;
        let members = pool.sync_ptp_members()?;
        if self.last_members.as_ref() != Some(&members) {
            if let Some(clock) = &self.clock_peers {
                clock.apply(&members).await?;
            }
            self.last_members = Some(members);
        }
        // Host handoff hooks may change a hardware route. Apply clock peers
        // first and recheck the lease after awaiting that external operation.
        self.timeline.reading_at(std::time::Instant::now())?;
        self.inner.active(pool).await?;
        self.timeline.reading_at(std::time::Instant::now())?;
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
        self.prepare_authorized(receiver, None).await
    }
    async fn prepare_authorized(
        &mut self,
        receiver: ReceiverEndpoint,
        authorization: Option<&crate::route_pairing::AuthorizationContext>,
    ) -> Result<Session> {
        self.timeline.reading_at(std::time::Instant::now())?;
        let session = self
            .inner
            .prepare_authorized(receiver, authorization)
            .await?;
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
    type PeerCalls = Vec<(Domain, u128, Vec<std::net::IpAddr>)>;
    #[derive(Default)]
    struct PeerRecorder(std::sync::Mutex<PeerCalls>);
    impl ExternalPeerControl for PeerRecorder {
        fn replace(
            &self,
            domain: Domain,
            owner: u128,
            peers: Vec<std::net::IpAddr>,
        ) -> PeerControlUpdate<'_> {
            Box::pin(async move {
                self.0.lock().unwrap().push((domain, owner, peers));
                Ok(())
            })
        }
    }
    #[tokio::test]
    async fn local_clock_cannot_be_relabelled_as_external_peer_control() {
        let (_updates, _user, group, _) = fixture();
        assert!(
            group
                .with_external_peer_control(99, Arc::new(PeerRecorder::default()))
                .is_err()
        );
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
    struct PipelineSink {
        seen: Arc<std::sync::Mutex<Vec<Arc<AudioPacket>>>>,
        finished: Arc<AtomicBool>,
        fail: bool,
    }
    impl sender::LocalSink for PipelineSink {
        async fn write(&mut self, packet: Arc<AudioPacket>, _: Instant) -> Result<()> {
            self.seen.lock().unwrap().push(packet);
            if self.fail {
                bail!("pipeline sink failed");
            }
            Ok(())
        }
        async fn finish(&mut self) -> Result<()> {
            self.finished.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    fn pipeline_card(remote: crate::remote::Remote) -> Arc<crate::card::Card> {
        crate::card::Card::new(
            mrp::messages::Device {
                name: "Pipeline card".into(),
                model: "AirPlayReceiver".into(),
                localized_model: "Receiver".into(),
                uid: "pipeline-card".into(),
                group: "pipeline-group".into(),
                system_app: "com.github.corsairguy.airplay".into(),
            },
            remote,
            Arc::new(crate::events::NoEvents),
        )
    }
    #[tokio::test]
    async fn external_clock_stamps_packets_and_owner_loss_stops_group() {
        use clock::external::{Correlation, Selection, StartMarker};
        let origin = Instant::now();
        let selection = Selection {
            daemon_instance: 42,
            domain: clock::domain::Domain::new(55, 2).unwrap(),
            master_start_raw_ns: 100,
            start_marker: StartMarker::RefreshedPublication,
            clock_change_offset: None,
            sender_master_verified: true,
        };
        let sample = clock::nqptp::Snapshot {
            version: 10,
            clock_id: 55,
            local_time: 1_000_000_000,
            offset: 37,
            clock_change_offset: None,
            master_start_time: 100,
        };
        let correlation = Correlation {
            at: origin,
            raw_ns: 1_000_000_100,
            rate_ppb: 0,
            rate_error_ppb: 10,
            uncertainty_ns: 10,
            valid_for: Duration::from_secs(1),
        };
        let (timeline, mut owner) = timesync::external_media::Timeline::new(
            selection,
            Duration::from_secs(1),
            Duration::from_micros(1),
            Duration::from_millis(100),
        )
        .unwrap();
        let (_user, upstream) = watch::channel(false);
        assert!(PtpGroup::new_external(timeline.clone(), 77, upstream.clone()).is_err());
        owner.update(selection, sample, correlation).unwrap();
        let recorder = Arc::new(PeerRecorder::default());
        assert!(
            PtpGroup::new_external(timeline.clone(), 77, upstream.clone())
                .unwrap()
                .with_external_peer_control(0, recorder.clone())
                .is_err()
        );
        let group = PtpGroup::new_external(timeline.clone(), 77, upstream)
            .unwrap()
            .with_external_peer_control(701, recorder.clone())
            .unwrap();
        let binding = group.clock_peers.clone().unwrap();
        let peer = "127.0.0.1".parse().unwrap();
        binding.apply(&[peer]).await.unwrap();

        let producer = group.producer(MediaClock::at(origin, 0));
        let pcm = vec![7; sender::PACKET_BYTES];
        let packet = producer.packet(0, &pcm).unwrap();
        assert_eq!(packet.pcm(), pcm.as_slice());
        assert_eq!(
            packet.ptp_reading().unwrap(),
            selection.domain.reading(1_000_000_137)
        );
        assert_eq!(packet.produced_at(), origin);
        let mut preparer = QualifiedPreparer {
            inner: Blocked {
                entered: Arc::new(AtomicBool::new(false)),
                dropped: Arc::new(AtomicBool::new(false)),
            },
            timeline: group.timeline.clone(),
            group: 77,
            clock_peers: Some(binding),
            last_members: None,
        };
        preparer.finish().await.unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                (selection.domain, 701, vec![peer]),
                (selection.domain, 701, vec![])
            ]
        );
        let mut stop = group.stop();
        drop(owner);
        assert!(producer.packet(1, &pcm).is_err());
        tokio::time::timeout(Duration::from_secs(1), async {
            while !*stop.borrow_and_update() {
                stop.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(*stop.borrow());
    }
    #[tokio::test]
    async fn raw_pcm_host_drains_input_and_awaits_cleanup_on_malformed_tail() {
        struct HostSink {
            inner: PipelineSink,
            released: Arc<AtomicBool>,
        }
        impl Drop for HostSink {
            fn drop(&mut self) {
                self.released.store(true, Ordering::SeqCst);
            }
        }
        impl sender::LocalSink for HostSink {
            async fn write(&mut self, packet: Arc<AudioPacket>, at: Instant) -> Result<()> {
                assert_eq!(
                    at.duration_since(packet.produced_at()),
                    Duration::from_millis(375)
                );
                self.inner.write(packet, at).await
            }
            async fn finish(&mut self) -> Result<()> {
                self.inner.finish().await
            }
        }
        for malformed in [false, true] {
            let (_clock, _user, group, origin) = fixture();
            let card = pipeline_card(crate::remote::Remote::new());
            let (_control, requests) = crate::output_control::OutputControl::channel(4);
            let (_catalog, discovery) =
                watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
            let (updates, state) = watch::channel(output_worker::Snapshot::default());
            let outputs = GroupOutputs {
                requests,
                discovery,
                local_ids: vec![],
                preparer: Blocked {
                    entered: Arc::new(AtomicBool::new(false)),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                updates,
            };
            let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
            let finished = Arc::new(AtomicBool::new(false));
            let released = Arc::new(AtomicBool::new(false));
            let source = RawPcmSource {
                reader: std::io::Cursor::new(vec![
                    9;
                    if malformed { 2 } else { sender::PACKET_BYTES }
                ]),
                epoch: origin,
                latency: Duration::from_millis(250),
                queue_capacity: 2,
            };
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                group.run_card_pcm_with_local_lead(
                    card,
                    source,
                    outputs,
                    HostSink {
                        inner: PipelineSink {
                            seen: seen.clone(),
                            finished: finished.clone(),
                            fail: false,
                        },
                        released: released.clone(),
                    },
                    Duration::from_millis(375),
                ),
            )
            .await
            .unwrap();
            if malformed {
                assert!(result.unwrap_err().to_string().contains("aligned"));
                assert!(seen.lock().unwrap().is_empty());
            } else {
                assert_eq!(result.unwrap(), sender::continuous_source::End::Eof);
                assert!(
                    seen.lock()
                        .unwrap()
                        .iter()
                        .any(|packet| packet.pcm()[0] == 9)
                );
            }
            assert_eq!(finished.load(Ordering::SeqCst), !malformed);
            assert!(released.load(Ordering::SeqCst));
            assert!(state.borrow().selected.is_empty());
        }
    }
    #[tokio::test]
    async fn card_mrp_pause_play_reaches_continuous_pcm_and_all_owners_end() {
        use crate::{
            events::Events,
            remote::{Command, Remote},
        };
        use tokio::io::AsyncWriteExt;
        let (_clock, _user, group, origin) = fixture();
        let remote = Remote::new();
        let mut commands = remote.attach(1);
        let card = pipeline_card(remote);
        card.playing(true);
        let mut states = card.continuous_controls();
        let (pcm, input) = mpsc::channel(8);
        pcm.send(sender::continuous_source::SourceFrame::new(
            0,
            Box::new([7; sender::PACKET_BYTES]),
        ))
        .await
        .unwrap();
        let source = ContinuousSource::for_card(
            &card,
            sender::continuous::Timeline::new(origin, Duration::from_millis(250)).unwrap(),
            MediaClock::at(origin, 0),
            input,
        );
        let (_control, requests) = crate::output_control::OutputControl::channel(4);
        let (_catalog, discovery) = watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
        let (updates, final_state) = watch::channel(output_worker::Snapshot::default());
        let outputs = GroupOutputs {
            requests,
            discovery,
            local_ids: vec![],
            preparer: Blocked {
                entered: Arc::new(AtomicBool::new(false)),
                dropped: Arc::new(AtomicBool::new(false)),
            },
            updates,
        };
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let finished = Arc::new(AtomicBool::new(false));
        let pipeline = tokio::spawn(group.run_card_continuous_with_local(
            card.clone(),
            source,
            outputs,
            PipelineSink {
                seen: seen.clone(),
                finished: finished.clone(),
                fail: false,
            },
            Duration::from_millis(250),
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            while seen.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (mut client, server) = tokio::io::duplex(65536);
        let mrp = tokio::spawn(card.clone().serve(server));
        let command = |seq, value| {
            let mut body = mrp::wire::Node::default();
            body.int(1, value);
            let mut message = mrp::wire::Node::default();
            message.int(1, 1).text(2, "pipeline-command").node(6, &body);
            mrp::frame::sync(seq, &[message])
        };
        client.write_all(&command(1, 2)).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), commands.recv())
                .await
                .unwrap(),
            Some(Command::Pause)
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while !states.borrow_and_update().paused {
                states.changed().await.unwrap();
            }
            while !seen
                .lock()
                .unwrap()
                .iter()
                .any(|frame| frame.pcm().iter().all(|byte| *byte == 0))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        client.write_all(&command(2, 1)).await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), commands.recv())
                .await
                .unwrap(),
            Some(Command::Play)
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            while states.borrow_and_update().paused {
                states.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        pcm.send(sender::continuous_source::SourceFrame::new(
            0,
            Box::new([9; sender::PACKET_BYTES]),
        ))
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !seen.lock().unwrap().iter().any(|frame| frame.pcm()[0] == 9) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(pcm);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), pipeline)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            sender::continuous_source::End::Eof
        );
        assert!(finished.load(Ordering::SeqCst));
        assert!(final_state.borrow().selected.is_empty());
        drop(client);
        let _closed = tokio::time::timeout(Duration::from_secs(1), mrp)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn card_pipeline_rejects_unrelated_source_control_before_startup() {
        let (_clock, _user, group, origin) = fixture();
        let card = pipeline_card(crate::remote::Remote::new());
        let (_pcm, input) = mpsc::channel(1);
        let (_state, controls) = watch::channel(sender::continuous_source::State::default());
        let source = ContinuousSource {
            timeline: sender::continuous::Timeline::new(origin, Duration::from_millis(250))
                .unwrap(),
            clock: MediaClock::at(origin, 0),
            input,
            controls,
        };
        let (_control, requests) = crate::output_control::OutputControl::channel(4);
        let (_catalog, discovery) = watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
        let (updates, _) = watch::channel(output_worker::Snapshot::default());
        let outputs = GroupOutputs {
            requests,
            discovery,
            local_ids: vec![],
            preparer: Blocked {
                entered: Arc::new(AtomicBool::new(false)),
                dropped: Arc::new(AtomicBool::new(false)),
            },
            updates,
        };
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let finished = Arc::new(AtomicBool::new(false));
        assert!(
            group
                .run_card_continuous_with_local(
                    card,
                    source,
                    outputs,
                    PipelineSink {
                        seen: seen.clone(),
                        finished: finished.clone(),
                        fail: false
                    },
                    Duration::from_millis(250)
                )
                .await
                .is_err()
        );
        assert!(seen.lock().unwrap().is_empty());
        assert!(!finished.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn continuous_pipeline_drains_eof_and_cancels_both_branches_on_failures() {
        for mode in 0..3 {
            let (_updates, _user, group, origin) = fixture();
            let domain = group.timeline.domain();
            let (source, input) = mpsc::channel(4);
            source
                .send(sender::continuous_source::SourceFrame::new(
                    0,
                    Box::new([3; sender::PACKET_BYTES]),
                ))
                .await
                .unwrap();
            // Keep the source live during sink failure: cleanup cannot rely on EOF.
            if mode != 1 {
                drop(source);
            }
            let (_state, controls) = watch::channel(sender::continuous_source::State::default());
            let clock_origin = if mode == 2 {
                origin + Duration::from_secs(1)
            } else {
                origin
            };
            let source = ContinuousSource {
                timeline: sender::continuous::Timeline::new(origin, Duration::from_millis(250))
                    .unwrap(),
                clock: MediaClock::at(clock_origin, 0),
                input,
                controls,
            };
            let (_control, requests) = crate::output_control::OutputControl::channel(4);
            let (_catalog, discovery) =
                watch::channel(Arc::new(OutputCatalog::new(vec![]).unwrap()));
            let (updates, final_state) = watch::channel(output_worker::Snapshot::default());
            let outputs = GroupOutputs {
                requests,
                discovery,
                local_ids: vec![],
                preparer: Blocked {
                    entered: Arc::new(AtomicBool::new(false)),
                    dropped: Arc::new(AtomicBool::new(false)),
                },
                updates,
            };
            let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
            let finished = Arc::new(AtomicBool::new(false));
            let result = tokio::time::timeout(
                Duration::from_secs(2),
                group.run_continuous_with_local(
                    source,
                    outputs,
                    PipelineSink {
                        seen: seen.clone(),
                        finished: finished.clone(),
                        fail: mode == 1,
                    },
                    Duration::from_millis(250),
                ),
            )
            .await
            .unwrap();
            if mode == 0 {
                assert_eq!(result.unwrap(), sender::continuous_source::End::Eof);
                let seen = seen.lock().unwrap();
                assert_eq!(seen.len(), 1);
                assert_eq!(seen[0].pcm(), &[3; sender::PACKET_BYTES]);
                assert_eq!(seen[0].ptp_reading().unwrap().domain(), domain);
                assert!(finished.load(Ordering::SeqCst));
            } else {
                assert!(result.is_err());
                assert!(!finished.load(Ordering::SeqCst));
                if mode == 2 {
                    assert!(seen.lock().unwrap().is_empty());
                }
            }
            assert!(final_state.borrow().selected.is_empty());
        }
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
        let event = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
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
