//! D10: клиент обрывается после OPTIONS, pairing и SETUP — приёмник
//! освобождает event/data/control сокеты, а следующий клиент проходит.
//! Loopback, audio null, без устройства.

use std::io::Cursor;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use media::events;
use runtime::lifecycle::receiver as serve;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::timeout;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "airplay-cleanup-{tag}-{}-{:?}",
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
        time: Some(timesync::TimeSync::idle(1)),
    }
}

struct Reply {
    status: u16,
    body: Vec<u8>,
}

async fn call(port: u16, method: &str, uri: &str, body: &[u8]) -> Reply {
    let mut stream = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port)))
        .await
        .unwrap();
    write_call(&mut stream, method, uri, body).await;
    read_reply(&mut stream).await
}

async fn write_call(stream: &mut TcpStream, method: &str, uri: &str, body: &[u8]) {
    let head = format!(
        "{method} {uri} RTSP/1.0\r\nCSeq: 1\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
}

async fn read_reply(stream: &mut TcpStream) -> Reply {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = timeout(Duration::from_secs(2), stream.read(&mut chunk))
            .await
            .expect("ответ за 2 с")
            .unwrap();
        assert_ne!(n, 0, "соединение закрыто до ответа");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]);
            let len = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + len {
                let status = head
                    .lines()
                    .next()
                    .unwrap()
                    .split(' ')
                    .nth(1)
                    .unwrap()
                    .parse()
                    .unwrap();
                return Reply {
                    status,
                    body: buf[end + 4..end + 4 + len].to_vec(),
                };
            }
        }
    }
}

fn plist(v: plist::Value) -> Vec<u8> {
    let mut out = Vec::new();
    v.to_writer_binary(&mut out).unwrap();
    out
}

fn ports(reply: &Reply) -> Vec<u16> {
    let value = plist::Value::from_reader(Cursor::new(reply.body.clone())).unwrap();
    let mut out = Vec::new();
    if let Some(streams) = value
        .as_dictionary()
        .and_then(|d| d.get("streams"))
        .and_then(|v| v.as_array())
    {
        for stream in streams {
            for key in ["dataPort", "controlPort"] {
                if let Some(port) = stream
                    .as_dictionary()
                    .and_then(|d| d.get(key))
                    .and_then(|v| v.as_unsigned_integer())
                {
                    out.push(port as u16);
                }
            }
        }
    }
    out
}

async fn info(port: u16) -> u16 {
    call(port, "GET", "/info", b"").await.status
}

async fn wait_tcp_closed(port: u16, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], port))).await {
            Err(_) => return,
            Ok(_) if Instant::now() > deadline => panic!("{what}: порт {port} не закрыт"),
            Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

async fn wait_udp_released(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], port))).await {
            Ok(_) => return,
            Err(_) if Instant::now() > deadline => panic!("UDP порт {port} не освобождён"),
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_can_vanish_after_each_stage_and_the_next_one_is_served() {
    let dir = TempDir::new("stages");
    let port = free_port();
    let receiver = runtime::AirPlay::start(options("Cleanup", dir.path(), port))
        .await
        .unwrap();

    // 1. OPTIONS и обрыв без TEARDOWN.
    {
        let reply = call(port, "OPTIONS", "*", b"").await;
        assert_eq!(reply.status, 200);
    }
    assert_eq!(info(port).await, 200);

    // 2. pairing: pair-pin-start, затем обрыв.
    {
        let reply = call(port, "POST", "/pair-pin-start", b"").await;
        assert!(
            reply.status == 200 || reply.status == 400,
            "pair-pin-start: {}",
            reply.status
        );
    }
    assert_eq!(info(port).await, 200);
    // Следующий клиент начинает pairing заново.
    let reply = call(port, "POST", "/pair-pin-start", b"").await;
    assert!(reply.status == 200 || reply.status == 400);

    // 3. начальный SETUP сессии (только пульт, без часов), затем обрыв.
    let mut session = plist::Dictionary::new();
    session.insert("timingProtocol".into(), "None".into());
    session.insert("isRemoteControlOnly".into(), true.into());
    let body = plist(plist::Value::Dictionary(session));
    let reply = call(port, "SETUP", "/setup", &body).await;
    assert_eq!(reply.status, 200);
    let event_port = plist::Value::from_reader(Cursor::new(reply.body))
        .unwrap()
        .as_dictionary()
        .and_then(|d| d.get("eventPort"))
        .and_then(|v| v.as_unsigned_integer())
        .unwrap_or(0) as u16;
    // Без сопряжения shared secret нет, eventPort = 0: канал не открыт.
    if event_port != 0 {
        wait_tcp_closed(event_port, "eventPort").await;
    }
    assert_eq!(info(port).await, 200);

    // 4. SETUP аудиопотока, затем обрыв: UDP-порты освобождаются,
    //    а новый клиент получает новый поток.
    let mut stream = plist::Dictionary::new();
    stream.insert("type".into(), 96.into());
    stream.insert("shk".into(), plist::Value::Data(vec![5; 32]));
    stream.insert("ct".into(), 1.into());
    stream.insert("ss".into(), 16.into());
    stream.insert("sr".into(), 44_100.into());
    stream.insert("spf".into(), 352.into());
    stream.insert("ch".into(), 2.into());
    let mut root = plist::Dictionary::new();
    root.insert(
        "streams".into(),
        plist::Value::Array(vec![plist::Value::Dictionary(stream.clone())]),
    );
    let reply = call(
        port,
        "SETUP",
        "/stream",
        &plist(plist::Value::Dictionary(root.clone())),
    )
    .await;
    assert_eq!(reply.status, 200);
    let first = ports(&reply);
    assert_eq!(first.len(), 2, "dataPort и controlPort");
    for p in &first {
        wait_udp_released(*p).await;
    }
    let reply = call(
        port,
        "SETUP",
        "/stream",
        &plist(plist::Value::Dictionary(root)),
    )
    .await;
    assert_eq!(reply.status, 200, "следующий клиент получает поток");
    let second = ports(&reply);
    assert_eq!(second.len(), 2);
    assert_ne!(first, second, "порты нового потока не те же");

    receiver.stop().await;
}
