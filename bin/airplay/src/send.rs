//! Realtime PCM sender frontend. One bounded producer and media epoch feed all
//! connected receivers; this does not claim AP2 group/PTP synchronization.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

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
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self {
            targets: Vec::new(),
            volume_db: -29.7,
            latency_ms: 500,
        };
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
                other => bail!("неизвестный параметр {other}"),
            }
        }
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
    options: Options,
    input: R,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    options.validate()?;
    if *stop.borrow() {
        return Ok(());
    }
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
    let (audio, receiver) = broadcast::channel(QUEUE_PACKETS);
    drop(receiver);
    let (_volume, volumes) = watch::channel(volume);
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
        workers.spawn(session.run(audio.subscribe(), volumes.clone(), halted.clone()));
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
    if !matches!(result, Ok(true)) {
        halt.send_replace(true);
    }
    let mut outcome = result.map(|_| ());
    let drain = async {
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
        Duration::from_millis(u64::from(options.latency_ms)) + Duration::from_secs(5),
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
        use crate::{media::Player, session::Context as ReceiverContext};
        use audio::Sink;
        use device::{DeviceInfo, Features};
        use std::sync::Mutex;

        struct Capture {
            null: audio::Null,
            samples: Arc<Mutex<Vec<i16>>>,
        }
        impl Sink for Capture {
            fn sample_rate(&self) -> u32 {
                self.null.sample_rate()
            }
            fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
                {
                    let mut captured = self.samples.lock().unwrap();
                    if captured.len() + samples.len() <= 100_000 {
                        captured.extend_from_slice(samples);
                    }
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
        let captured = Arc::new(Mutex::new(Vec::new()));
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
        for _ in 0..(FRAMES * 48) {
            pcm.extend_from_slice(&0x1234_i16.to_be_bytes());
            pcm.extend_from_slice(&(-0x2345_i16).to_be_bytes());
        }
        let options = Options {
            targets: vec![address.to_string()],
            volume_db: 0.0,
            latency_ms: 250,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(12),
            run(options, std::io::Cursor::new(pcm), stopped),
        )
        .await;
        if !matches!(&result, Ok(Ok(()))) {
            receiver.abort();
        }
        result.expect("loopback deadline").expect("sender loopback");
        tokio::time::timeout(Duration::from_secs(2), receiver)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let actual = captured.lock().unwrap();
        let exact_pairs = actual
            .chunks_exact(2)
            .filter(|pair| *pair == [0x1234, -0x2345])
            .count();
        assert!(
            exact_pairs >= FRAMES * 4,
            "too few exact S16BE stereo frames: {exact_pairs}"
        );
    }
}
