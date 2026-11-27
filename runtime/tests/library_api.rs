//! Публичный API `runtime::AirPlay` для приложения-потребителя (C36).
//!
//! Проверяются запуск/остановка нескольких экземпляров, независимые identity и
//! конфигурации, обработка ошибок и drop. Только loopback и `--audio null`.

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
            "airplay-api-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&path).unwrap();
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

fn options(name: &str, state: &Path, port: u16, model: &str) -> serve::Options {
    serve::Options {
        name: name.into(),
        port,
        ptp_bind: None,
        pin: Some("1234".into()),
        state_dir: state.into(),
        mdns: false,
        model: model.into(),
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

async fn status(addr: SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /info RTSP/1.0\r\nCSeq: 1\r\n\r\n")
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await.unwrap();
        assert_ne!(n, 0);
        head.extend_from_slice(&chunk[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    text.lines().next().unwrap_or_default().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_instances_start_answer_and_stop_independently() {
    let a_dir = TempDir::new("a");
    let b_dir = TempDir::new("b");
    let (pa, pb) = (free_port(), free_port());
    let a = runtime::AirPlay::start(options("A", a_dir.path(), pa, "AppleTV3,2"))
        .await
        .expect("A стартует");
    let b = runtime::AirPlay::start(options("B", b_dir.path(), pb, "AudioAccessory5,1"))
        .await
        .expect("B стартует");

    assert_eq!(a.receiver().port(), pa);
    assert_eq!(b.receiver().port(), pb);
    assert_ne!(a.receiver().device_id(), b.receiver().device_id());
    assert_eq!(a.receiver().name(), "A");
    assert_eq!(b.receiver().name(), "B");

    let (sa, sb) = tokio::join!(
        status(SocketAddr::from(([127, 0, 0, 1], pa))),
        status(SocketAddr::from(([127, 0, 0, 1], pb)))
    );
    assert!(sa.starts_with("RTSP/1.0 200"), "{sa}");
    assert!(sb.starts_with("RTSP/1.0 200"), "{sb}");

    // Остановка A: порт закрыт, B продолжает отвечать.
    a.stop().await;
    assert!(
        TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], pa)))
            .await
            .is_err()
    );
    assert!(
        status(SocketAddr::from(([127, 0, 0, 1], pb)))
            .await
            .starts_with("RTSP/1.0 200")
    );
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_an_instance_releases_its_port() {
    let dir = TempDir::new("drop");
    let port = free_port();
    {
        let instance = runtime::AirPlay::start(options("Drop", dir.path(), port, "AppleTV3,2"))
            .await
            .expect("старт");
        assert!(
            status(SocketAddr::from(([127, 0, 0, 1], port)))
                .await
                .starts_with("RTSP/1.0 200")
        );
        drop(instance);
    }
    // Drop не обязан быть асинхронным, но порт должен освободиться.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).await {
            Err(_) => break,
            Ok(_) if std::time::Instant::now() > deadline => {
                panic!("порт не освобождён после drop")
            }
            Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    // Повторный запуск на том же порту возможен.
    let again = runtime::AirPlay::start(options("Drop", dir.path(), port, "AppleTV3,2"))
        .await
        .expect("повторный старт после drop");
    again.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_port_is_reported_as_a_start_error() {
    let dir = TempDir::new("busy");
    let port = free_port();
    let first = runtime::AirPlay::start(options("First", dir.path(), port, "AppleTV3,2"))
        .await
        .expect("первый старт");
    let second = runtime::AirPlay::start(options("Second", dir.path(), port, "AppleTV3,2")).await;
    assert!(
        second.is_err(),
        "занятый порт должен возвращать ошибку, а не панику"
    );
    // Первый экземпляр не пострадал.
    assert!(
        status(SocketAddr::from(([127, 0, 0, 1], port)))
            .await
            .starts_with("RTSP/1.0 200")
    );
    first.stop().await;
}
