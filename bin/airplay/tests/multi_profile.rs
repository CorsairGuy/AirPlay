//! Два независимых экземпляра приёмника в одном процессе: разные порты,
//! identity и профили; одновременные подключения; остановка одного не ломает
//! второй; повторный запуск с прежним состоянием сохраняет identity.
//!
//! Только loopback и `--audio null`: ни Bonjour, ни PTP-сокеты, ни железо.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use airplay::sessions::serve;
use airplay::{audio_stream, device, events, timesync};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn airplay1_sender_and_receiver_exchange_pcm() {
    use media::tx::airplay1::{Codec, Receiver, Session, SessionConfig};
    use std::sync::{Arc, Mutex};

    struct Capture(Arc<Mutex<Vec<i16>>>);
    impl airplay::audio::Sink for Capture {
        fn sample_rate(&self) -> u32 {
            44_100
        }
        fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
            self.0.lock().unwrap().extend_from_slice(samples);
            std::thread::sleep(Duration::from_secs_f64(
                samples.len() as f64 / 2.0 / 44_100.0,
            ));
            Ok(())
        }
        fn delay_frames(&mut self) -> u32 {
            0
        }
        fn stop(&mut self) {}
    }

    let dir = TempDir::new("ap1-loopback");
    let port = free_port();
    let samples = Arc::new(Mutex::new(Vec::new()));
    let output = samples.clone();
    let mut opts = options(
        "AP1",
        dir.path(),
        port,
        "AirPlay",
        timesync::TimeSync::idle(1),
    );
    opts.audio.sink = Some(airplay::audio::SinkFactory::new(move |_, _| {
        Ok(Box::new(Capture(output.clone())) as Box<dyn airplay::audio::Sink>)
    }));
    let receiver = serve::start(opts).await.unwrap();
    let target = Receiver {
        name: "AP1".into(),
        address: ([127, 0, 0, 1], port).into(),
        pcm: true,
        alac: false,
        encryption: false,
    };
    let mut sender = Session::connect(SessionConfig::new(target, Codec::Pcm))
        .await
        .unwrap();
    let pcm: Vec<u8> = (0..352).flat_map(|_| [0x03, 0xe8, 0xfc, 0x18]).collect();
    for _ in 0..8 {
        sender.send(&pcm).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if samples
                .lock()
                .unwrap()
                .windows(2)
                .any(|w| w == [1000, -1000])
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("AP1 sender PCM did not reach receiver output");
    sender.finish().await.unwrap();
    receiver.stop().await;
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "airplay-multi-{tag}-{}-{:?}",
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
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Один экземпляр: своё имя, профиль, каталог состояния (identity) и порт.
/// Служба времени передаётся снаружи: так приложение делит один движок PTP.
fn options(
    name: &str,
    state: &Path,
    port: u16,
    model: &str,
    time: timesync::TimeSync,
) -> serve::Options {
    serve::Options {
        name: name.into(),
        port,
        ptp_bind: None,
        pin: Some("1234".into()),
        state_dir: state.into(),
        // Bonjour не поднимаем: тест про экземпляры, а не про multicast.
        mdns: false,
        model: model.into(),
        // Без бита PTP: сокеты 319/320 не нужны двум экземплярам в тесте.
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
        display: device::Display {
            width: 1920,
            height: 1080,
            fps: 60,
            ..Default::default()
        },
        photo: None,
        ownership: Default::default(),
        remote: Default::default(),
        mrp: serve::Mrp::Off,
        events: std::sync::Arc::new(events::NoEvents),
        volume_range_db: 30.0,
        apply_volume: true,
        source_version: "366.0".into(),
        enclosure_color: None,
        time: Some(time),
    }
}

/// `GET /info` по RTSP и разбор двоичного plist из тела ответа.
async fn info(addr: SocketAddr) -> plist::Value {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /info RTSP/1.0\r\nCSeq: 1\r\nUser-Agent: AirPlay/1.0\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let body = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&buf[..end]).to_string();
                assert!(head.starts_with("RTSP/1.0 200"), "{head}");
                let length = head
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .expect("Content-Length в ответе /info");
                if buf.len() >= end + 4 + length {
                    return buf[end + 4..end + 4 + length].to_vec();
                }
            }
            let mut chunk = [0u8; 8192];
            let n = stream.read(&mut chunk).await.unwrap();
            assert_ne!(n, 0, "соединение закрылось до тела ответа");
            buf.extend_from_slice(&chunk[..n]);
        }
    })
    .await
    .expect("ответ /info вовремя");
    plist::Value::from_reader(std::io::Cursor::new(body)).expect("plist в /info")
}

fn string(value: &plist::Value, key: &str) -> String {
    value
        .as_dictionary()
        .and_then(|d| d.get(key))
        .and_then(|v| v.as_string())
        .unwrap_or_else(|| panic!("нет строки {key}"))
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_receivers_are_independent_across_stop_and_restart() {
    let state_a = TempDir::new("a");
    let state_b = TempDir::new("b");
    let port_a = free_port();
    let port_b = free_port();
    assert_ne!(port_a, port_b);

    // Один движок времени на процесс: экземпляры получают его клоны.
    let clock = timesync::TimeSync::idle(1);
    let a = serve::start(options(
        "Профиль A",
        state_a.path(),
        port_a,
        "AppleTV3,2",
        clock.clone(),
    ))
    .await
    .expect("экземпляр A");
    let b = serve::start(options(
        "Профиль B",
        state_b.path(),
        port_b,
        "AudioAccessory5,1",
        clock.clone(),
    ))
    .await
    .expect("экземпляр B");

    assert_eq!(a.port(), port_a);
    assert_eq!(b.port(), port_b);
    assert_ne!(a.device_id(), b.device_id(), "identity должна быть своя");

    // Профили не перетекают: имена, модели и deviceID разные.
    let (ia, ib) = tokio::join!(
        info(SocketAddr::from(([127, 0, 0, 1], port_a))),
        info(SocketAddr::from(([127, 0, 0, 1], port_b)))
    );
    assert_eq!(string(&ia, "name"), "Профиль A");
    assert_eq!(string(&ib, "name"), "Профиль B");
    assert_eq!(string(&ia, "model"), "AppleTV3,2");
    assert_eq!(string(&ib, "model"), "AudioAccessory5,1");
    assert_eq!(string(&ia, "deviceID"), a.device_id());
    assert_eq!(string(&ib, "deviceID"), b.device_id());
    let address_a = SocketAddr::from(([127, 0, 0, 1], port_a));
    let address_b = SocketAddr::from(([127, 0, 0, 1], port_b));

    // Четыре одновременных подключения: две пары к двум экземплярам.
    let (a1, a2, b1, b2) = tokio::join!(
        info(address_a),
        info(address_a),
        info(address_b),
        info(address_b)
    );
    assert_eq!(string(&a1, "name"), "Профиль A");
    assert_eq!(string(&a2, "name"), "Профиль A");
    assert_eq!(string(&b1, "name"), "Профиль B");
    assert_eq!(string(&b2, "name"), "Профиль B");

    // Остановка A: её порт закрыт, B отвечает и не меняет identity.
    let device_a = a.device_id().to_owned();
    a.stop().await;
    assert!(
        TcpStream::connect(address_a).await.is_err(),
        "порт остановленного экземпляра должен быть закрыт"
    );
    let b_after = info(address_b).await;
    assert_eq!(string(&b_after, "name"), "Профиль B");
    assert_eq!(string(&b_after, "deviceID"), b.device_id());

    // Повторный запуск A с тем же каталогом состояния: identity сохранилась,
    // соседний B при этом продолжает работать.
    let a2 = serve::start(options(
        "Профиль A2",
        state_a.path(),
        port_a,
        "AppleTV3,2",
        clock,
    ))
    .await
    .expect("повторный запуск A");
    assert_eq!(a2.device_id(), device_a);
    let ia2 = info(address_a).await;
    assert_eq!(string(&ia2, "name"), "Профиль A2");
    assert_eq!(string(&ia2, "deviceID"), device_a);
    let b_still = info(address_b).await;
    assert_eq!(string(&b_still, "deviceID"), b.device_id());

    a2.stop().await;
    b.stop().await;
}
