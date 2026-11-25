//! `airplay send-buffered`: отправить готовый AAC-файл (ADTS) buffered-потоком
//! (type 103), без энкодера и без чтения stdin.
//!
//! Транспорт и RTSP-последовательность живут в `media::tx::buffered_session`;
//! здесь только параметры командной строки, файл и темп отправки. Кадры идут
//! в реальном времени (1024 сэмпла = 23,2 мс), чтобы не переполнять буфер
//! приёмника; `audioBufferSize` из ответа печатается, но не используется как
//! единица pacing — его семантика не подтверждена.
//!
//! Не проверено на устройствах: ключей нет, сопряжение transient (PIN 3939),
//! как у realtime-отправителя.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use media::tx::{AAC_FRAMES_PER_PACKET, BufferedConfig, BufferedSession, adts_frames};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Instant, sleep_until};

/// Длительность одного AAC-кадра 44,1 кГц, 1024 сэмпла.
pub fn frame_duration() -> Duration {
    Duration::from_secs_f64(AAC_FRAMES_PER_PACKET as f64 / 44_100.0)
}

/// Момент отправки кадра `index` от начала воспроизведения.
pub fn frame_deadline(start: Instant, index: u64) -> Instant {
    start + frame_duration() * index as u32
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub target: String,
    pub aac: PathBuf,
    /// Порт управляющего канала отправителя; 0 — выбрать свободный.
    pub control_port: u16,
    pub timeout: Duration,
}

impl Options {
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut target = None;
        let mut aac = None;
        let mut control_port = 0u16;
        let mut timeout = Duration::from_secs(8);
        let mut args = args.iter();
        while let Some(argument) = args.next() {
            if argument == "--help" || argument == "-h" {
                bail!("справка: airplay help");
            }
            let value = args
                .next()
                .ok_or_else(|| anyhow!("{argument}: нужно значение"))?;
            match argument.as_str() {
                "--to" => {
                    if target.is_some() {
                        bail!("--to задан повторно");
                    }
                    endpoint(value)?;
                    target = Some(value.clone());
                }
                "--aac" => {
                    if aac.is_some() {
                        bail!("--aac задан повторно");
                    }
                    aac = Some(PathBuf::from(value));
                }
                "--control-port" => {
                    control_port = value.parse().context("--control-port: нужен порт")?;
                }
                "--timeout-s" => {
                    let seconds: u64 = value.parse().context("--timeout-s: нужны секунды")?;
                    if !(1..=30).contains(&seconds) {
                        bail!("--timeout-s: допустимо 1..30");
                    }
                    timeout = Duration::from_secs(seconds);
                }
                other => bail!("неизвестный параметр {other}"),
            }
        }
        let target = target.ok_or_else(|| anyhow!("укажите --to HOST[:PORT]"))?;
        let aac = aac.ok_or_else(|| anyhow!("укажите --aac FILE.aac (ADTS)"))?;
        Ok(Self {
            target,
            aac,
            control_port,
            timeout,
        })
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

/// Читает ADTS-файл и проверяет, что кадры в нём есть.
pub fn read_frames(path: &std::path::Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path).with_context(|| format!("чтение {}", path.display()))?;
    let frames = adts_frames(&bytes).map_err(|e| anyhow!("ADTS: {e:?}"))?;
    if frames.is_empty() {
        bail!("в файле нет AAC-кадров");
    }
    Ok(bytes)
}

/// Принимает и сразу закрывает управляющее соединение приёмника: этот
/// фронтенд не реализует управляющий канал, но обязан держать порт открытым,
/// иначе приёмник не сможет его установить.
async fn hold_control(listener: TcpListener) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let mut sink = [0u8; 256];
            while matches!(socket.read(&mut sink).await, Ok(n) if n > 0) {}
        });
    }
}

pub async fn run(options: Options) -> Result<()> {
    let bytes = read_frames(&options.aac)?;
    let frames = adts_frames(&bytes).map_err(|e| anyhow!("ADTS: {e:?}"))?;
    let (host, port) = endpoint(&options.target)?;
    let address = tokio::net::lookup_host((host.as_str(), port))
        .await?
        .next()
        .ok_or_else(|| anyhow!("не найден адрес {}", options.target))?;

    let stream = TcpStream::connect(address)
        .await
        .with_context(|| format!("подключение {}", options.target))?;
    let local = stream.local_addr()?.ip();
    let listener = TcpListener::bind((local, options.control_port))
        .await
        .with_context(|| format!("управляющий порт на {local}"))?;
    let control_port = listener.local_addr()?.port();
    tokio::spawn(hold_control(listener));

    let mut shk = [0u8; 32];
    for byte in &mut shk {
        *byte = rand::random();
    }
    let mut id = [0u8; 8];
    for byte in &mut id {
        *byte = rand::random();
    }
    let stream_connection_id = u64::from_be_bytes(id);
    let device_id = format!(
        "02:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>()
    );
    let uri = format!("rtsp://{local}/{stream_connection_id:016X}");
    let config = BufferedConfig::aac_44100(address, control_port, shk, stream_connection_id);
    let mut config = config;
    config.request_timeout = options.timeout;
    let mut session = BufferedSession::pair_transient(stream, uri, &device_id, config).await?;
    if let Some(size) = session.reply().audio_buffer_size {
        tracing::info!(
            bytes = size,
            "audioBufferSize приёмника (не используется как pacing)"
        );
    }
    let start = Instant::now();
    for (index, frame) in frames.iter().enumerate() {
        sleep_until(frame_deadline(start, index as u64)).await;
        session.send_frame(frame).await?;
    }
    let sent = session.finish().await?;
    println!(
        "buffered: {sent} кадров, {:.1} с",
        (frames.len() as f64) * frame_duration().as_secs_f64()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn parse_requires_target_and_file() {
        assert!(Options::parse(&args(&["--aac", "a.aac"])).is_err());
        assert!(Options::parse(&args(&["--to", "127.0.0.1"])).is_err());
        let options =
            Options::parse(&args(&["--to", "192.168.1.5:7000", "--aac", "a.aac"])).unwrap();
        assert_eq!(options.target, "192.168.1.5:7000");
        assert_eq!(options.aac, PathBuf::from("a.aac"));
        assert_eq!(options.control_port, 0);
        assert_eq!(options.timeout, Duration::from_secs(8));
    }

    #[test]
    fn parse_rejects_unknown_and_repeated_flags() {
        assert!(Options::parse(&args(&["--to", "h", "--aac", "a", "--nope", "1"])).is_err());
        assert!(Options::parse(&args(&["--to", "h", "--to", "g", "--aac", "a"])).is_err());
        assert!(Options::parse(&args(&["--to", "h", "--aac", "a", "--timeout-s", "31"])).is_err());
        assert!(Options::parse(&args(&["--to", "0.0.0.0:0", "--aac", "a"])).is_err());
    }

    #[test]
    fn frame_deadline_is_1024_frames_at_44k() {
        let start = Instant::now();
        assert_eq!(frame_duration(), Duration::from_secs_f64(1024.0 / 44_100.0));
        assert_eq!(frame_deadline(start, 0), start);
        let third = frame_deadline(start, 3) - start;
        assert!((third.as_secs_f64() - 3.0 * 1024.0 / 44_100.0).abs() < 1e-6);
    }

    #[test]
    fn read_frames_needs_valid_adts() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("airplay-adts-{}.aac", std::process::id()));
        std::fs::write(&path, b"not adts").unwrap();
        assert!(read_frames(&path).is_err());
        let mut data = Vec::new();
        let length = 7 + 3;
        data.extend_from_slice(&[
            0xFF,
            0xF1,
            0x50,
            0x80 | (((length >> 11) as u8) & 0x03),
            ((length >> 3) & 0xFF) as u8,
            (((length & 0x07) as u8) << 5) | 0x1F,
            0xFC,
            1,
            2,
            3,
        ]);
        std::fs::write(&path, &data).unwrap();
        assert_eq!(read_frames(&path).unwrap(), data);
        std::fs::remove_file(&path).ok();
    }
}
