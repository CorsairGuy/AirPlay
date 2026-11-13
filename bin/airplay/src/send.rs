//! Realtime PCM sender frontend. One bounded producer and media epoch feed all
//! connected receivers; this does not claim AP2 group/PTP synchronization.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use std::{future::Future, pin::Pin};

use anyhow::{Context, Result, anyhow, bail};
use sender::{AudioPacket, Identity, MediaClock, Session, SessionConfig, Volume};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinSet;

const RATE: u64 = sender::SAMPLE_RATE as u64;
const FRAMES: usize = sender::FRAMES_PER_PACKET;
const PCM_BYTES: usize = sender::PACKET_BYTES;
const QUEUE_PACKETS: usize = 64;

#[derive(Debug)]
pub struct Options {
    pub targets: Vec<String>,
    pub volume_db: f32,
    pub latency_ms: u32,
    pub local: Option<crate::local_process::Options>,
    pub control_socket: Option<std::path::PathBuf>,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self {
            targets: Vec::new(),
            volume_db: -29.7,
            latency_ms: 500,
            local: None,
            control_socket: None,
        };
        let mut local_program = None;
        let mut local_args = Vec::new();
        let mut local_gain = 1;
        let mut local_gate = None;
        let mut args = args.iter();
        while let Some(argument) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| anyhow!("{argument}: нужно значение"))?;
            match argument.as_str() {
                "--to" => {
                    endpoint(value)?;
                    options.targets.push(value.clone());
                }
                "--volume" => options.volume_db = value.parse().context("--volume: нужны дБ")?,
                "--latency-ms" => {
                    options.latency_ms =
                        value.parse().context("--latency-ms: нужны миллисекунды")?
                }
                "--local-program" => {
                    if local_program.is_some() {
                        bail!("--local-program задан повторно");
                    }
                    local_program = Some(std::path::PathBuf::from(value));
                }
                "--local-arg" => local_args.push(value.clone()),
                "--local-gain" => {
                    local_gain = value.parse().context("--local-gain: нужно целое 1..4")?
                }
                "--local-gate" => local_gate = Some(std::path::PathBuf::from(value)),
                "--control-socket" => options.control_socket = Some(value.into()),
                other => bail!("неизвестный параметр {other}"),
            }
        }
        options.local = match local_program {
            Some(program) => Some(crate::local_process::Options {
                program,
                args: local_args,
                gain: local_gain,
                gate: local_gate,
            }),
            None => {
                if !local_args.is_empty() || local_gain != 1 || local_gate.is_some() {
                    bail!("локальные параметры требуют --local-program");
                }
                None
            }
        };
        options.validate()?;
        Ok(options)
    }

    fn validate(&self) -> Result<()> {
        if self.targets.is_empty() || self.targets.len() > 8 {
            bail!("укажите от 1 до 8 --to HOST[:PORT]");
        }
        Volume::db(self.volume_db)?;
        if !(250..=2_000).contains(&self.latency_ms) {
            bail!("--latency-ms: допустимо 250..2000 мс");
        }
        if let Some(local) = &self.local {
            local.validate()?;
        }
        if let Some(path) = &self.control_socket {
            if !cfg!(unix) || !path.is_absolute() {
                bail!("control socket requires an absolute Unix path")
            }
        }
        Ok(())
    }
}

fn endpoint(value: &str) -> Result<(String, u16)> {
    if let Ok(address) = value.parse::<SocketAddr>() {
        if address.port() == 0 {
            bail!("нулевой порт недопустим");
        }
        return Ok((address.ip().to_string(), address.port()));
    }
    if let Ok(address) = value.parse::<IpAddr>() {
        return Ok((address.to_string(), 7000));
    }
    if let Some(address) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        return Ok((address.parse::<IpAddr>()?.to_string(), 7000));
    }
    let (host, port) = match value.split_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().context("неверный порт --to")?),
        None => (value, 7000),
    };
    if host.is_empty() || host.contains([':', '/', ' ', '\\']) || port == 0 {
        bail!("неверный --to HOST[:PORT]: {value}");
    }
    Ok((host.to_owned(), port))
}

/// stdin is S16BE, stereo, 44.1 kHz. A final complete stereo frame is padded
/// with silence to the packet boundary; a truncated sample/frame is an error.
async fn produce<R: AsyncRead + Unpin>(
    mut input: R,
    output: broadcast::Sender<Arc<AudioPacket>>,
) -> Result<u64> {
    let clock = MediaClock::now();
    let mut frame_index = 0_u64;
    loop {
        let mut pcm = [0_u8; PCM_BYTES];
        let mut used = 0;
        while used < pcm.len() {
            let count = input
                .read(&mut pcm[used..])
                .await
                .context("чтение PCM stdin")?;
            if count == 0 {
                break;
            }
            used += count;
        }
        if used == 0 {
            return Ok(frame_index);
        }
        if used % 4 != 0 {
            bail!("stdin закончился посреди S16BE стереокадра");
        }
        let packet = Arc::new(clock.packet(frame_index, &pcm)?);
        tokio::time::sleep_until(packet.produced_at().into()).await;
        output
            .send(packet)
            .map_err(|_| anyhow!("нет активных получателей PCM"))?;
        frame_index += FRAMES as u64;
        if used < PCM_BYTES {
            return Ok(frame_index);
        }
    }
}

/// All receivers connect before a single media clock starts. Any peer failure
/// stops this run: reconnect/group restoration is deliberately not implicit.
pub async fn run<R: AsyncRead + Unpin>(
    mut options: Options,
    input: R,
    stop: watch::Receiver<bool>,
) -> Result<()> {
    options.validate()?;
    if *stop.borrow() {
        return Ok(());
    }
    if let Some(path) = options.control_socket.take() {
        #[cfg(unix)]
        return crate::send_ipc::run(options, input, stop, path).await;
        #[cfg(not(unix))]
        {
            let _ = path;
            bail!("control socket unsupported")
        }
    }
    run_inner(options, input, stop, None, None).await
}

/// Library entry point for a platform controller with independent target volume
/// and per-target successful-media observation. The CLI remains static-volume.
pub async fn run_controlled<R: AsyncRead + Unpin>(
    options: Options,
    input: R,
    stop: watch::Receiver<bool>,
    control: crate::send_control::RelayControl,
) -> Result<()> {
    run_inner(options, input, stop, None, Some(control)).await
}

/// Resolve UI logical rows before the existing controlled sender opens any
/// connection. Discovery/catalog ownership stays with the embedding application.
pub async fn run_selected<R: AsyncRead + Unpin>(
    mut options: Options,
    catalog: &sender::outputs::OutputCatalog,
    selected: &[String],
    input: R,
    stop: watch::Receiver<bool>,
    control: crate::send_control::RelayControl,
) -> Result<()> {
    if !options.targets.is_empty() {
        bail!("logical routes and explicit targets cannot be combined");
    }
    options.targets = catalog
        .receivers(selected)?
        .into_iter()
        .map(|receiver| receiver.address.to_string())
        .collect();
    run_controlled(options, input, stop, control).await
}

/// Optional platform renderer, fed from the same clock and immutable packet queue.
/// The adapter owns its output latency, gain, startup gate and resource cleanup.
/// No route is changed by this API; platform handoff requires separate validation.
pub async fn run_with_local<R: AsyncRead + Unpin, S: sender::LocalSink>(
    options: Options,
    input: R,
    stop: watch::Receiver<bool>,
    sink: S,
) -> Result<()> {
    if options.local.is_some() {
        bail!("local process and supplied LocalSink are mutually exclusive");
    }
    run_inner(options, input, stop, Some(local_factory(sink)), None).await
}

pub async fn run_with_local_controlled<R: AsyncRead + Unpin, S: sender::LocalSink>(
    options: Options,
    input: R,
    stop: watch::Receiver<bool>,
    sink: S,
    control: crate::send_control::RelayControl,
) -> Result<()> {
    if options.local.is_some() {
        bail!("local process and supplied LocalSink are mutually exclusive");
    }
    run_inner(
        options,
        input,
        stop,
        Some(local_factory(sink)),
        Some(control),
    )
    .await
}

fn local_factory<S: sender::LocalSink>(sink: S) -> LocalFactory {
    Box::new(move |packets, latency, stop| {
        Box::pin(async move {
            sender::run_local(sink, packets, latency, stop).await?;
            Ok(sender::StreamStats::default())
        })
    })
}

type LocalFuture = Pin<Box<dyn Future<Output = Result<sender::StreamStats>> + Send>>;
type LocalFactory = Box<
    dyn FnOnce(
            broadcast::Receiver<Arc<AudioPacket>>,
            Duration,
            watch::Receiver<bool>,
        ) -> LocalFuture
        + Send,
>;

async fn run_inner<R: AsyncRead + Unpin>(
    mut options: Options,
    input: R,
    mut stop: watch::Receiver<bool>,
    local: Option<LocalFactory>,
    control: Option<crate::send_control::RelayControl>,
) -> Result<()> {
    options.validate()?;
    if options.control_socket.is_some() {
        bail!("explicit RelayControl and CLI control socket cannot be combined")
    }
    if *stop.borrow() {
        return Ok(());
    }
    let _control_guard = control
        .as_ref()
        .map(|v| v.claim(options.targets.len()))
        .transpose()?;
    let local = if let Some(process) = options.local.take() {
        Some(local_factory(crate::local_process::Renderer::spawn(
            process,
        )?))
    } else {
        local
    };
    let volume = Volume::db(options.volume_db)?;
    let identity = Identity::new(
        format!(
            "02:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            rand::random::<u8>(),
            rand::random::<u8>(),
            rand::random::<u8>(),
            rand::random::<u8>(),
            rand::random::<u8>()
        ),
        "AirPlay sender",
    );
    // A renderer may block while its device consumes lead-in silence. Bound memory,
    // but allow the configured latency before declaring a local subscriber overrun.
    let capacity = if local.is_some() {
        ((u64::from(options.latency_ms) * RATE).div_ceil(1000 * FRAMES as u64) + 32) as usize
    } else {
        QUEUE_PACKETS
    };
    let (audio, receiver) = broadcast::channel(capacity);
    let mut sessions = sender::pool::SessionPool::new(receiver);
    let (halt, halted) = watch::channel(false);
    let mut workers = JoinSet::new();
    let mut addresses = HashSet::new();
    for target in &options.targets {
        let (host, port) = endpoint(target)?;
        let resolved = tokio::select! {
            resolved = tokio::net::lookup_host((host.as_str(), port)) => resolved?,
            _ = stop.changed() => return Ok(()),
        };
        let address = resolved
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("не найден адрес {target}"))?;
        if !addresses.insert(address) {
            bail!("получатель указан повторно: {target}");
        }
        let mut config = SessionConfig::new(address, identity.clone());
        config.volume = volume;
        config.latency_frames = (u64::from(options.latency_ms) * RATE / 1000) as u32;
        let session = tokio::select! {
            session = Session::connect(config) => session.with_context(|| format!("подключение {target}"))?,
            _ = stop.changed() => return Ok(()),
        };
        let handle = sessions.add(&address.to_string(), session)?;
        if let Some(control) = &control {
            control.register_session(address, handle.clone());
        }
        let mut state = handle.state.clone();
        workers.spawn(async move {
            loop {
                let current = *state.borrow_and_update();
                match current {
                    sender::pool::SessionState::Running => {
                        state.changed().await.context("receiver task closed")?
                    }
                    sender::pool::SessionState::Ended => return Ok(sender::StreamStats::default()),
                    sender::pool::SessionState::Failed => bail!("receiver session failed"),
                }
            }
        });
    }
    if let Some(start) = local {
        workers.spawn(start(
            audio.subscribe(),
            Duration::from_millis(u64::from(options.latency_ms)),
            halted.clone(),
        ));
    }
    let result: Result<bool> = tokio::select! {
        produced = produce(input, audio) => produced.map(|_| true),
        finished = workers.join_next() => Err(match finished {
            Some(Ok(Err(error))) => error,
            Some(Err(error)) => error.into(),
            _ => anyhow!("получатель остановился до EOF"),
        }),
        _ = stop.changed() => Ok(false),
    };
    let graceful_eof = matches!(result, Ok(true));
    if !graceful_eof {
        if let Some(control) = &control {
            control.invalidate();
        }
        halt.send_replace(true);
    }
    let mut outcome = result.map(|_| ());
    let drain = async {
        let finished = if graceful_eof {
            sessions.drain().await
        } else {
            sessions.shutdown().await
        };
        for (_, result) in finished {
            if let Err(error) = result {
                halt.send_replace(true);
                if outcome.is_ok() {
                    outcome = Err(error);
                }
            }
        }
        while let Some(finished) = workers.join_next().await {
            match finished {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    halt.send_replace(true);
                    if outcome.is_ok() {
                        outcome = Err(error);
                    }
                }
                Err(error) => {
                    halt.send_replace(true);
                    if outcome.is_ok() {
                        outcome = Err(error.into());
                    }
                }
            }
        }
    };
    if tokio::time::timeout(
        Duration::from_millis(u64::from(options.latency_ms)) + Duration::from_secs(14),
        drain,
    )
    .await
    .is_err()
    {
        workers.abort_all();
        bail!("таймаут завершения отправителей");
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct SampleCounts {
        samples: usize,
        exact_pairs: usize,
        left: Option<i16>,
    }
    impl SampleCounts {
        fn record(&mut self, samples: &[i16]) {
            self.samples += samples.len();
            for &sample in samples {
                if let Some(left) = self.left.take() {
                    self.exact_pairs += usize::from([left, sample] == [0x1234, -0x2345]);
                } else {
                    self.left = Some(sample);
                }
            }
        }
    }
    #[test]
    fn capture_counts_content_after_long_silence_and_split_stereo_writes() {
        let mut counts = SampleCounts::default();
        counts.record(&vec![0; 100_000]);
        counts.record(&[0x1234]);
        counts.record(&[-0x2345, 0x1234]);
        counts.record(&[-0x2345]);
        assert_eq!(counts.samples, 100_004);
        assert_eq!(counts.exact_pairs, 2);
        assert!(counts.left.is_none());
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn validates_targets_volume_and_latency_before_connecting() {
        assert!(Options::parse(&args(&["--to", "localhost", "--to", "[::1]:7123"])).is_ok());
        for values in [
            vec![],
            vec!["--to"],
            vec!["--to", "x:0"],
            vec!["--to", "x", "--volume", "NaN"],
            vec!["--to", "x", "--latency-ms", "249"],
            vec!["--to", "x", "--latency-ms", "2001"],
            vec!["--to", "x", "--local-arg", "null"],
            vec!["--to", "x", "--local-program", "relative"],
            vec![
                "--to",
                "x",
                "--local-program",
                "/bin/cat",
                "--local-gain",
                "5",
            ],
        ] {
            assert!(Options::parse(&args(&values)).is_err(), "{values:?}");
        }
    }

    #[tokio::test]
    async fn pcm_tail_is_frame_aligned_and_zero_padded() {
        let (tx, mut rx) = broadcast::channel(2);
        assert_eq!(
            produce(std::io::Cursor::new(vec![0x12, 0x34, 0xED, 0xCC]), tx)
                .await
                .unwrap(),
            352
        );
        let packet = rx.recv().await.unwrap();
        assert_eq!(&packet.pcm()[..4], &[0x12, 0x34, 0xED, 0xCC]);
        assert!(packet.pcm()[4..].iter().all(|b| *b == 0));
        let (tx, _rx) = broadcast::channel(2);
        assert!(
            produce(std::io::Cursor::new(vec![0, 0, 0]), tx)
                .await
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sender_to_receiver_delivers_big_endian_stereo_samples() {
        loopback(false, false).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn local_and_remote_outputs_share_pcm_and_presentation_deadlines() {
        loopback(true, false).await;
    }

    #[tokio::test]
    async fn selected_logical_route_reaches_receiver_once_despite_overlapping_alias() {
        loopback(false, true).await;
    }

    async fn loopback(with_local: bool, with_routes: bool) {
        use crate::{media::Player, session::Context as ReceiverContext};
        use audio::Sink;
        use device::{DeviceInfo, Features};
        use std::sync::Mutex;

        struct Capture {
            null: audio::Null,
            samples: Arc<Mutex<SampleCounts>>,
        }
        impl Sink for Capture {
            fn sample_rate(&self) -> u32 {
                self.null.sample_rate()
            }
            fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
                {
                    let mut captured = self.samples.lock().unwrap();
                    captured.record(samples);
                }
                self.null.write(samples)
            }
            fn delay_frames(&mut self) -> u32 {
                self.null.delay_frames()
            }
            fn stop(&mut self) {
                self.null.stop();
            }
        }
        let captured = Arc::new(Mutex::new(SampleCounts::default()));
        let samples = captured.clone();
        let audio = audio_stream::Config {
            device: "null".into(),
            fallback_latency: Duration::from_millis(250),
            sink: Some(audio::SinkFactory::new(move |rate, _latency| {
                Ok(Box::new(Capture {
                    null: audio::Null::new(rate),
                    samples: samples.clone(),
                }))
            })),
            ..Default::default()
        };
        let identity = pair::Identity::from_seed("loopback-receiver", &[23; 32]);
        let context = Arc::new(ReceiverContext {
            device: DeviceInfo {
                name: "Loopback null receiver".into(),
                device_id: "02:00:00:00:00:23".into(),
                model: "AirPlay".into(),
                features: Features::AUDIO_RECEIVER,
                status_flags: 4,
                pairing_id: identity.pairing_id.clone(),
                public_key: identity.public_key(),
                source_version: "366.0".into(),
                display: None,
                audio_only_ui: false,
                group: None,
                pgid: "00000000-0000-4000-8000-000000000000".into(),
                enclosure_color: None,
            },
            identity,
            store: Box::new(pair::MemoryStore::default()),
            pin: "1234".into(),
            require_pin: false,
            player: Arc::new(Player::new(audio, timesync::TimeSync::idle(23))),
            mdns: Mutex::new(None),
            port: 0,
            #[cfg(feature = "mirroring")]
            mirror: None,
            remote: Default::default(),
            mrp: None,
            session_group: Default::default(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let receiver = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            crate::serve::serve_connection(stream, context, peer).await
        });
        let (_stop, stopped) = watch::channel(false);
        let mut pcm = Vec::new();
        // Keep real-time content available beyond startup/fallback buffering on
        // hosts with coarse timer resolution. Exact PCM assertions remain below.
        const PACKETS: usize = 256;
        for _ in 0..(FRAMES * PACKETS) {
            pcm.extend_from_slice(&0x1234_i16.to_be_bytes());
            pcm.extend_from_slice(&(-0x2345_i16).to_be_bytes());
        }
        let mut options = Options {
            targets: vec![address.to_string()],
            volume_db: 0.0,
            latency_ms: 250,
            local: None,
            control_socket: None,
        };
        struct LocalCapture(Arc<Mutex<Vec<(Arc<AudioPacket>, std::time::Instant)>>>);
        impl sender::LocalSink for LocalCapture {
            async fn write(
                &mut self,
                packet: Arc<AudioPacket>,
                at: std::time::Instant,
            ) -> Result<()> {
                self.0.lock().unwrap().push((packet, at));
                Ok(())
            }
            async fn finish(&mut self) -> Result<()> {
                Ok(())
            }
        }
        let local_seen = Arc::new(Mutex::new(Vec::new()));
        let control = crate::send_control::RelayControl::default();
        let monitoring = if with_local || with_routes {
            let handle = control.clone();
            Some(tokio::spawn(async move {
                tokio::time::timeout(Duration::from_secs(6), async {
                    loop {
                        if handle.all_media_started() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                let snapshot = handle.snapshot();
                assert_eq!(snapshot.len(), 1);
                assert!(snapshot[0].media.is_some());
                assert_eq!(snapshot[0].endpoint, address);
                assert!(handle.progress(address).is_some());
                handle
                    .set_volume(address, Volume::db(0.0).unwrap())
                    .unwrap();
            }))
        } else {
            None
        };
        let result = tokio::time::timeout(Duration::from_secs(12), async {
            if with_local {
                run_with_local_controlled(
                    options,
                    std::io::Cursor::new(pcm),
                    stopped,
                    LocalCapture(local_seen.clone()),
                    control.clone(),
                )
                .await
            } else if with_routes {
                use sender::outputs::{OutputCatalog, OutputRoute, ReceiverEndpoint};
                let catalog = OutputCatalog::new(vec![OutputRoute {
                    uid: "logical-output".into(),
                    name: "Output".into(),
                    model: "AirPlay".into(),
                    aliases: vec!["group-alias".into()],
                    receivers: vec![ReceiverEndpoint {
                        uid: "receiver".into(),
                        address,
                    }],
                }])?;
                options.targets.clear();
                run_selected(
                    options,
                    &catalog,
                    &["logical-output".into(), "group-alias".into()],
                    std::io::Cursor::new(pcm),
                    stopped,
                    control.clone(),
                )
                .await
            } else {
                run(options, std::io::Cursor::new(pcm), stopped).await
            }
        })
        .await;
        if !matches!(&result, Ok(Ok(()))) {
            receiver.abort();
        }
        result.expect("loopback deadline").expect("sender loopback");
        if let Some(monitoring) = monitoring {
            monitoring.await.unwrap();
            assert!(control.snapshot().is_empty());
            assert!(!control.all_media_started());
        }
        tokio::time::timeout(Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let actual = captured.lock().unwrap();
        let exact_pairs = actual.exact_pairs;
        assert!(
            exact_pairs >= FRAMES * 4,
            "too few exact S16BE stereo frames: {exact_pairs}; captured {} samples",
            actual.samples
        );
        if with_local {
            let records = local_seen.lock().unwrap();
            assert_eq!(records.len(), PACKETS);
            for (i, (packet, at)) in records.iter().enumerate() {
                assert_eq!(packet.frame_index(), i as u64 * FRAMES as u64);
                assert_eq!(*at, packet.play_at(Duration::from_millis(250)).unwrap());
                assert_eq!(&packet.pcm()[..4], &[0x12, 0x34, 0xdc, 0xbb]);
            }
        }
    }
}
