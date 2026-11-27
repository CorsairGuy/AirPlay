//! Изоляция профиля и ключей двух экземпляров runtime (C53).
//!
//! Временные каталоги состояния, только loopback, без пользовательских
//! identity. Проверяется, что deviceID, публичный ключ и настройки не
//! перетекают между экземплярами.

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
            "airplay-isolation-{tag}-{}-{:?}",
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

fn options(name: &str, state: &Path, port: u16, version: &str) -> serve::Options {
    serve::Options {
        name: name.into(),
        port,
        ptp_bind: None,
        pin: Some("1234".into()),
        state_dir: state.into(),
        mdns: false,
        model: "AudioAccessory5,1".into(),
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
        source_version: version.into(),
        enclosure_color: None,
        time: Some(timesync::TimeSync::idle(1)),
    }
}

/// `GET /info` и разбор двоичного plist из тела ответа.
async fn info_body(addr: SocketAddr) -> plist::Value {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /info RTSP/1.0\r\nCSeq: 1\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let body = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_string();
                let length: usize = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse().ok())?
                    })
                    .expect("Content-Length");
                if buf.len() >= end + 4 + length {
                    return buf[end + 4..end + 4 + length].to_vec();
                }
            }
            let mut chunk = [0u8; 8192];
            let n = stream.read(&mut chunk).await.unwrap();
            assert_ne!(n, 0, "ответ /info оборвался");
            buf.extend_from_slice(&chunk[..n]);
        }
    })
    .await
    .expect("ответ /info вовремя");
    plist::Value::from_reader(std::io::Cursor::new(body)).unwrap()
}

fn field<'a>(value: &'a plist::Value, key: &str) -> &'a plist::Value {
    value
        .as_dictionary()
        .and_then(|d| d.get(key))
        .unwrap_or_else(|| panic!("нет ключа {key}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keys_and_profiles_do_not_leak_between_instances() {
    let dir_a = TempDir::new("a");
    let dir_b = TempDir::new("b");
    let (pa, pb) = (free_port(), free_port());
    let a = runtime::AirPlay::start(options("A", dir_a.path(), pa, "366.0"))
        .await
        .unwrap();
    let b = runtime::AirPlay::start(options("B", dir_b.path(), pb, "550.10"))
        .await
        .unwrap();

    let (ia, ib) = tokio::join!(
        info_body(SocketAddr::from(([127, 0, 0, 1], pa))),
        info_body(SocketAddr::from(([127, 0, 0, 1], pb)))
    );
    assert_ne!(field(&ia, "deviceID"), field(&ib, "deviceID"));
    assert_ne!(
        field(&ia, "pk").as_data(),
        field(&ib, "pk").as_data(),
        "публичные ключи должны различаться"
    );
    assert_eq!(
        field(&ia, "pk").as_data(),
        Some(a.receiver().device().public_key.as_slice())
    );
    assert_eq!(field(&ia, "sourceVersion").as_string(), Some("366.0"));
    assert_eq!(field(&ib, "sourceVersion").as_string(), Some("550.10"));
    assert_ne!(field(&ia, "psi"), field(&ib, "psi"));

    let device_a = a.receiver().device_id().to_owned();
    a.stop().await;
    // Повторный запуск с тем же каталогом сохраняет identity A...
    let a2 = runtime::AirPlay::start(options("A", dir_a.path(), pa, "366.0"))
        .await
        .unwrap();
    assert_eq!(a2.receiver().device_id(), device_a);
    // ...и не трогает B.
    assert_eq!(
        b.receiver().device_id(),
        field(&ib, "deviceID").as_string().unwrap()
    );
    a2.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_start_and_restart_do_not_share_state_directories() {
    let dir_a = TempDir::new("ca");
    let dir_b = TempDir::new("cb");
    let (pa, pb) = (free_port(), free_port());
    let (a, b) = tokio::join!(
        runtime::AirPlay::start(options("A", dir_a.path(), pa, "366.0")),
        runtime::AirPlay::start(options("B", dir_b.path(), pb, "366.0"))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.receiver().device_id(), b.receiver().device_id());
    a.stop().await;
    b.stop().await;

    // Второй запуск: каждый каталог помнит свою identity.
    let (a, b) = tokio::join!(
        runtime::AirPlay::start(options("A", dir_a.path(), pa, "366.0")),
        runtime::AirPlay::start(options("B", dir_b.path(), pb, "366.0"))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.receiver().device_id(), b.receiver().device_id());
    a.stop().await;
    b.stop().await;
}
