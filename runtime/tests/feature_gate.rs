//! D14/R05: профиль, объявляющий экран/видео при выключенном backend
//! зеркалирования, не должен молча уходить в сеть. Loopback, audio null.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use media::events;
use runtime::lifecycle::receiver as serve;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "airplay-gate-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn options(name: &str, state: &Path, port: u16, features: device::Features) -> serve::Options {
    serve::Options {
        name: name.into(),
        port,
        ptp_bind: None,
        pin: Some("1234".into()),
        state_dir: state.into(),
        mdns: false,
        model: "AppleTV3,2".into(),
        features: Some(features),
        status_flags: None,
        accessory_type: None,
        audio: audio_stream::Config {
            device: "null".into(),
            ..audio_stream::Config::default()
        },
        require_pin: false,
        #[cfg(feature = "mirroring")]
        mirror: None,
        #[cfg(feature = "hls")]
        hls: None,
        display: device::Display::default(),
        photo: None,
        profile: None,
        ownership: Default::default(),
        remote: Default::default(),
        mrp: serve::Mrp::Off,
        events: std::sync::Arc::new(events::NoEvents),
        volume_range_db: 30.0,
        apply_volume: true,
        source_version: "366.0".into(),
        enclosure_color: None,
        time: Some(timesync::TimeSync::idle(1)),
    }
}

async fn info_features(port: u16) -> u64 {
    let mut stream = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .unwrap();
    stream
        .write_all(b"GET /info RTSP/1.0\r\nCSeq: 1\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut chunk))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(n, 0);
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]);
            let len: usize = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse().ok())?
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + len {
                let value =
                    plist::Value::from_reader(std::io::Cursor::new(buf[end + 4..end + 4 + len].to_vec()))
                        .unwrap();
                return value
                    .as_dictionary()
                    .and_then(|d| d.get("features"))
                    .and_then(|v| v.as_unsigned_integer())
                    .unwrap();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn screen_bits_without_a_mirror_backend_refuse_to_start() {
    let dir = TempDir::new("screen");
    let features = device::Features(device::Features::SCREEN | device::Features::VIDEO);
    let err = match runtime::AirPlay::start(options("Screen", dir.path(), free_port(), features))
        .await
    {
        Ok(_) => panic!("нельзя объявлять экран без обработчика"),
        Err(err) => err,
    };
    let text = format!("{err:#}");
    assert!(text.contains("mirroring"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audio_bits_still_start_and_are_reported_unchanged() {
    let dir = TempDir::new("audio");
    let port = free_port();
    let features = device::Features(
        device::Features::AUDIO_RECEIVER.0 & !device::Features::PTP,
    );
    let receiver = runtime::AirPlay::start(options("Audio", dir.path(), port, features))
        .await
        .expect("аудиопрофиль без битов экрана стартует");
    assert_eq!(info_features(port).await, features.0);
    receiver.stop().await;
}
