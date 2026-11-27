//! Lifecycle двух экземпляров runtime с общим и раздельными idle clock handles.
//! Настоящая работающая служба часов этими тестами не проверяется.
//!
//! Проверяется, что остановка одного приёмника не останавливает другой.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use media::events;
use runtime::lifecycle::receiver as serve;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "airplay-clock-{tag}-{}-{:?}",
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

fn options(
    name: &str,
    state: &Path,
    port: u16,
    clock: Option<timesync::TimeSync>,
) -> serve::Options {
    serve::Options {
        name: name.into(),
        port,
        ptp_bind: None,
        pin: Some("1234".into()),
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
        time: clock,
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

async fn wait_released(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).await {
            Err(_) => return,
            Ok(_) if Instant::now() > deadline => panic!("порт {port} не освобождён"),
            Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_idle_handle_does_not_stop_other_instance() {
    let a_dir = TempDir::new("shared-a");
    let b_dir = TempDir::new("shared-b");
    let (pa, pb) = (free_port(), free_port());
    let clock = timesync::TimeSync::idle(7);
    let a = runtime::AirPlay::start(options("A", a_dir.path(), pa, Some(clock.clone())))
        .await
        .unwrap();
    let b = runtime::AirPlay::start(options("B", b_dir.path(), pb, Some(clock.clone())))
        .await
        .unwrap();
    assert_eq!(info(pa).await, "RTSP/1.0 200 OK");
    assert_eq!(info(pb).await, "RTSP/1.0 200 OK");

    timeout(Duration::from_secs(3), a.stop())
        .await
        .expect("остановка A ограничена по времени");
    wait_released(pa).await;
    assert_eq!(
        info(pb).await,
        "RTSP/1.0 200 OK",
        "второй экземпляр не остановлен вместе с A"
    );

    // Тот же Arc часов можно передать новому экземпляру.
    let again = runtime::AirPlay::start(options("A2", a_dir.path(), pa, Some(clock.clone())))
        .await
        .expect("повторный запуск с той же службой часов");
    assert_eq!(info(pa).await, "RTSP/1.0 200 OK");
    again.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn separate_clocks_are_independent_and_restart_reuses_the_port() {
    let a_dir = TempDir::new("sep-a");
    let b_dir = TempDir::new("sep-b");
    let (pa, pb) = (free_port(), free_port());
    let a = runtime::AirPlay::start(options(
        "A",
        a_dir.path(),
        pa,
        Some(timesync::TimeSync::idle(1)),
    ))
    .await
    .unwrap();
    let b = runtime::AirPlay::start(options(
        "B",
        b_dir.path(),
        pb,
        Some(timesync::TimeSync::idle(2)),
    ))
    .await
    .unwrap();

    timeout(Duration::from_secs(3), b.stop())
        .await
        .expect("остановка B ограничена по времени");
    wait_released(pb).await;
    assert_eq!(info(pa).await, "RTSP/1.0 200 OK", "A не зависит от часов B");

    let b_again = runtime::AirPlay::start(options(
        "B2",
        b_dir.path(),
        pb,
        Some(timesync::TimeSync::idle(2)),
    ))
    .await
    .expect("B перезапускается на том же порту");
    assert_eq!(info(pb).await, "RTSP/1.0 200 OK");
    b_again.stop().await;
    a.stop().await;
}
