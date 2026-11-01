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
    pub request_timeout: Duration,
}

impl SessionConfig {
    pub fn new(endpoint: SocketAddr, identity: Identity) -> Self {
        Self {
            endpoint,
            identity,
            latency_frames: 22_050,
            volume: Volume(-30.0),
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

// Tokio handles detach on drop; these protocol helpers must instead stop on all
// setup errors, caller cancellation and session destruction.
struct Task(JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// One receiver's negotiated, experimental transient realtime session.
/// `connect` confirms RTSP setup, NOT delivery/acoustic readiness. `run` consumes
/// paced packets and returns counters; it does not reconnect behind the caller.
pub struct Session {
    config: SessionConfig,
    rtsp: Rtsp,
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
            let packetizer =
                rtp::PcmPacketizer::new(shk, session_id, first_sequence, config.latency_frames);
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
            apply_volume(&mut rtsp, config.volume).await?;
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
            rtsp,
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
        mut self,
        mut audio: broadcast::Receiver<Arc<AudioPacket>>,
        mut volume: watch::Receiver<Volume>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<StreamStats> {
        let result = self.stream(&mut audio, &mut volume, &mut stop).await;
        let teardown = timeout(
            Duration::from_secs(2),
            self.rtsp.request("TEARDOWN", None, Vec::new(), None, &[]),
        )
        .await;
        match (result, teardown) {
            (Err(error), _) => Err(error),
            (Ok(stats), Ok(Ok(_))) => Ok(stats),
            (Ok(_), Ok(Err(error))) => Err(error.context("receiver TEARDOWN failed")),
            (Ok(_), Err(_)) => Err(anyhow!("receiver TEARDOWN deadline exceeded")),
        }
    }

    async fn stream(
        &mut self,
        audio: &mut broadcast::Receiver<Arc<AudioPacket>>,
        volume: &mut watch::Receiver<Volume>,
        stop: &mut watch::Receiver<bool>,
    ) -> Result<StreamStats> {
        let mut stats = StreamStats::default();
        let mut last_sync = Instant::now() - Duration::from_secs(2);
        let mut feedback_tick = tokio::time::interval(Duration::from_secs(2));
        let mut current_volume = self.config.volume;
        let mut volume_tick = tokio::time::interval(Duration::from_millis(100));
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
                _ = volume_tick.tick() => {
                    let desired = *volume.borrow_and_update();
                    if desired != current_volume {
                        apply_volume(&mut self.rtsp, desired).await?;
                        current_volume = desired;
                        stats.volume_updates += 1;
                    }
                }
                _ = feedback_tick.tick() => {
                    self.rtsp.request("POST", Some("/feedback"), Vec::new(), None, &[]).await?;
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
    use super::*;

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
