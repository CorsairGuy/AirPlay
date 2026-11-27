use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use crypto::kdf::ControlKeys;
use pair::{
    Identity as PairIdentity,
    hap::{Accessory, ControllerSetup, ControllerVerify},
};
use rand::RngCore;
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::{broadcast, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{sleep, sleep_until, timeout},
};

use crate::tx::{
    AudioPacket, FRAMES_PER_PACKET, SAMPLE_RATE,
    packet::frames_duration,
    transport::{Connection, Rtsp},
};

/// Valid AirPlay attenuation in dB, including the protocol's mute sentinel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume(f32);

impl Volume {
    pub const MUTE: Self = Self(-144.0);

    pub fn db(value: f32) -> Result<Self> {
        if value.is_finite() && (value == -144.0 || (-30.0..=0.0).contains(&value)) {
            Ok(Self(value))
        } else {
            bail!("volume must be -144 (mute) or between -30 and 0 dB");
        }
    }

    pub fn normalized(value: f32) -> Result<Self> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            bail!("volume must be between 0 and 1");
        }
        Self::db(if value == 0.0 {
            -144.0
        } else {
            value * 30.0 - 30.0
        })
    }

    pub fn as_db(self) -> f32 {
        self.0
    }
}

/// Обложка в обновлении трека: не менять, заменить или очистить старую.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Artwork {
    #[default]
    Unchanged,
    Set {
        mime: String,
        data: Vec<u8>,
    },
    Clear,
}

/// Метаданные, которые sender отправляет приёмнику существующими
/// SET_PARAMETER-запросами: DMAP (название/исполнитель/альбом), обложка и
/// progress. Позиция — в кадрах 44,1 кГц, как в `progress:` AirPlay.
pub const MAX_ARTWORK_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub artwork: Artwork,
    /// Текущая позиция и длительность в кадрах; progress шлётся только
    /// когда известны оба значения.
    pub position_frames: Option<u64>,
    pub duration_frames: Option<u64>,
}

impl TrackMetadata {
    fn validate(&self) -> Result<()> {
        for value in [&self.title, &self.artist, &self.album]
            .into_iter()
            .flatten()
        {
            if value.len() > 4096 || value.chars().any(char::is_control) {
                bail!("metadata text is invalid or too long");
            }
        }
        if self.duration_frames == Some(0) {
            bail!("metadata duration must be nonzero");
        }
        if self
            .position_frames
            .zip(self.duration_frames)
            .is_some_and(|(position, duration)| position > duration)
        {
            bail!("metadata position exceeds duration");
        }
        if let Artwork::Set { mime, data } = &self.artwork {
            if data.is_empty() || data.len() > MAX_ARTWORK_BYTES {
                bail!("artwork size is outside allowed range");
            }
            if !matches!(mime.as_str(), "image/jpeg" | "image/png") {
                bail!("artwork must be JPEG or PNG");
            }
        }
        Ok(())
    }
}

/// Клон, который можно держать во время `Session::run`: сессия забирает себя,
/// а метаданные и смена трека идут через этот канал.
#[derive(Clone)]
pub struct MetadataHandle {
    updates: watch::Sender<Option<TrackMetadata>>,
}

impl MetadataHandle {
    /// `None` завершает публикацию: новые обновления не отправляются.
    pub fn set(&self, metadata: Option<TrackMetadata>) -> Result<()> {
        if let Some(metadata) = &metadata {
            metadata.validate()?;
        }
        self.updates
            .send(metadata)
            .map_err(|_| anyhow!("receiver session ended"))
    }
}

/// Caller-supplied identity; no model impersonation or hardware-specific name is
/// hidden in transport defaults. Device ID must be a stable, unique adapter ID.
#[derive(Clone, Debug)]
pub struct Identity {
    pub device_id: String,
    pub name: String,
    pub model: String,
    pub source_version: String,
}

impl Identity {
    pub fn new(device_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            name: name.into(),
            model: "AirPlay".into(),
            source_version: "690.7.1".into(),
        }
    }
}

/// Внешняя политика задержки sender: допустимый диапазон, желаемая задержка и
/// начальный буфер. Значения в кадрах 44,1 кГц. Расчёт — не измеренная
/// акустическая задержка.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatencyPolicy {
    pub min_frames: u32,
    pub max_frames: u32,
    /// Пауза перед RECORD; данные до RECORD не отправляются и буфер не заполняется.
    pub pre_record_delay: Duration,
}

impl Default for LatencyPolicy {
    fn default() -> Self {
        Self {
            min_frames: 11_025,
            max_frames: 88_200,
            pre_record_delay: Duration::ZERO,
        }
    }
}

/// Итог согласования: задержка в кадрах внутри общего диапазона sender и
/// приёмника. `receiver_*` сохранены для отчёта, а не для «измерения».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NegotiatedLatency {
    pub frames: u32,
    pub receiver_min: u32,
    pub receiver_max: u32,
    pub pre_record_delay: Duration,
}

impl LatencyPolicy {
    fn validate(&self) -> Result<()> {
        if self.min_frames == 0 || self.min_frames > self.max_frames || self.max_frames > 88_200 {
            bail!("invalid sender latency range");
        }
        if self.pre_record_delay > frames_duration(self.max_frames as u64) {
            bail!("pre-RECORD delay exceeds latency range");
        }
        Ok(())
    }

    /// Пересечение диапазонов обязательно: иначе приёмник не сможет играть
    /// с заявленной задержкой, и отправку начинать нельзя.
    pub fn resolve(
        &self,
        desired_frames: u32,
        receiver_min: Option<u32>,
        receiver_max: Option<u32>,
    ) -> Result<NegotiatedLatency> {
        self.validate()?;
        let receiver_min = receiver_min.unwrap_or(11_025);
        let receiver_max = receiver_max.unwrap_or(88_200);
        if receiver_min > receiver_max {
            bail!("invalid receiver latency range");
        }
        let low = self.min_frames.max(receiver_min);
        let high = self.max_frames.min(receiver_max);
        if low > high {
            bail!("no common latency range with receiver");
        }
        let frames = desired_frames.clamp(low, high);
        let pre_record_delay = self.pre_record_delay.min(frames_duration(frames as u64));
        Ok(NegotiatedLatency {
            frames,
            receiver_min,
            receiver_max,
            pre_record_delay,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub endpoint: SocketAddr,
    pub identity: Identity,
    /// Желаемая задержка в кадрах 44,1 кГц; политика ограничивает её
    /// диапазоном, согласованным с приёмником.
    pub latency_frames: u32,
    /// Внешняя политика задержки: пределы и начальный буфер.
    pub latency_policy: LatencyPolicy,
    pub volume: Volume,
    /// Leave the receiver's gain untouched until the caller publishes a volume
    /// revision. The initial watch value is a placeholder in this mode.
    pub preserve_receiver_volume: bool,
    /// Credentials obtained by a previously completed PIN pairing. The adapter
    /// owns secure persistence; None keeps the transient3939 mode.
    pub pairing: Option<PairingCredentials>,
    /// Explicit PTP transport. Caller owns one qualified clock service/provider
    /// for the whole group and supplies tagged shared producer frames.
    pub ptp: Option<crate::tx::timing::PtpPlan>,
    pub request_timeout: Duration,
}

impl SessionConfig {
    pub fn new(endpoint: SocketAddr, identity: Identity) -> Self {
        Self {
            endpoint,
            identity,
            latency_frames: 22_050,
            latency_policy: LatencyPolicy::default(),
            volume: Volume(-30.0),
            preserve_receiver_volume: false,
            pairing: None,
            ptp: None,
            request_timeout: Duration::from_secs(8),
        }
    }

    fn validate(&self) -> Result<()> {
        if self
            .ptp
            .as_ref()
            .is_some_and(|plan| !plan.has_receiver(self.endpoint.ip()))
        {
            bail!("receiver is not in PTP peer configuration");
        }
        self.latency_policy.validate()?;
        if self.latency_frames == 0
            || !(self.latency_policy.min_frames..=self.latency_policy.max_frames)
                .contains(&self.latency_frames)
        {
            bail!("latency outside configured policy");
        }
        if self.request_timeout.is_zero() || self.request_timeout > Duration::from_secs(30) {
            bail!("request timeout must be positive and at most 30 seconds");
        }
        if self.endpoint.port() == 0 {
            bail!("receiver port must be nonzero");
        }
        if [
            &self.identity.device_id,
            &self.identity.name,
            &self.identity.model,
            &self.identity.source_version,
        ]
        .iter()
        .any(|v| v.is_empty() || v.len() > 256)
        {
            bail!("sender identity fields must contain 1..256 bytes");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct PairingCredentials {
    controller: PairIdentity,
    accessory_id: Vec<u8>,
    accessory_key: [u8; 32],
}

impl PairingCredentials {
    pub fn controller_id(&self) -> &str {
        &self.controller.pairing_id
    }
    pub fn controller_public_key(&self) -> [u8; 32] {
        self.controller.public_key()
    }
    pub fn accessory_id(&self) -> &[u8] {
        &self.accessory_id
    }
    pub fn accessory_public_key(&self) -> [u8; 32] {
        self.accessory_key
    }

    /// Controller identity with its signing key, needed by callers that run
    /// pair-verify themselves (the mirroring sender does).
    pub(crate) fn controller_identity(&self) -> &PairIdentity {
        &self.controller
    }
    pub fn new(
        controller: PairIdentity,
        accessory_id: Vec<u8>,
        accessory_key: [u8; 32],
    ) -> Result<Self> {
        if controller.pairing_id.is_empty()
            || controller.pairing_id.len() > 256
            || accessory_id.is_empty()
            || accessory_id.len() > 256
        {
            bail!("pairing identifiers must contain 1..256 bytes");
        }
        Ok(Self {
            controller,
            accessory_id,
            accessory_key,
        })
    }
}

impl std::fmt::Debug for PairingCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingCredentials { private }")
    }
}

/// A first-time PIN exchange, paused while the application obtains the displayed
/// code. No audio SETUP, playback or credential files are created. Dropping it
/// closes this connection; retry requires an explicit new begin().
pub struct PinPairing {
    rtsp: Rtsp,
    controller: PairIdentity,
    client_name: String,
    m2: Vec<u8>,
    deadline: Duration,
}

impl PinPairing {
    pub async fn begin(
        endpoint: SocketAddr,
        controller: PairIdentity,
        deadline: Duration,
    ) -> Result<Self> {
        Self::begin_with_client_name(endpoint, controller, deadline, "AirPlay").await
    }

    /// Begin PIN pairing with the caller's display name, sent in pairing headers.
    pub async fn begin_with_client_name(
        endpoint: SocketAddr,
        controller: PairIdentity,
        deadline: Duration,
        client_name: &str,
    ) -> Result<Self> {
        if endpoint.port() == 0
            || deadline.is_zero()
            || deadline > Duration::from_secs(30)
            || controller.pairing_id.is_empty()
            || controller.pairing_id.len() > 256
            || client_name.trim().is_empty()
            || client_name.len() > 256
            || client_name.chars().any(char::is_control)
        {
            bail!("invalid PIN pairing endpoint, identity or deadline");
        }
        let stream = timeout(deadline, TcpStream::connect(endpoint))
            .await
            .context("PIN connect deadline")??;
        stream.set_nodelay(true)?;
        let mut rtsp = Rtsp::new(
            Connection::new(stream),
            "rtsp://pairing/0".into(),
            "pairing".into(),
            0,
            deadline,
        );
        rtsp.pin_post("/pair-pin-start", Vec::new(), client_name)
            .await?;
        // M1 has no PIN-dependent values; proof generation waits for finish().
        let setup = ControllerSetup::new(controller.clone(), "", false);
        let m2 = rtsp
            .pin_post("/pair-setup", setup.m1(), client_name)
            .await?
            .body;
        let reply = pair::tlv8::Tlv8::decode(&m2)?;
        if reply.state()? != 2 || reply.get(pair::tlv8::ERROR).is_some() {
            bail!("receiver rejected PIN pairing start");
        }
        reply.require(pair::tlv8::SALT, "missing PIN setup salt")?;
        reply.require(pair::tlv8::PUBLIC_KEY, "missing PIN setup key")?;
        Ok(Self {
            rtsp,
            controller,
            client_name: client_name.to_owned(),
            m2,
            deadline,
        })
    }

    pub async fn finish(mut self, pin: &str) -> Result<PairingCredentials> {
        if pin.is_empty() || pin.len() > 64 {
            bail!("PIN must contain 1..64 bytes");
        }
        let setup = ControllerSetup::new(self.controller.clone(), pin.to_owned(), false);
        let (setup, m3) = timeout(self.deadline, setup_proof(setup, self.m2))
            .await
            .context("PIN proof deadline")??;
        let m4 = self
            .rtsp
            .pin_post("/pair-setup", m3, &self.client_name)
            .await?;
        if setup.check_m4(&m4.body)?.is_some() {
            bail!("unexpected transient PIN exchange");
        }
        let m6 = self
            .rtsp
            .pin_post("/pair-setup", setup.m5()?, &self.client_name)
            .await?;
        let accessory = setup.check_m6(&m6.body)?;
        PairingCredentials::new(self.controller, accessory.id, accessory.public_key)
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamStats {
    pub packets_sent: u64,
    pub retransmitted_packets: u64,
    pub volume_updates: u64,
}

/// Successful media transmission, not receiver playback/acoustic acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaProgress {
    pub packets_sent: u64,
    pub frame_index: u64,
    pub play_at: Instant,
}

// Tokio handles detach on drop; these protocol helpers must instead stop on all
// setup errors, caller cancellation and session destruction.
struct Task<T = ()>(JoinHandle<T>);
impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One receiver's negotiated, experimental transient realtime session.
/// `connect` confirms RTSP setup, NOT delivery/acoustic readiness. `run` consumes
/// paced packets and returns counters; it does not reconnect behind the caller.
pub struct Session {
    config: SessionConfig,
    session_id: u32,
    rtsp: Option<Rtsp>,
    audio: UdpSocket,
    control: Arc<UdpSocket>,
    control_destination: SocketAddr,
    packetizer: TargetPacketizer,
    _timing_task: Option<Task>,
    _event_task: Option<Task>,
    peers: Option<PtpPeerControl>,
    peer_changes: Option<PeerChanges>,
    metadata: watch::Sender<Option<TrackMetadata>>,
}

/// Requested peer state and the last RTSP-accepted update, not proof of native
/// group adoption. Bound to this connection's original group/domain and route.
#[derive(Clone)]
pub struct PtpPeerControl {
    plan: crate::tx::timing::PtpPlan,
    receiver: std::net::IpAddr,
    requested: watch::Sender<crate::tx::timing::PeerUpdate>,
    applied: watch::Receiver<crate::tx::timing::PeerUpdate>,
}
struct PeerChanges {
    requested: watch::Receiver<crate::tx::timing::PeerUpdate>,
    applied: watch::Sender<crate::tx::timing::PeerUpdate>,
}
impl PtpPeerControl {
    pub fn requested(&self) -> crate::tx::timing::PeerUpdate {
        self.requested.borrow().clone()
    }
    pub fn applied(&self) -> watch::Receiver<crate::tx::timing::PeerUpdate> {
        self.applied.clone()
    }
    pub(crate) fn prepare(
        &self,
        receivers: &[std::net::IpAddr],
    ) -> Result<crate::tx::timing::PeerUpdate> {
        if !receivers.contains(&self.receiver) {
            bail!("PTP peer update omits this receiver");
        }
        if self.requested.is_closed() {
            bail!("PTP session ended");
        }
        self.plan.peer_update(receivers)
    }
    pub(crate) fn publish(&self, update: crate::tx::timing::PeerUpdate) -> Result<()> {
        if self.requested.is_closed() {
            bail!("PTP session ended");
        }
        if *self.requested.borrow() == update {
            return Ok(());
        }
        self.requested
            .send(update)
            .map_err(|_| anyhow!("PTP session ended"))
    }
    pub fn set_peers(&self, receivers: &[std::net::IpAddr]) -> Result<()> {
        self.publish(self.prepare(receivers)?)
    }
}

enum TargetPacketizer {
    Ntp(rtp::PcmPacketizer),
    Ptp(crate::tx::timing::PtpPacketizer),
}
enum SyncPacket {
    Ntp([u8; 20]),
    Ptp([u8; 28]),
}
impl SyncPacket {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Ntp(bytes) => bytes,
            Self::Ptp(bytes) => bytes,
        }
    }
}
struct TargetPacket {
    audio: Vec<u8>,
    sync: SyncPacket,
    header: rtp::Header,
}
struct FlushRequest {
    sequence: u16,
    timestamp: u32,
    reply: oneshot::Sender<Result<()>>,
}
impl TargetPacketizer {
    fn clear_retransmit_cache(&mut self) {
        match self {
            Self::Ntp(packetizer) => packetizer.clear_retransmit_cache(),
            Self::Ptp(packetizer) => packetizer.clear_retransmit_cache(),
        }
    }
    fn restart_spurt(&mut self) {
        match self {
            Self::Ntp(packetizer) => packetizer.restart_spurt(),
            Self::Ptp(packetizer) => packetizer.restart_spurt(),
        }
    }
    fn initial_timestamp(&self) -> u32 {
        match self {
            Self::Ntp(p) => p.initial_timestamp(),
            Self::Ptp(p) => p.initial_timestamp(),
        }
    }
    fn retransmit(&self, bytes: &[u8]) -> Vec<Vec<u8>> {
        match self {
            Self::Ntp(p) => p.retransmit(bytes),
            Self::Ptp(p) => p.retransmit(bytes),
        }
    }
    fn packetize(&mut self, frame: &AudioPacket) -> Result<TargetPacket> {
        Ok(match self {
            Self::Ntp(p) => {
                let packet =
                    p.packetize(frame.pcm(), frame.frame_index(), frame.ntp_timestamp())?;
                TargetPacket {
                    audio: packet.audio,
                    sync: SyncPacket::Ntp(packet.sync),
                    header: packet.header,
                }
            }
            Self::Ptp(p) => {
                let packet = p.packetize(frame)?;
                TargetPacket {
                    audio: packet.audio,
                    sync: SyncPacket::Ptp(packet.sync),
                    header: packet.header,
                }
            }
        })
    }
}

impl Session {
    pub fn ptp_peer_control(&self) -> Option<PtpPeerControl> {
        self.peers.clone()
    }
    pub fn endpoint(&self) -> SocketAddr {
        self.config.endpoint
    }
    pub fn initial_volume(&self) -> Volume {
        self.config.volume
    }
    pub fn metadata_handle(&self) -> MetadataHandle {
        MetadataHandle {
            updates: self.metadata.clone(),
        }
    }
    pub(crate) fn shutdown_deadline(&self) -> Duration {
        self.config.request_timeout + Duration::from_secs(3)
    }
    pub async fn connect(mut config: SessionConfig) -> Result<Self> {
        config.validate()?;
        let stream = timeout(config.request_timeout, TcpStream::connect(config.endpoint))
            .await
            .context("receiver connect deadline exceeded")??;
        stream.set_nodelay(true)?;
        let local_ip = stream.local_addr()?.ip();
        if config
            .ptp
            .as_ref()
            .is_some_and(|plan| !plan.has_local(local_ip))
        {
            bail!("TCP route is outside configured PTP local addresses");
        }
        let (session_id, dacp, active_remote, uuid_tail, first_sequence, identity_seed) = {
            let mut rng = rand::thread_rng();
            let mut seed = [0; 32];
            rng.fill_bytes(&mut seed);
            (
                rng.next_u32(),
                format!("{:016X}", rng.next_u64()),
                rng.next_u32(),
                rng.next_u64() & 0xFFFF_FFFF_FFFF,
                rng.next_u32() as u16,
                seed,
            )
        };
        let uri_host = match local_ip {
            std::net::IpAddr::V4(ip) => ip.to_string(),
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        };
        let uri = format!("rtsp://{uri_host}/{session_id}");
        let mut rtsp = Rtsp::new(
            Connection::new(stream),
            uri,
            dacp,
            active_remote,
            config.request_timeout,
        );
        let (keys, shared) = if let Some(credentials) = &config.pairing {
            verify_pairing(&mut rtsp, credentials).await?
        } else {
            let pair = ControllerSetup::new(
                PairIdentity::from_seed(config.identity.device_id.clone(), &identity_seed),
                "3939",
                true,
            );
            rtsp.pair_post("/pair-pin-start", Vec::new()).await?;
            let m2 = rtsp.pair_post("/pair-setup", pair.m1()).await?;
            let (pair, m3) = timeout(config.request_timeout, setup_proof(pair, m2.body))
                .await
                .context("pairing proof deadline exceeded")??;
            let m4 = rtsp.pair_post("/pair-setup", m3).await?;
            // Unlike the source draft, verify the receiver's SRP proof before using
            // its control/session keys, using the shared core implementation.
            let keys = pair
                .check_m4(&m4.body)?
                .ok_or_else(|| anyhow!("transient pairing did not produce control keys"))?;
            let shared = pair
                .session_key()
                .ok_or_else(|| anyhow!("missing transient session key"))?
                .to_vec();
            (keys, shared)
        };
        rtsp.conn.encrypt(keys);

        let timing = if config.ptp.is_none() {
            Some(Arc::new(UdpSocket::bind((local_ip, 0)).await?))
        } else {
            None
        };
        let control = Arc::new(UdpSocket::bind((local_ip, 0)).await?);
        let timing_task = timing.as_ref().map(|timing| {
            Task(tokio::spawn(timing_server(
                timing.clone(),
                config.endpoint.ip(),
            )))
        });
        // A receiver may allocate session state at either SETUP. Even a later
        // decode/port/RECORD failure must release it before closing the socket.
        let prepared = async {
            let setup = if let Some(plan) = &config.ptp {
                let uuid = ((session_id as u128) << 96)
                    | (0x4000_u128 << 64)
                    | (0x8000_u128 << 48)
                    | (uuid_tail as u128);
                let mut setup = plan.setup(&config.identity, uuid)?;
                let fields = setup.as_dictionary_mut().context("PTP SETUP dictionary")?;
                fields.insert("isMultiSelectAirPlay".into(), plist::Value::Boolean(true));
                fields.insert("senderSupportsRelay".into(), plist::Value::Boolean(false));
                fields.insert(
                    "statsCollectionEnabled".into(),
                    plist::Value::Boolean(false),
                );
                setup
            } else {
                dictionary(vec![
                    (
                        "deviceID",
                        plist::Value::String(config.identity.device_id.clone()),
                    ),
                    (
                        "macAddress",
                        plist::Value::String(config.identity.device_id.clone()),
                    ),
                    ("name", plist::Value::String(config.identity.name.clone())),
                    ("model", plist::Value::String(config.identity.model.clone())),
                    (
                        "sourceVersion",
                        plist::Value::String(config.identity.source_version.clone()),
                    ),
                    (
                        "sessionUUID",
                        plist::Value::String(format!(
                            "{session_id:08X}-0000-4000-8000-{uuid_tail:012X}"
                        )),
                    ),
                    (
                        "timingPort",
                        plist::Value::Integer(
                            (timing
                                .as_ref()
                                .context("NTP timing socket")?
                                .local_addr()?
                                .port() as u64)
                                .into(),
                        ),
                    ),
                    ("timingProtocol", plist::Value::String("NTP".into())),
                    ("isMultiSelectAirPlay", plist::Value::Boolean(true)),
                    ("groupContainsGroupLeader", plist::Value::Boolean(false)),
                    ("senderSupportsRelay", plist::Value::Boolean(false)),
                    ("statsCollectionEnabled", plist::Value::Boolean(false)),
                ])
            };
            let response = rtsp
                .request(
                    "SETUP",
                    None,
                    binary_plist(setup)?,
                    Some("application/x-apple-binary-plist"),
                    &[],
                )
                .await?;
            let setup = plist::Value::from_reader(std::io::Cursor::new(response.body))?;
            let setup = setup
                .as_dictionary()
                .ok_or_else(|| anyhow!("session SETUP reply is not a dictionary"))?;
            if let Some(plan) = &config.ptp {
                if setup
                    .get("timingProtocol")
                    .is_some_and(|protocol| protocol.as_string() != Some("PTP"))
                {
                    bail!("receiver rejected PTP timing protocol");
                }
                rtsp.request(
                    "SETPEERS",
                    None,
                    binary_plist(plan.setpeers()?)?,
                    Some("/peer-list-changed"),
                    &[],
                )
                .await
                .context("PTP SETPEERS")?;
            }
            let event_port = optional_port(setup.get("eventPort"))?;
            let event_task = event_port.map(|port| {
                Task(tokio::spawn(event_channel(
                    SocketAddr::new(config.endpoint.ip(), port),
                    ControlKeys::controller_events(&shared),
                    config.request_timeout,
                )))
            });
            // Retain the original receiver-compatibility settling interval, without
            // claiming that it is an event-channel readiness acknowledgement.
            sleep(Duration::from_millis(100)).await;
            if event_task.as_ref().is_some_and(|task| task.0.is_finished()) {
                bail!("receiver event channel failed during startup");
            }

            // Preserve the original sender's Events-Write SHK. Direction is named
            // from the accessory side; controller_events().write is Events-Read.
            let shk = ControlKeys::accessory_events(&shared).write;
            let stream_setup = dictionary(vec![(
                "streams",
                plist::Value::Array(vec![dictionary(vec![
                    ("audioFormat", plist::Value::Integer(0x800u64.into())),
                    ("audioMode", plist::Value::String("default".into())),
                    (
                        "controlPort",
                        plist::Value::Integer((control.local_addr()?.port() as u64).into()),
                    ),
                    ("ct", plist::Value::Integer(1u64.into())),
                    ("isMedia", plist::Value::Boolean(true)),
                    (
                        "latencyMax",
                        plist::Value::Integer((config.latency_policy.max_frames as u64).into()),
                    ),
                    (
                        "latencyMin",
                        plist::Value::Integer((config.latency_policy.min_frames as u64).into()),
                    ),
                    ("shk", plist::Value::Data(shk.to_vec())),
                    (
                        "spf",
                        plist::Value::Integer((FRAMES_PER_PACKET as u64).into()),
                    ),
                    ("sr", plist::Value::Integer((SAMPLE_RATE as u64).into())),
                    ("type", plist::Value::Integer(96u64.into())),
                    ("supportsDynamicStreamID", plist::Value::Boolean(false)),
                    (
                        "streamConnectionID",
                        plist::Value::Integer((session_id as u64).into()),
                    ),
                ])]),
            )]);
            let response = rtsp
                .request(
                    "SETUP",
                    None,
                    binary_plist(stream_setup)?,
                    Some("application/x-apple-binary-plist"),
                    &[],
                )
                .await?;
            let setup = plist::Value::from_reader(std::io::Cursor::new(response.body))?;
            let first = setup
                .as_dictionary()
                .and_then(|d| d.get("streams"))
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .and_then(|v| v.as_dictionary())
                .ok_or_else(|| anyhow!("no stream in SETUP reply"))?;
            let data_port = optional_port(first.get("dataPort"))?
                .ok_or_else(|| anyhow!("no data port in SETUP reply"))?;
            let control_port = optional_port(first.get("controlPort"))?
                .ok_or_else(|| anyhow!("no control port in SETUP reply"))?;
            let receiver_min = receiver_render_frames(first.get("latencyMin"))?;
            let receiver_max = receiver_max_frames(first.get("latencyMax"))?;
            let negotiated = config.latency_policy.resolve(
                config.latency_frames,
                Some(receiver_min),
                Some(receiver_max),
            )?;
            let render_frames = receiver_min;
            config.latency_frames = negotiated.frames;
            let packetizer = if let Some(plan) = &config.ptp {
                TargetPacketizer::Ptp(crate::tx::timing::PtpPacketizer::new(
                    plan.clone(),
                    shk,
                    session_id,
                    first_sequence,
                    config.latency_frames,
                    render_frames,
                )?)
            } else {
                TargetPacketizer::Ntp(rtp::PcmPacketizer::with_render_latency(
                    shk,
                    session_id,
                    first_sequence,
                    config.latency_frames,
                    render_frames,
                )?)
            };
            if !negotiated.pre_record_delay.is_zero() {
                sleep(negotiated.pre_record_delay).await;
            }
            rtsp.request("RECORD", None, Vec::new(), None, &[]).await?;
            rtsp.request(
                "FLUSH",
                None,
                Vec::new(),
                None,
                &[
                    ("Range", "npt=0-".into()),
                    ("Session", session_id.to_string()),
                    (
                        "RTP-Info",
                        format!(
                            "seq={first_sequence};rtptime={}",
                            packetizer.initial_timestamp()
                        ),
                    ),
                ],
            )
            .await?;
            if !config.preserve_receiver_volume {
                apply_volume(&mut rtsp, config.volume).await?;
            }
            let audio = UdpSocket::bind((local_ip, 0)).await?;
            audio
                .connect(SocketAddr::new(config.endpoint.ip(), data_port))
                .await?;
            let control_destination = SocketAddr::new(config.endpoint.ip(), control_port);
            Ok::<_, anyhow::Error>((audio, control_destination, packetizer, event_task))
        }
        .await;
        let (audio, control_destination, packetizer, event_task) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = timeout(
                    Duration::from_secs(2),
                    rtsp.request("TEARDOWN", None, Vec::new(), None, &[]),
                )
                .await;
                return Err(error);
            }
        };
        let (peers, peer_changes) = if let Some(plan) = &config.ptp {
            let initial = plan.initial_peer_update();
            let (requested, changes) = watch::channel(initial.clone());
            let (applied, accepted) = watch::channel(initial);
            (
                Some(PtpPeerControl {
                    plan: plan.clone(),
                    receiver: config.endpoint.ip(),
                    requested,
                    applied: accepted,
                }),
                Some(PeerChanges {
                    requested: changes,
                    applied,
                }),
            )
        } else {
            (None, None)
        };
        let (metadata, _) = watch::channel(None);
        Ok(Self {
            config,
            session_id,
            rtsp: Some(rtsp),
            audio,
            control,
            control_destination,
            packetizer,
            _timing_task: timing_task,
            _event_task: event_task,
            peers,
            peer_changes,
            metadata,
        })
    }

    pub fn latency(&self) -> Duration {
        frames_duration(self.config.latency_frames as u64)
    }

    /// `audio` may be a bounded broadcast subscription shared with other sessions.
    /// Close its producer for EOF; `stop=true` or dropping the stop sender stops
    /// the session. EOF drains the last packet's presentation deadline before
    /// TEARDOWN, while stop interrupts the drain. Per-target volume uses watch's latest-value coalescing, with
    /// no file polling, slider callback or platform-specific gain policy here.
    pub async fn run(
        self,
        audio: broadcast::Receiver<Arc<AudioPacket>>,
        volume: watch::Receiver<Volume>,
        stop: watch::Receiver<bool>,
    ) -> Result<StreamStats> {
        self.run_inner(audio, volume, stop, None).await
    }

    /// Coalesced observation channel. It remains None until a UDP audio send succeeds.
    pub async fn run_with_progress(
        self,
        audio: broadcast::Receiver<Arc<AudioPacket>>,
        volume: watch::Receiver<Volume>,
        stop: watch::Receiver<bool>,
        progress: watch::Sender<Option<MediaProgress>>,
    ) -> Result<StreamStats> {
        self.run_inner(audio, volume, stop, Some(progress)).await
    }

    async fn run_inner(
        mut self,
        mut audio: broadcast::Receiver<Arc<AudioPacket>>,
        volume: watch::Receiver<Volume>,
        mut stop: watch::Receiver<bool>,
        progress: Option<watch::Sender<Option<MediaProgress>>>,
    ) -> Result<StreamStats> {
        let rtsp = self
            .rtsp
            .take()
            .context("missing receiver control connection")?;
        let (control_stop, stopping) = watch::channel(false);
        let (flush_tx, flush_rx) = mpsc::channel(1);
        let mut owner = Task(tokio::spawn(control_owner(
            rtsp,
            volume,
            stopping,
            if self.config.preserve_receiver_volume {
                None
            } else {
                Some(self.config.volume)
            },
            self.peer_changes.take(),
            self.metadata.subscribe(),
            flush_rx,
            self.session_id,
        )));
        let result = tokio::select! {
            result = self.stream(&mut audio, &mut stop, progress.as_ref(), flush_tx) => result,
            result = &mut owner.0 => {
                result.context("receiver control task failed")??;
                bail!("receiver control task ended before media");
            }
        };
        control_stop.send_replace(true);
        let control = match timeout(
            self.config.request_timeout + Duration::from_secs(2),
            &mut owner.0,
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err(anyhow!("receiver control shutdown deadline exceeded")),
        };
        match (result, control) {
            (Err(error), _) => Err(error),
            (Ok(mut stats), Ok(updates)) => {
                stats.volume_updates = updates;
                Ok(stats)
            }
            (Ok(_), Err(error)) => Err(error),
        }
    }

    async fn stream(
        &mut self,
        audio: &mut broadcast::Receiver<Arc<AudioPacket>>,
        stop: &mut watch::Receiver<bool>,
        progress: Option<&watch::Sender<Option<MediaProgress>>>,
        flush: mpsc::Sender<FlushRequest>,
    ) -> Result<StreamStats> {
        let mut stats = StreamStats::default();
        let mut last_sync = Instant::now() - Duration::from_secs(2);
        let mut control_buf = [0u8; 64];
        let mut last_presentation: Option<Instant> = None;
        let mut last_source: Option<Arc<AudioPacket>> = None;
        let mut drain_until: Option<Instant> = None;
        loop {
            if *stop.borrow() {
                break;
            }
            if self
                ._timing_task
                .as_ref()
                .is_some_and(|task| task.0.is_finished())
                || self
                    ._event_task
                    .as_ref()
                    .is_some_and(|task| task.0.is_finished())
            {
                bail!("receiver timing or event channel stopped");
            }
            let drain = drain_until;
            tokio::select! {
                chunk = audio.recv(), if drain_until.is_none() => {
                    let chunk = match chunk {
                        Ok(packet) => packet,
                        Err(broadcast::error::RecvError::Lagged(n)) => bail!("PCM producer overran the bounded session queue by {n} packets"),
                        Err(broadcast::error::RecvError::Closed) => {
                            if let Some(deadline) = last_presentation { drain_until = Some(deadline); continue; }
                            break;
                        }
                    };
                    if !chunk.source_is_current() { continue; }
                    if chunk.play_at(self.latency()).is_none_or(|deadline| deadline <= Instant::now()) {
                        bail!("PCM packet expired before transmission");
                    }
                    // A future timestamp cannot be used to hold control handling
                    // hostage indefinitely. The producer must pace its media input.
                    if chunk.produced > Instant::now() + Duration::from_millis(100) { bail!("PCM producer is not paced to its media clock"); }
                    tokio::select! {
                        _ = sleep_until(chunk.produced.into()) => {}
                        _ = wait_stop(stop) => break,
                    }
                    if !chunk.source_is_current() { continue; }
                    let discontinuity = last_source.as_ref().is_some_and(|previous| !previous.same_source_revision(&chunk));
                    if discontinuity {
                        self.packetizer.clear_retransmit_cache();
                        self.packetizer.restart_spurt();
                    }
                    let packet = self.packetizer.packetize(&chunk)?;
                    if discontinuity {
                        let (reply, accepted) = oneshot::channel();
                        flush.send(FlushRequest { sequence: packet.header.seq, timestamp: packet.header.timestamp, reply }).await.context("receiver control stopped before FLUSH")?;
                        tokio::select! {
                            result = accepted => result.context("receiver control dropped FLUSH")??,
                            _ = wait_stop(stop) => break,
                        }
                        if !chunk.source_is_current() { continue; }
                        if chunk.play_at(self.latency()).is_none_or(|deadline| deadline <= Instant::now()) {
                            bail!("PCM packet expired while waiting for FLUSH");
                        }
                        last_sync = Instant::now() - Duration::from_secs(2);
                    }
                    if last_sync.elapsed() >= Duration::from_secs(1) {
                        self.control.send_to(packet.sync.bytes(), self.control_destination).await?;
                        last_sync = Instant::now();
                    }
                    // A failed datagram is a transport failure, not proof of a
                    // playable receiver. The caller decides whether to reconnect.
                    self.audio.send(&packet.audio).await?;
                    last_source = Some(chunk.clone());
                    stats.packets_sent += 1;
                    if let Some(progress)=progress {
                        progress.send_replace(Some(MediaProgress{packets_sent:stats.packets_sent,frame_index:chunk.index,play_at:chunk.play_at(self.latency()).context("presentation overflow")?}));
                    }
                    last_presentation = chunk.play_at(self.latency()).and_then(|t| t.checked_add(frames_duration(FRAMES_PER_PACKET as u64) + Duration::from_millis(50)));
                }
                got = self.control.recv_from(&mut control_buf) => {
                    let (size, from) = got?;
                    // Do not become an unauthenticated UDP reflector. Retransmit
                    // only to the exact control endpoint negotiated with the peer.
                    if from != self.control_destination { continue; }
                    if last_source.as_ref().is_some_and(|previous| !previous.source_is_current()) {
                        self.packetizer.clear_retransmit_cache();
                        continue;
                    }
                    for retry in self.packetizer.retransmit(&control_buf[..size]) {
                        self.control.send_to(&retry, from).await?;
                        stats.retransmitted_packets += 1;
                    }
                }
                _ = async {
                    if let Some(deadline) = drain { sleep_until(deadline.into()).await; }
                    else { std::future::pending::<()>().await; }
                } => break,
                _ = wait_stop(stop) => break,
            }
        }
        Ok(stats)
    }
}

// Own the control stream in one task. A slow peer cannot stall UDP audio or
// retransmissions. Never reuse a connection after an incomplete failed request.
async fn control_owner(
    mut rtsp: Rtsp,
    mut volume: watch::Receiver<Volume>,
    mut stop: watch::Receiver<bool>,
    mut current: Option<Volume>,
    mut peers: Option<PeerChanges>,
    mut metadata: watch::Receiver<Option<TrackMetadata>>,
    mut flush: mpsc::Receiver<FlushRequest>,
    session_id: u32,
) -> Result<u64> {
    let mut updates = 0;
    let mut requested = false;
    let mut volume_open = true;
    let mut peers_open = peers.is_some();
    let mut metadata_open = true;
    let mut feedback = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(2),
        Duration::from_secs(2),
    );
    feedback.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut volume_tick = tokio::time::interval(Duration::from_millis(100));
    volume_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        if *stop.borrow() {
            break;
        }
        tokio::select! {
            Some(request) = flush.recv() => {
                let result = rtsp.request("FLUSH", None, Vec::new(), None, &[
                    ("Range", "npt=0-".into()),
                    ("Session", session_id.to_string()),
                    ("RTP-Info", format!("seq={};rtptime={}", request.sequence, request.timestamp)),
                ]).await.map(|_| ());
                match result {
                    Ok(()) => { let _ = request.reply.send(Ok(())); }
                    Err(error) => { let message = format!("FLUSH failed: {error:#}"); let _ = request.reply.send(Err(anyhow!(message.clone()))); bail!(message); }
                }
            }
            changed=async {match peers.as_mut() {Some(peers)=>peers.requested.changed().await,None=>std::future::pending().await}}, if peers_open => {
                match changed {
                    Ok(())=>{
                        let peers=peers.as_mut().context("PTP update channel")?;
                        let requested=peers.requested.borrow_and_update().clone();
                        let applied=peers.applied.borrow().clone();
                        if requested!=applied {
                            rtsp.request("SETPEERS",None,binary_plist(requested.body()?)?,Some("/peer-list-changed"),&[]).await.context("PTP peer update rejected")?;
                            peers.applied.send_replace(requested);
                        }
                    }
                    Err(_)=>peers_open=false,
                }
            }
            _ = volume_tick.tick() => {
                // Preserve mode needs a published revision, including an explicit
                // request equal to the placeholder. Never consume a queued user
                // update merely to establish a baseline at run() entry.
                if current.is_none() && !requested {
                    continue;
                }
                let desired = *volume.borrow_and_update();
                requested = false;
                if Some(desired) != current {
                    apply_volume(&mut rtsp, desired).await?;
                    current = Some(desired);
                    updates += 1;
                }
            }
            changed = volume.changed(), if volume_open => {
                match changed {
                    Ok(()) => requested = true,
                    Err(_) => volume_open = false,
                }
            }
            changed = metadata.changed(), if metadata_open => {
                match changed {
                    Ok(()) => {
                        let track = metadata.borrow_and_update().clone();
                        if let Some(track) = track {
                            publish_metadata(&mut rtsp, &track).await?;
                        }
                    }
                    Err(_) => metadata_open = false,
                }
            }
            _ = feedback.tick() => {
                rtsp.request("POST", Some("/feedback"), Vec::new(), None, &[]).await?;
            }
            _ = wait_stop(&mut stop) => break,
        }
    }
    timeout(
        Duration::from_secs(2),
        rtsp.request("TEARDOWN", None, Vec::new(), None, &[]),
    )
    .await
    .context("receiver TEARDOWN deadline exceeded")??;
    Ok(updates)
}

/// DMAP-тело `mlit` с непустыми полями трека. Неизвестные приёмнику теги
/// игнорируются, поэтому дополнительные поля не обязательны.
fn dmap_metadata(track: &TrackMetadata) -> Vec<u8> {
    let mut items = Vec::new();
    for (tag, value) in [
        ("minm", &track.title),
        ("asar", &track.artist),
        ("asal", &track.album),
    ] {
        if let Some(value) = value {
            items.extend_from_slice(tag.as_bytes());
            items.extend_from_slice(&(value.len() as u32).to_be_bytes());
            items.extend_from_slice(value.as_bytes());
        }
    }
    let mut body = Vec::with_capacity(items.len() + 8);
    body.extend_from_slice(b"mlit");
    body.extend_from_slice(&(items.len() as u32).to_be_bytes());
    body.extend_from_slice(&items);
    body
}

fn progress_text(track: &TrackMetadata) -> Option<String> {
    track
        .position_frames
        .zip(track.duration_frames)
        .map(|(position, duration)| format!("progress: 0/{position}/{duration}\r\n"))
}

/// Запросы одного обновления: DMAP, обложка и progress. Вынесено из сокета,
/// чтобы протокольные тела можно было проверить без приёмника.
fn metadata_requests(track: &TrackMetadata) -> Vec<(Option<&'static str>, Vec<u8>)> {
    let mut requests = Vec::new();
    if track.title.is_some() || track.artist.is_some() || track.album.is_some() {
        requests.push((Some("application/x-dmap-tagged"), dmap_metadata(track)));
    }
    match &track.artwork {
        Artwork::Unchanged => {}
        Artwork::Set { mime, data } => requests.push((
            Some(if mime == "image/png" {
                "image/png"
            } else {
                "image/jpeg"
            }),
            data.clone(),
        )),
        // Пустой JPEG-запрос — явная очистка обложки, а не «нет изменений».
        Artwork::Clear => requests.push((Some("image/jpeg"), Vec::new())),
    }
    if let Some(progress) = progress_text(track) {
        requests.push((Some("text/parameters"), progress.into_bytes()));
    }
    requests
}

async fn publish_metadata(rtsp: &mut Rtsp, track: &TrackMetadata) -> Result<()> {
    for (content_type, body) in metadata_requests(track) {
        rtsp.request("SET_PARAMETER", None, body, content_type, &[])
            .await?;
    }
    Ok(())
}

fn dictionary(items: Vec<(&str, plist::Value)>) -> plist::Value {
    plist::Value::Dictionary(items.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

async fn wait_stop(stop: &mut watch::Receiver<bool>) {
    loop {
        if *stop.borrow_and_update() {
            return;
        }
        if stop.changed().await.is_err() {
            return;
        }
    }
}

fn binary_plist(value: plist::Value) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    value.to_writer_binary(&mut bytes)?;
    Ok(bytes)
}

pub(crate) async fn verify_pairing(
    rtsp: &mut Rtsp,
    credentials: &PairingCredentials,
) -> Result<(ControlKeys, Vec<u8>)> {
    let mut verify = ControllerVerify::new(
        credentials.controller.clone(),
        Accessory {
            id: credentials.accessory_id.clone(),
            public_key: credentials.accessory_key,
        },
    );
    let m2 = rtsp.verify_post(verify.m1()).await?;
    let m3 = verify.m3(&m2.body)?;
    let m4 = rtsp.verify_post(m3).await?;
    let keys = verify.check_m4(&m4.body)?;
    let shared = verify
        .session_key()
        .context("missing pair-verify shared secret")?
        .to_vec();
    Ok((keys, shared))
}

pub(crate) async fn setup_proof(
    mut pair: ControllerSetup,
    m2: Vec<u8>,
) -> Result<(ControllerSetup, Vec<u8>)> {
    // 3072-bit SRP exponentiation must not stall media/control tasks on the
    // current-thread runtime. Cancellation closes the socket; the finite CPU
    // operation may finish in the blocking pool without owning that connection.
    tokio::task::spawn_blocking(move || {
        let m3 = pair.m3(&m2)?;
        Ok((pair, m3))
    })
    .await
    .context("pairing proof task failed")?
}

// Fallback is the minimum explicitly requested in this session's SETUP, not a
// measured receiver delay. Same correction as Station cf1894c (audibly verified).
fn receiver_render_frames(value: Option<&plist::Value>) -> Result<u32> {
    match value {
        None => Ok(11_025),
        Some(value) => value
            .as_unsigned_integer()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| anyhow!("invalid receiver latencyMin")),
    }
}

fn receiver_max_frames(value: Option<&plist::Value>) -> Result<u32> {
    match value {
        None => Ok(88_200),
        Some(value) => value
            .as_unsigned_integer()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| anyhow!("invalid receiver latencyMax")),
    }
}

fn optional_port(value: Option<&plist::Value>) -> Result<Option<u16>> {
    match value {
        None => Ok(None),
        Some(value) => match value.as_unsigned_integer() {
            Some(0) => Ok(None),
            Some(port) if port <= u16::MAX as u64 => Ok(Some(port as u16)),
            _ => bail!("invalid receiver UDP/TCP port"),
        },
    }
}

async fn apply_volume(rtsp: &mut Rtsp, volume: Volume) -> Result<()> {
    rtsp.request(
        "SET_PARAMETER",
        None,
        format!("volume: {:.6}\r\n", volume.as_db()).into_bytes(),
        Some("text/parameters"),
        &[],
    )
    .await?;
    Ok(())
}

async fn event_channel(endpoint: SocketAddr, keys: ControlKeys, deadline: Duration) {
    let Ok(Ok(stream)) = timeout(deadline, TcpStream::connect(endpoint)).await else {
        return;
    };
    let mut conn = Connection::new(stream);
    conn.encrypt(keys);
    while let Ok(request) = conn.event_request().await {
        let mut response = rtsp::Response::to(&request, 200);
        response.headers.insert("Audio-Latency", "0");
        let mut bytes = Vec::new();
        response.encode(&mut bytes);
        if !matches!(timeout(deadline, conn.send(&bytes)).await, Ok(Ok(()))) {
            break;
        }
    }
}

async fn timing_server(socket: Arc<UdpSocket>, peer: std::net::IpAddr) {
    let mut buf = [0u8; 128];
    while let Ok((size, from)) = socket.recv_from(&mut buf).await {
        if from.ip() != peer {
            continue;
        }
        if let Some(response) = timesync::ntp::reply(&buf[..size]) {
            let _ = socket.send_to(&response, from).await;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ptp_receiver_outside_declared_peers_is_rejected_before_connecting() {
        use super::*;
        let plan = crate::tx::timing::PtpPlan::new(
            coordinator::group::GroupContext::new(
                123,
                coordinator::group::ClockDomain::Ptp(7),
                true,
            )
            .unwrap(),
            clock::domain::Domain::new(7, 1).unwrap(),
            456,
            vec!["127.0.0.1".parse().unwrap()],
            vec!["192.0.2.1".parse().unwrap()],
        )
        .unwrap();
        let mut config = SessionConfig::new(
            "127.0.0.1:9".parse().unwrap(),
            Identity::new("sender", "test"),
        );
        config.ptp = Some(plan);
        assert!(config.validate().is_err());
    }
    #[tokio::test]
    async fn first_pin_pairing_returns_verified_keys_or_rejects_wrong_pin() {
        use pair::{
            PairingStore,
            hap::{PairSetup, SetupOutcome},
        };
        for pin in ["1234", "9999"] {
            let expected_name = if pin == "1234" {
                "Test Sender"
            } else {
                "AirPlay"
            };
            let controller = PairIdentity::from_seed("controller", &[11; 32]);
            let accessory = PairIdentity::from_seed("receiver", &[12; 32]);
            let expected_key = accessory.public_key();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = listener.local_addr().unwrap();
            let peer = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut connection = Connection::new(socket);
                let mut setup = PairSetup::new(accessory, "1234");
                let store = pair::MemoryStore::default();
                for step in 0..4 {
                    let request = connection.event_request().await.unwrap();
                    assert_eq!(request.method, "POST");
                    assert_eq!(request.headers.get("X-Apple-HKP"), Some("3"));
                    assert_eq!(
                        request.headers.get("X-Apple-Client-Name"),
                        Some(expected_name)
                    );
                    let mut response = rtsp::Response::to(&request, 200);
                    let mut ended = false;
                    if step == 0 {
                        assert_eq!(request.uri, "/pair-pin-start");
                    } else {
                        assert_eq!(request.uri, "/pair-setup");
                        response.body = match setup.handle(&request.body, &store).unwrap() {
                            SetupOutcome::Continue(reply) => reply,
                            SetupOutcome::Paired { reply, .. } | SetupOutcome::Rejected(reply) => {
                                ended = true;
                                reply
                            }
                            _ => panic!("unexpected transient setup"),
                        };
                    }
                    let mut bytes = Vec::new();
                    response.encode(&mut bytes);
                    connection.send(&bytes).await.unwrap();
                    if ended {
                        break;
                    }
                }
                store.len()
            });
            let waiting = if pin == "1234" {
                PinPairing::begin_with_client_name(
                    endpoint,
                    controller,
                    Duration::from_secs(3),
                    expected_name,
                )
                .await
            } else {
                PinPairing::begin(endpoint, controller, Duration::from_secs(3)).await
            }
            .unwrap();
            let result = waiting.finish(pin).await;
            let registered = peer.await.unwrap();
            if pin == "1234" {
                let credentials = result.unwrap();
                assert_eq!(credentials.accessory_id(), b"receiver");
                assert_eq!(credentials.accessory_public_key(), expected_key);
                assert_eq!(registered, 1);
            } else {
                assert!(result.is_err());
                assert_eq!(registered, 0);
            }
        }
    }

    #[tokio::test]
    async fn invalid_pin_pairing_parameters_fail_before_connecting() {
        let controller = PairIdentity::from_seed("controller", &[1; 32]);
        for name in ["", " ", "sender\r\nInjected: yes"] {
            assert!(
                PinPairing::begin_with_client_name(
                    "127.0.0.1:7000".parse().unwrap(),
                    controller.clone(),
                    Duration::from_secs(1),
                    name
                )
                .await
                .is_err()
            );
        }
        assert!(
            PinPairing::begin(
                "127.0.0.1:0".parse().unwrap(),
                controller.clone(),
                Duration::from_secs(1)
            )
            .await
            .is_err()
        );
        assert!(
            PinPairing::begin(
                "127.0.0.1:7000".parse().unwrap(),
                controller,
                Duration::ZERO
            )
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn stored_pairing_verifies_without_pin_or_new_pair_setup() {
        use pair::{
            PairingStore,
            hap::{PairVerify, VerifyOutcome},
        };
        let controller = PairIdentity::from_seed("controller", &[7; 32]);
        let accessory = PairIdentity::from_seed("receiver", &[8; 32]);
        let credentials = PairingCredentials::new(
            controller.clone(),
            b"receiver".to_vec(),
            accessory.public_key(),
        )
        .unwrap();
        assert_eq!(format!("{credentials:?}"), "PairingCredentials { private }");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let peer = tokio::spawn(async move {
            let store = pair::MemoryStore::default();
            store.add(controller.pairing_id.as_bytes(), controller.public_key());
            let mut verifier = PairVerify::new(accessory);
            let mut connection = Connection::new(server);
            for step in 0..2 {
                let request = connection.event_request().await.unwrap();
                assert_eq!(request.uri, "/pair-verify");
                assert_eq!(request.headers.get("X-Apple-HKP"), Some("3"));
                let outcome = verifier.handle(&request.body, &store).unwrap();
                let (body, keys) = match outcome {
                    VerifyOutcome::Continue(reply) => {
                        assert_eq!(step, 0);
                        (reply, None)
                    }
                    VerifyOutcome::Verified { reply, keys, .. } => {
                        assert_eq!(step, 1);
                        (reply, Some(keys))
                    }
                    _ => panic!("verification rejected"),
                };
                let mut response = rtsp::Response::to(&request, 200);
                response.body = body;
                let mut bytes = Vec::new();
                response.encode(&mut bytes);
                connection.send(&bytes).await.unwrap();
                if let Some(keys) = keys {
                    return keys;
                }
            }
            panic!("missing verified keys")
        });
        let mut rtsp = Rtsp::new(
            Connection::new(client),
            "rtsp://test/1".into(),
            "test".into(),
            1,
            Duration::from_secs(1),
        );
        let (keys, shared) = verify_pairing(&mut rtsp, &credentials).await.unwrap();
        let receiver = peer.await.unwrap();
        assert_eq!(keys.write, receiver.read);
        assert_eq!(keys.read, receiver.write);
        assert_eq!(shared.len(), 32);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn offloaded_setup_keeps_proof_state_for_receiver_verification() {
        use pair::{
            MemoryStore,
            hap::{PairSetup, SetupOutcome},
        };
        let mut receiver = PairSetup::new(PairIdentity::from_seed("receiver", &[3; 32]), "0000");
        let sender =
            ControllerSetup::new(PairIdentity::from_seed("sender", &[4; 32]), "3939", true);
        let store = MemoryStore::default();
        let SetupOutcome::Continue(m2) = receiver.handle(&sender.m1(), &store).unwrap() else {
            panic!("expected M2")
        };
        let (sender, m3) = setup_proof(sender, m2).await.unwrap();
        let SetupOutcome::Transient { reply, keys, .. } = receiver.handle(&m3, &store).unwrap()
        else {
            panic!("expected M4")
        };
        let controller = sender.check_m4(&reply).unwrap().unwrap();
        assert_eq!(controller.write, keys.read);
        assert_eq!(controller.read, keys.write);
    }
    #[tokio::test]
    async fn preserve_volume_ignores_placeholder_but_applies_explicit_same_value() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (events, mut received) = tokio::sync::mpsc::channel(4);
        let peer = tokio::spawn(async move {
            let mut connection = Connection::new(server);
            loop {
                let request = connection.event_request().await.unwrap();
                let teardown = request.method == "TEARDOWN";
                events.send(request.method.clone()).await.unwrap();
                let mut bytes = Vec::new();
                rtsp::Response::to(&request, 200).encode(&mut bytes);
                connection.send(&bytes).await.unwrap();
                if teardown {
                    break;
                }
            }
        });
        let rtsp = Rtsp::new(
            Connection::new(client),
            "rtsp://test/1".into(),
            "test".into(),
            1,
            Duration::from_secs(1),
        );
        let placeholder = Volume::db(-30.0).unwrap();
        let (volume, changes) = watch::channel(placeholder);
        let (stop, stopped) = watch::channel(false);
        let (_metadata, no_metadata) = watch::channel(None);
        let (_flush, flush_rx) = mpsc::channel(1);
        let owner = tokio::spawn(control_owner(
            rtsp,
            changes,
            stopped,
            None,
            None,
            no_metadata,
            flush_rx,
            1,
        ));
        assert!(
            timeout(Duration::from_millis(230), received.recv())
                .await
                .is_err()
        );
        volume.send_replace(placeholder);
        // The last published request must survive the writer closing too.
        drop(volume);
        assert_eq!(
            timeout(Duration::from_secs(1), received.recv())
                .await
                .unwrap()
                .unwrap(),
            "SET_PARAMETER"
        );
        stop.send_replace(true);
        assert_eq!(
            timeout(Duration::from_secs(1), owner)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            1
        );
        assert_eq!(received.recv().await.unwrap(), "TEARDOWN");
        peer.await.unwrap();
    }
    #[test]
    fn negotiated_render_min_is_validated_and_absence_uses_request() {
        assert_eq!(super::receiver_render_frames(None).unwrap(), 11_025);
        assert_eq!(
            super::receiver_render_frames(Some(&8_820u64.into())).unwrap(),
            8_820
        );
        assert_eq!(
            super::receiver_render_frames(Some(&0u64.into())).unwrap(),
            0
        );
        assert!(super::receiver_render_frames(Some(&"invalid".into())).is_err());
        assert!(super::receiver_render_frames(Some(&u64::MAX.into())).is_err());
    }
    use super::*;

    #[tokio::test]
    async fn slow_rtsp_does_not_hold_the_real_udp_media_loop() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let mut conn = Connection::new(server);
            let mut entered = Some(entered);
            loop {
                let request = conn.event_request().await.unwrap();
                if request.method == "SET_PARAMETER" {
                    entered.take().unwrap().send(()).unwrap();
                    sleep(Duration::from_millis(400)).await;
                }
                let mut bytes = Vec::new();
                rtsp::Response::to(&request, 200).encode(&mut bytes);
                conn.send(&bytes).await.unwrap();
                if request.method == "TEARDOWN" {
                    break;
                }
            }
        });
        let receiving = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let audio = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        audio
            .connect(receiving.local_addr().unwrap())
            .await
            .unwrap();
        let control_peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = SessionConfig::new(
            listener.local_addr().unwrap(),
            Identity::new("test", "test"),
        );
        let (metadata, _keep_metadata) = watch::channel(None);
        let session = Session {
            metadata,
            session_id: 1,
            rtsp: Some(Rtsp::new(
                Connection::new(client),
                "rtsp://test/1".into(),
                "test".into(),
                1,
                Duration::from_secs(1),
            )),
            packetizer: TargetPacketizer::Ntp(rtp::PcmPacketizer::new(
                [7; 32],
                1,
                7,
                config.latency_frames,
            )),
            config,
            audio,
            control: Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap()),
            control_destination: control_peer.local_addr().unwrap(),
            _timing_task: Some(Task(tokio::spawn(std::future::pending()))),
            _event_task: None,
            peers: None,
            peer_changes: None,
        };
        let (pcm, packets) = broadcast::channel(32);
        let (volume, volumes) = watch::channel(Volume::db(-30.0).unwrap());
        let (_stop, stopped) = watch::channel(false);
        volume.send_replace(Volume::db(-25.0).unwrap());
        let (progress, observed) = watch::channel(None);
        let running = tokio::spawn(session.run_with_progress(packets, volumes, stopped, progress));
        timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
        let clock = crate::tx::MediaClock::now();
        assert!(observed.borrow().is_none());
        for index in 0..20u64 {
            let packet = Arc::new(
                clock
                    .packet(
                        index * FRAMES_PER_PACKET as u64,
                        &[0; crate::tx::PACKET_BYTES],
                    )
                    .unwrap(),
            );
            sleep_until(packet.produced_at().into()).await;
            pcm.send(packet).unwrap();
            let mut bytes = [0; 2048];
            let n = timeout(Duration::from_millis(100), receiving.recv(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            assert!(n > 12);
            assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 7 + index as u16);
        }
        drop(pcm);
        let stats = timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(stats.packets_sent, 20);
        assert_eq!(stats.volume_updates, 1);
        let progress = observed.borrow().unwrap();
        assert_eq!(progress.packets_sent, 20);
        assert_eq!(progress.frame_index, 19 * FRAMES_PER_PACKET as u64);
        assert_eq!(
            progress.play_at,
            clock
                .packet(progress.frame_index, &[0; crate::tx::PACKET_BYTES])
                .unwrap()
                .play_at(Duration::from_millis(500))
                .unwrap()
        );
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn source_change_flushes_before_sending_marked_packet() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let (noticed, observed_flush) = oneshot::channel();
        let (release, proceed) = oneshot::channel();
        let peer = tokio::spawn(async move {
            let mut conn = Connection::new(server);
            let mut noticed = Some(noticed);
            let mut proceed = Some(proceed);
            loop {
                let request = conn.event_request().await.unwrap();
                if request.method == "FLUSH" {
                    noticed
                        .take()
                        .unwrap()
                        .send(request.headers.get("RTP-Info").unwrap().to_owned())
                        .unwrap();
                    proceed.take().unwrap().await.unwrap();
                }
                let done = request.method == "TEARDOWN";
                let mut bytes = Vec::new();
                rtsp::Response::to(&request, 200).encode(&mut bytes);
                conn.send(&bytes).await.unwrap();
                if done {
                    break;
                }
            }
        });
        let receiving = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let audio = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        audio
            .connect(receiving.local_addr().unwrap())
            .await
            .unwrap();
        let control_peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let config = SessionConfig::new(
            listener.local_addr().unwrap(),
            Identity::new("test", "test"),
        );
        let (metadata, _keep_metadata) = watch::channel(None);
        let session = Session {
            session_id: 1,
            metadata,
            rtsp: Some(Rtsp::new(
                Connection::new(client),
                "rtsp://test/1".into(),
                "test".into(),
                1,
                Duration::from_secs(1),
            )),
            packetizer: TargetPacketizer::Ntp(rtp::PcmPacketizer::new(
                [7; 32],
                1,
                7,
                config.latency_frames,
            )),
            config,
            audio,
            control: Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap()),
            control_destination: control_peer.local_addr().unwrap(),
            _timing_task: None,
            _event_task: None,
            peers: None,
            peer_changes: None,
        };
        let (pcm, packets) = broadcast::channel(8);
        let (_volume, volumes) = watch::channel(Volume::db(-30.0).unwrap());
        let (stop, stopped) = watch::channel(false);
        let running = tokio::spawn(session.run(packets, volumes, stopped));
        let (source, revision) = watch::channel(crate::tx::continuous_source::State::default());
        let clock = crate::tx::MediaClock::now();
        pcm.send(Arc::new(
            clock
                .packet(0, &[0; crate::tx::PACKET_BYTES])
                .unwrap()
                .guard_source(revision),
        ))
        .unwrap();
        let mut bytes = [0; 2048];
        timeout(Duration::from_secs(1), receiving.recv(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        source.send_replace(crate::tx::continuous_source::State {
            generation: 1,
            paused: false,
        });
        let next = clock
            .packet(FRAMES_PER_PACKET as u64, &[1; crate::tx::PACKET_BYTES])
            .unwrap()
            .guard_source(source.subscribe());
        pcm.send(Arc::new(next)).unwrap();
        let info = timeout(Duration::from_secs(1), observed_flush)
            .await
            .unwrap()
            .unwrap();
        assert!(info.starts_with("seq=8;rtptime="));
        assert!(
            timeout(Duration::from_millis(30), receiving.recv(&mut bytes))
                .await
                .is_err()
        );
        release.send(()).unwrap();
        let size = timeout(Duration::from_secs(1), receiving.recv(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert!(size > 12);
        assert_eq!(bytes[1] & 0x80, 0x80);
        assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 8);
        let timestamp = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        assert_eq!(info, format!("seq=8;rtptime={timestamp}"));
        stop.send_replace(true);
        running.await.unwrap().unwrap();
        peer.await.unwrap();
    }

    #[test]
    fn volume_is_validated_before_entering_control_channel() {
        for value in [f32::NAN, f32::INFINITY, -31.0, 1.0] {
            assert!(Volume::db(value).is_err());
        }
        assert_eq!(Volume::normalized(0.0).unwrap(), Volume::MUTE);
        assert_eq!(Volume::normalized(0.5).unwrap().as_db(), -15.0);
        assert_eq!(Volume::normalized(1.0).unwrap().as_db(), 0.0);
    }

    #[test]
    fn peer_port_cannot_wrap_to_another_endpoint() {
        assert!(optional_port(Some(&plist::Value::Integer(65_536u64.into()))).is_err());
        assert_eq!(
            optional_port(Some(&plist::Value::Integer(7000u64.into()))).unwrap(),
            Some(7000)
        );
    }

    #[test]
    fn setup_configuration_rejects_invalid_limits_before_connecting() {
        let mut config = SessionConfig::new(
            "127.0.0.1:7000".parse().unwrap(),
            Identity::new("sender", "test"),
        );
        assert!(config.validate().is_ok());
        config.latency_frames = 0;
        assert!(config.validate().is_err());
        config.latency_frames = 22_050;
        config.request_timeout = Duration::ZERO;
        assert!(config.validate().is_err());
    }

    #[tokio::test]
    async fn false_stop_updates_do_not_cancel_the_session() {
        let (sender, mut stop) = watch::channel(false);
        sender.send(false).unwrap();
        assert!(
            timeout(Duration::from_millis(10), wait_stop(&mut stop))
                .await
                .is_err()
        );
        sender.send(true).unwrap();
        timeout(Duration::from_secs(1), wait_stop(&mut stop))
            .await
            .unwrap();
    }
}

#[cfg(test)]
#[path = "session_ptp_tests.rs"]
mod ptp_tests;

#[cfg(test)]
mod latency_tests {
    use super::*;

    #[test]
    fn desired_latency_is_clamped_to_receiver_range() {
        let policy = LatencyPolicy::default();
        let zero = policy.resolve(22_050, Some(0), Some(120_000)).unwrap();
        assert_eq!(zero.frames, 22_050);
        let fast = policy.resolve(22_050, Some(5_000), Some(11_025)).unwrap();
        assert_eq!(fast.frames, 11_025);
        let slow = policy.resolve(22_050, Some(44_100), Some(88_200)).unwrap();
        assert_eq!(slow.frames, 44_100);
        assert_eq!((slow.receiver_min, slow.receiver_max), (44_100, 88_200));
        let exact = policy.resolve(33_075, Some(11_025), Some(88_200)).unwrap();
        assert_eq!(exact.frames, 33_075);
    }

    #[test]
    fn unusable_or_contradictory_receiver_ranges_are_rejected() {
        let policy = LatencyPolicy::default();
        assert!(policy.resolve(22_050, Some(90_000), Some(100_000)).is_err());
        assert!(policy.resolve(22_050, Some(20_000), Some(10_000)).is_err());
        let narrow = LatencyPolicy {
            min_frames: 30_000,
            max_frames: 40_000,
            ..Default::default()
        };
        assert!(narrow.resolve(35_000, Some(11_025), Some(22_050)).is_err());
        assert!(narrow.resolve(35_000, Some(35_000), Some(40_000)).is_ok());
    }

    #[test]
    fn pre_record_delay_is_bounded_by_negotiated_latency() {
        let policy = LatencyPolicy {
            min_frames: 11_025,
            max_frames: 88_200,
            pre_record_delay: Duration::from_millis(1_500),
        };
        let negotiated = policy.resolve(11_025, Some(11_025), Some(88_200)).unwrap();
        assert!(negotiated.pre_record_delay <= frames_duration(11_025));
        assert_eq!(negotiated.pre_record_delay, frames_duration(11_025));
    }

    #[test]
    fn invalid_policy_is_rejected_before_connecting() {
        let policy = LatencyPolicy {
            min_frames: 40_000,
            max_frames: 20_000,
            ..Default::default()
        };
        assert!(policy.validate().is_err());
        let mut config =
            SessionConfig::new("127.0.0.1:7000".parse().unwrap(), Identity::new("s", "s"));
        config.latency_policy = policy;
        assert!(config.validate().is_err());
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;

    fn track() -> TrackMetadata {
        TrackMetadata {
            title: Some("Песня".into()),
            artist: Some("Артист".into()),
            album: Some("Альбом".into()),
            ..Default::default()
        }
    }

    fn tag(body: &[u8], tag: &[u8; 4]) -> Option<Vec<u8>> {
        let mut at = 8; // mlit + длина
        while at + 8 <= body.len() {
            let name = &body[at..at + 4];
            let len = u32::from_be_bytes(body[at + 4..at + 8].try_into().unwrap()) as usize;
            let value = body.get(at + 8..at + 8 + len)?.to_vec();
            if name == tag {
                return Some(value);
            }
            at += 8 + len;
        }
        None
    }

    #[test]
    fn dmap_carries_title_artist_and_album() {
        let requests = metadata_requests(&track());
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, Some("application/x-dmap-tagged"));
        let body = &requests[0].1;
        assert_eq!(&body[..4], b"mlit");
        assert_eq!(tag(body, b"minm").as_deref(), Some("Песня".as_bytes()));
        assert_eq!(tag(body, b"asar").as_deref(), Some("Артист".as_bytes()));
        assert_eq!(tag(body, b"asal").as_deref(), Some("Альбом".as_bytes()));
    }

    #[test]
    fn artwork_sets_and_clears_but_unchanged_is_silent() {
        let mut update = track();
        update.title = None;
        update.artist = None;
        update.album = None;
        update.artwork = Artwork::Set {
            mime: "image/png".into(),
            data: vec![1, 2, 3],
        };
        let requests = metadata_requests(&update);
        assert_eq!(requests, [(Some("image/png"), vec![1, 2, 3])]);
        update.artwork = Artwork::Clear;
        assert_eq!(
            metadata_requests(&update),
            [(Some("image/jpeg"), Vec::new())]
        );
        update.artwork = Artwork::Unchanged;
        assert!(metadata_requests(&update).is_empty());
    }

    #[test]
    fn progress_uses_airplay_frame_triple() {
        let mut update = track();
        update.position_frames = Some(88_200);
        update.duration_frames = Some(441_000);
        let requests = metadata_requests(&update);
        assert_eq!(requests.last().unwrap().1, b"progress: 0/88200/441000\r\n");
    }

    #[test]
    fn invalid_metadata_is_rejected_before_sending() {
        let (tx, _rx) = watch::channel(None);
        let handle = MetadataHandle { updates: tx };
        let mut bad = track();
        bad.artwork = Artwork::Set {
            mime: "image/gif".into(),
            data: vec![1],
        };
        assert!(handle.set(Some(bad)).is_err());
        let mut bad = track();
        bad.position_frames = Some(10);
        bad.duration_frames = Some(5);
        assert!(handle.set(Some(bad)).is_err());
        let mut bad = track();
        bad.artwork = Artwork::Set {
            mime: "image/jpeg".into(),
            data: vec![0; MAX_ARTWORK_BYTES + 1],
        };
        assert!(handle.set(Some(bad)).is_err());
        assert!(handle.set(Some(track())).is_ok());
    }
}
