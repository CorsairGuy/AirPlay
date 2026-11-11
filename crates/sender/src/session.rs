use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use crypto::kdf::ControlKeys;
use pair::{Identity as PairIdentity, hap::ControllerSetup};
use rand::RngCore;
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::{broadcast, watch},
    task::JoinHandle,
    time::{sleep, sleep_until, timeout},
};

use crate::{
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

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub endpoint: SocketAddr,
    pub identity: Identity,
    /// Receiver latency in 44.1kHz frames (250ms..2s for this stream format).
    pub latency_frames: u32,
    pub volume: Volume,
    /// Leave the receiver's gain untouched until the caller publishes a volume
    /// revision. The initial watch value is a placeholder in this mode.
    pub preserve_receiver_volume: bool,
    pub request_timeout: Duration,
}

impl SessionConfig {
    pub fn new(endpoint: SocketAddr, identity: Identity) -> Self {
        Self {
            endpoint,
            identity,
            latency_frames: 22_050,
            volume: Volume(-30.0),
            preserve_receiver_volume: false,
            request_timeout: Duration::from_secs(8),
        }
    }

    fn validate(&self) -> Result<()> {
        if !(11_025..=88_200).contains(&self.latency_frames) {
            bail!("latency outside realtime stream limits");
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
    rtsp: Option<Rtsp>,
    audio: UdpSocket,
    control: Arc<UdpSocket>,
    control_destination: SocketAddr,
    packetizer: rtp::PcmPacketizer,
    _timing_task: Task,
    _event_task: Option<Task>,
}

impl Session {
    pub async fn connect(config: SessionConfig) -> Result<Self> {
        config.validate()?;
        let stream = timeout(config.request_timeout, TcpStream::connect(config.endpoint))
            .await
            .context("receiver connect deadline exceeded")??;
        stream.set_nodelay(true)?;
        let local_ip = stream.local_addr()?.ip();
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
        let mut pair = ControllerSetup::new(
            PairIdentity::from_seed(config.identity.device_id.clone(), &identity_seed),
            "3939",
            true,
        );
        rtsp.pair_post("/pair-pin-start", Vec::new()).await?;
        let m2 = rtsp.pair_post("/pair-setup", pair.m1()).await?;
        let m3 = pair.m3(&m2.body)?;
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
        rtsp.conn.encrypt(keys);

        let timing = Arc::new(UdpSocket::bind((local_ip, 0)).await?);
        let control = Arc::new(UdpSocket::bind((local_ip, 0)).await?);
        let timing_task = Task(tokio::spawn(timing_server(
            timing.clone(),
            config.endpoint.ip(),
        )));
        // A receiver may allocate session state at either SETUP. Even a later
        // decode/port/RECORD failure must release it before closing the socket.
        let prepared = async {
            let setup = dictionary(vec![
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
                    plist::Value::Integer((timing.local_addr()?.port() as u64).into()),
                ),
                ("timingProtocol", plist::Value::String("NTP".into())),
                ("isMultiSelectAirPlay", plist::Value::Boolean(true)),
                ("groupContainsGroupLeader", plist::Value::Boolean(false)),
                ("senderSupportsRelay", plist::Value::Boolean(false)),
                ("statsCollectionEnabled", plist::Value::Boolean(false)),
            ]);
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
                    ("latencyMax", plist::Value::Integer(88_200u64.into())),
                    ("latencyMin", plist::Value::Integer(11_025u64.into())),
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
            let render_frames = receiver_render_frames(first.get("latencyMin"))?;
            let packetizer = rtp::PcmPacketizer::with_render_latency(
                shk,
                session_id,
                first_sequence,
                config.latency_frames,
                render_frames,
            )?;
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
        Ok(Self {
            config,
            rtsp: Some(rtsp),
            audio,
            control,
            control_destination,
            packetizer,
            _timing_task: timing_task,
            _event_task: event_task,
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
        let mut owner = Task(tokio::spawn(control_owner(
            rtsp,
            volume,
            stopping,
            if self.config.preserve_receiver_volume {
                None
            } else {
                Some(self.config.volume)
            },
        )));
        let result = tokio::select! {
            result = self.stream(&mut audio, &mut stop, progress.as_ref()) => result,
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
    ) -> Result<StreamStats> {
        let mut stats = StreamStats::default();
        let mut last_sync = Instant::now() - Duration::from_secs(2);
        let mut control_buf = [0u8; 64];
        let mut last_presentation: Option<Instant> = None;
        let mut drain_until: Option<Instant> = None;
        loop {
            if *stop.borrow() {
                break;
            }
            if self._timing_task.0.is_finished()
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
                    let packet = self.packetizer.packetize(&chunk.data, chunk.index, chunk.ntp)?;
                    if last_sync.elapsed() >= Duration::from_secs(1) {
                        self.control.send_to(&packet.sync, self.control_destination).await?;
                        last_sync = Instant::now();
                    }
                    // A failed datagram is a transport failure, not proof of a
                    // playable receiver. The caller decides whether to reconnect.
                    self.audio.send(&packet.audio).await?;
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
) -> Result<u64> {
    let mut updates = 0;
    let mut requested = false;
    let mut volume_open = true;
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
        let owner = tokio::spawn(control_owner(rtsp, changes, stopped, None));
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
        let session = Session {
            rtsp: Some(Rtsp::new(
                Connection::new(client),
                "rtsp://test/1".into(),
                "test".into(),
                1,
                Duration::from_secs(1),
            )),
            packetizer: rtp::PcmPacketizer::new([7; 32], 1, 7, config.latency_frames),
            config,
            audio,
            control: Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap()),
            control_destination: control_peer.local_addr().unwrap(),
            _timing_task: Task(tokio::spawn(std::future::pending())),
            _event_task: None,
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
        let clock = crate::MediaClock::now();
        assert!(observed.borrow().is_none());
        for index in 0..20u64 {
            let packet = Arc::new(
                clock
                    .packet(index * FRAMES_PER_PACKET as u64, &[0; crate::PACKET_BYTES])
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
                .packet(progress.frame_index, &[0; crate::PACKET_BYTES])
                .unwrap()
                .play_at(Duration::from_millis(500))
                .unwrap()
        );
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
