//! Отмена вложенных операций runtime (C57).
//!
//! Остановка экземпляра во время незавершённого сопряжения должна быть
//! ограниченной по времени, закрывать его сокеты и не трогать соседний
//! экземпляр. Только loopback и `--audio null`; устройство не участвует.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use media::events;
use runtime::lifecycle::receiver as serve;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "airplay-cancel-{tag}-{}-{:?}",
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

fn options(name: &str, state: &Path, port: u16) -> serve::Options {
    serve::Options {
        name: name.into(),
        port,
        ptp_bind: None,
        // Без PIN: сопряжение остаётся в руках теста, а не устройства.
        pin: None,
        state_dir: state.into(),
        mdns: false,
        model: "AppleTV3,2".into(),
        features: Some(device::Features(
            device::Features::AUDIO_RECEIVER.0 & !device::Features::PTP,
        )),
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

async fn info(port: u16) -> String {
    let mut stream = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .unwrap();
    stream
        .write_all(b"GET /info RTSP/1.0\r\nCSeq: 1\r\n\r\n")
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    loop {
        let n = timeout(Duration::from_secs(2), stream.read(&mut chunk))
            .await
            .unwrap()
            .unwrap();
        assert_ne!(n, 0);
        head.extend_from_slice(&chunk[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Соединение, застрявшее посреди сопряжения: pair-pin-start уже принят,
/// pair-setup ещё не отправлен, TCP не закрыт.
async fn stuck_pairing(port: u16) -> TcpStream {
    let mut stream = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .unwrap();
    stream
        .write_all(
            b"POST /pair-pin-start RTSP/1.0\r\nCSeq: 1\r\nX-Apple-HKP: 3\r\nContent-Length: 0\r\n\r\n",
        )
        .await
        .unwrap();
    let mut chunk = [0u8; 512];
    let n = timeout(Duration::from_secs(2), stream.read(&mut chunk))
        .await
        .unwrap()
        .unwrap();
    assert!(n > 0);
    assert!(String::from_utf8_lossy(&chunk[..n]).starts_with("RTSP/1.0 200"));
    stream
}

async fn closed(stream: &mut TcpStream) -> bool {
    let mut probe = [0u8; 64];
    match timeout(Duration::from_secs(2), stream.read(&mut probe)).await {
        Ok(Ok(0)) => true,
        Ok(Err(_)) => true,
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_during_pairing_is_bounded_and_keeps_the_other_instance() {
    let a_dir = TempDir::new("a");
    let b_dir = TempDir::new("b");
    let (pa, pb) = (free_port(), free_port());
    let a = runtime::AirPlay::start(options("A", a_dir.path(), pa))
        .await
        .unwrap();
    let b = runtime::AirPlay::start(options("B", b_dir.path(), pb))
        .await
        .unwrap();

    let mut one = stuck_pairing(pa).await;
    let mut two = stuck_pairing(pa).await;
    timeout(Duration::from_secs(3), a.stop())
        .await
        .expect("stop не должен зависнуть на незавершённом сопряжении");
    assert!(closed(&mut one).await, "первое соединение закрыто");
    assert!(closed(&mut two).await, "второе соединение закрыто");
    assert!(TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], pa)))
        .await
        .is_err());
    assert_eq!(info(pb).await, "RTSP/1.0 200 OK", "соседний экземпляр жив");

    // Порт освобождён: на том же адресе можно поднять новый экземпляр.
    let again = runtime::AirPlay::start(options("A2", a_dir.path(), pa))
        .await
        .expect("повторный запуск после отмены");
    assert_eq!(info(pa).await, "RTSP/1.0 200 OK");
    again.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_during_pairing_releases_the_port_without_stopping_peers() {
    let dir = TempDir::new("drop");
    let port = free_port();
    {
        let instance = runtime::AirPlay::start(options("Drop", dir.path(), port))
            .await
            .unwrap();
        let mut stream = stuck_pairing(port).await;
        drop(instance);
        assert!(closed(&mut stream).await, "соединение закрыто при drop");
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).await {
            Err(_) => break,
            Ok(_) if std::time::Instant::now() > deadline => {
                panic!("порт не освобождён после drop")
            }
            Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}
