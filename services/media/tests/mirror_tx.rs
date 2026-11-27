//! Тестовый приёмник для TX зеркалирования: pair-verify, порядок сообщений,
//! ключи SETUP, расшифровка видео/звука, разрыв и повторное подключение.
//!
//! Только loopback. Это проверка протокола и криптографии нашего отправителя,
//! а не воспроизведение картинки на Apple TV.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use crypto::Channel;
use media::tx::mirror::{
    AudioFormat, MirrorAuth, MirrorConfig, MirrorSession, VIDEO_HEADER_LEN, VideoCodec,
};
use media::tx::{Identity, PairingCredentials};
use pair::hap::{PairVerify, VerifyOutcome};
use pair::{Identity as PairIdentity, MemoryStore, PairingStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

const H264_RECORD: &[u8] = &[
    1, 0x64, 0x00, 0x28, 0xff, 0xe1, 0x00, 0x04, 0x67, 0x64, 0x00, 0x28,
];
const H264_FRAME: &[u8] = &[0, 0, 0, 3, 0x65, 0x88, 0x84];
const AUDIO_FRAME: &[u8] = &[0x11, 0x22, 0x33, 0x44, 0x55];

struct Conn {
    stream: TcpStream,
    raw: Vec<u8>,
    plain: Vec<u8>,
    channel: Option<Channel>,
}

impl Conn {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            raw: Vec::new(),
            plain: Vec::new(),
            channel: None,
        }
    }

    async fn fill(&mut self) -> Result<()> {
        let mut buf = [0u8; 4096];
        let n = self.stream.read(&mut buf).await?;
        if n == 0 {
            bail!("control connection closed");
        }
        if let Some(channel) = self.channel.as_mut() {
            self.raw.extend_from_slice(&buf[..n]);
            channel.open(&mut self.raw, &mut self.plain)?;
        } else {
            self.plain.extend_from_slice(&buf[..n]);
        }
        Ok(())
    }

    async fn request(&mut self) -> Result<rtsp::Request> {
        loop {
            if let Some(request) = rtsp::decode_request(&mut self.plain)? {
                return Ok(request);
            }
            self.fill().await?;
        }
    }

    async fn reply(&mut self, request: &rtsp::Request, body: Vec<u8>) -> Result<()> {
        self.reply_status(request, 200, body).await
    }

    async fn reply_status(
        &mut self,
        request: &rtsp::Request,
        status: u16,
        body: Vec<u8>,
    ) -> Result<()> {
        let mut response = rtsp::Response::to(request, status);
        response.body = body;
        let mut plain = Vec::new();
        response.encode(&mut plain);
        match self.channel.as_mut() {
            Some(channel) => {
                let mut sealed = Vec::new();
                channel.seal(&plain, &mut sealed)?;
                self.stream.write_all(&sealed).await?;
            }
            None => self.stream.write_all(&plain).await?,
        }
        Ok(())
    }
}

async fn read_video_packet(video: &mut TcpStream) -> Result<([u8; VIDEO_HEADER_LEN], Vec<u8>)> {
    let mut header = [0u8; VIDEO_HEADER_LEN];
    video.read_exact(&mut header).await?;
    let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let mut payload = vec![0u8; len];
    video.read_exact(&mut payload).await?;
    Ok((header, payload))
}

fn plist(body: &[u8]) -> Result<plist::Dictionary> {
    let value = plist::Value::from_reader(std::io::Cursor::new(body.to_vec()))?;
    value
        .as_dictionary()
        .cloned()
        .ok_or_else(|| anyhow!("SETUP body is not a dictionary"))
}

fn streams(d: &plist::Dictionary) -> Result<&plist::Dictionary> {
    d.get("streams")
        .and_then(|v| v.as_array())
        .and_then(|v| v.first())
        .and_then(|v| v.as_dictionary())
        .ok_or_else(|| anyhow!("no stream descriptor"))
}

fn binary(d: plist::Dictionary) -> Vec<u8> {
    let mut bytes = Vec::new();
    plist::Value::Dictionary(d)
        .to_writer_binary(&mut bytes)
        .unwrap();
    bytes
}

fn dictionary(items: Vec<(&str, plist::Value)>) -> plist::Dictionary {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

#[derive(Debug)]
struct Round {
    audio_shk: [u8; 32],
    video_id: u64,
    video_latency: i64,
    audio_stream_id: u64,
    first_frame: Vec<u8>,
    keyframe: bool,
    pts: u64,
    audio_plain: Vec<u8>,
}

/// Один цикл: pair-verify, SETUP звука и видео, RECORD, кадры, TEARDOWN.
struct Fixture {
    store: MemoryStore,
    audio_socket: UdpSocket,
    audio_control_socket: UdpSocket,
    video_listener: TcpListener,
    event_listener: TcpListener,
    timing_socket: UdpSocket,
}

async fn serve_round(
    control: TcpStream,
    mut verify: Option<PairVerify>,
    fixture: &Fixture,
    close_after_first_frame: bool,
    expect_audio: bool,
) -> Result<Round> {
    let audio_socket = &fixture.audio_socket;
    let audio_control_socket = &fixture.audio_control_socket;
    let video_listener = &fixture.video_listener;
    let mut conn = Conn::new(control);
    let mut shared: Option<[u8; 32]> = None;
    let mut audio_shk = [0u8; 32];
    let mut video_id = None;
    let mut audio_stream_id = None;
    let mut video_latency = 0i64;
    let mut _event_connection = None;
    let mut rejected_modern = false;

    loop {
        let request = conn.request().await?;
        match (request.method.as_str(), request.uri.as_str()) {
            ("POST", "/pair-verify") => {
                let verifier = verify
                    .as_mut()
                    .ok_or_else(|| anyhow!("unexpected second pair-verify"))?;
                let outcome = verifier.handle(&request.body, &fixture.store)?;
                match outcome {
                    VerifyOutcome::Continue(body) => conn.reply(&request, body).await?,
                    VerifyOutcome::Verified {
                        reply,
                        keys,
                        shared: secret,
                    } => {
                        // Ответ на M3 уходит открытым текстом, затем канал шифруется.
                        conn.reply(&request, reply).await?;
                        conn.channel = Some(Channel::new(keys));
                        shared = Some(secret);
                    }
                    VerifyOutcome::Rejected(_) => bail!("pair-verify rejected"),
                }
            }
            ("SETUP", _) => {
                let body = plist(&request.body)?;
                let stream = streams(&body)?;
                let kind = stream
                    .get("type")
                    .and_then(|v| v.as_unsigned_integer())
                    .ok_or_else(|| anyhow!("stream without type"))?;
                if kind == 96 {
                    if close_after_first_frame && !rejected_modern {
                        assert!(stream.contains_key("streamConnections"));
                        rejected_modern = true;
                        conn.reply_status(&request, 415, Vec::new()).await?;
                        continue;
                    }
                    if rejected_modern {
                        assert!(!stream.contains_key("streamConnections"));
                        assert!(stream.contains_key("controlPort"));
                    }
                    // Первый SETUP: сессия и звук экрана.
                    assert_eq!(
                        body.get("timingProtocol").and_then(|v| v.as_string()),
                        Some("NTP")
                    );
                    assert!(body.get("deviceID").and_then(|v| v.as_string()).is_some());
                    assert!(
                        body.get("timingPort")
                            .and_then(|v| v.as_unsigned_integer())
                            .is_some()
                    );
                    let shk = stream
                        .get("shk")
                        .and_then(|v| v.as_data())
                        .ok_or_else(|| anyhow!("audio SETUP has no shk"))?;
                    audio_shk.copy_from_slice(shk);
                    assert_eq!(
                        stream.get("usingScreen").and_then(|v| v.as_boolean()),
                        Some(true)
                    );
                    assert_eq!(
                        stream.get("ct").and_then(|v| v.as_unsigned_integer()),
                        Some(8)
                    );
                    audio_stream_id = stream
                        .get("streamConnectionID")
                        .and_then(|v| v.as_unsigned_integer());
                    let reply = dictionary(vec![
                        (
                            "timingPort",
                            plist::Value::Integer(
                                (fixture.timing_socket.local_addr()?.port() as u64).into(),
                            ),
                        ),
                        ("skipRecord", plist::Value::Boolean(close_after_first_frame)),
                        (
                            "eventPort",
                            plist::Value::Integer(
                                (fixture.event_listener.local_addr()?.port() as u64).into(),
                            ),
                        ),
                        (
                            "streams",
                            plist::Value::Array(vec![plist::Value::Dictionary(dictionary(vec![
                                ("type", plist::Value::Integer(96u64.into())),
                                (
                                    "dataPort",
                                    plist::Value::Integer(
                                        (audio_socket.local_addr()?.port() as u64).into(),
                                    ),
                                ),
                                (
                                    "controlPort",
                                    plist::Value::Integer(
                                        (audio_control_socket.local_addr()?.port() as u64).into(),
                                    ),
                                ),
                            ]))]),
                        ),
                    ]);
                    conn.reply(&request, binary(reply)).await?;
                    let mut timing_packet = [0u8; 64];
                    let expected_port = body["timingPort"].as_unsigned_integer().unwrap();
                    let (n, peer) = tokio::time::timeout(Duration::from_secs(2), async {
                        loop {
                            let (n, peer) =
                                fixture.timing_socket.recv_from(&mut timing_packet).await?;
                            if peer.port() as u64 == expected_port {
                                break Ok::<_, std::io::Error>((n, peer));
                            }
                        }
                    })
                    .await??;
                    assert_eq!((n, timing_packet[1]), (32, 0xd2));
                    assert_eq!(peer.port() as u64, expected_port);
                    _event_connection = Some(
                        tokio::time::timeout(
                            Duration::from_secs(2),
                            fixture.event_listener.accept(),
                        )
                        .await??
                        .0,
                    );
                } else if kind == 110 {
                    assert!(shared.is_some(), "video SETUP before pair-verify");
                    assert_eq!(
                        body.get("streams")
                            .and_then(|v| v.as_array())
                            .map(|v| v.len()),
                        Some(1)
                    );
                    video_id = stream
                        .get("streamConnectionID")
                        .and_then(|v| v.as_unsigned_integer());
                    video_latency = stream
                        .get("latencyMs")
                        .and_then(|v| v.as_signed_integer())
                        .unwrap_or(0);
                    let reply = dictionary(vec![(
                        "streams",
                        plist::Value::Array(vec![plist::Value::Dictionary(dictionary(vec![
                            ("type", plist::Value::Integer(110u64.into())),
                            (
                                "dataPort",
                                plist::Value::Integer(
                                    (video_listener.local_addr()?.port() as u64).into(),
                                ),
                            ),
                        ]))]),
                    )]);
                    conn.reply(&request, binary(reply)).await?;
                    if close_after_first_frame {
                        break;
                    }
                } else {
                    bail!("unexpected stream type {kind}");
                }
            }
            ("RECORD", _) => {
                assert!(video_id.is_some(), "RECORD before video SETUP");
                assert!(
                    request.headers.get("Session").is_some(),
                    "RECORD carries the session UUID header"
                );
                conn.reply(&request, Vec::new()).await?;
                break;
            }
            (method, uri) => bail!("unexpected request {method} {uri}"),
        }
    }

    let shared = shared.ok_or_else(|| anyhow!("session started without pair-verify"))?;
    let video_id = video_id.ok_or_else(|| anyhow!("no video stream id"))?;
    let mut video = video_listener.accept().await?.0;
    video.set_nodelay(true)?;
    let video_key = crypto::kdf::hkdf_sha512(
        &shared,
        format!("DataStream-Salt{video_id}").as_bytes(),
        b"DataStream-Output-Encryption-Key",
    );

    // Конфигурация кодека: не шифруется.
    let (header, payload) = read_video_packet(&mut video).await?;
    assert_eq!(header[4], 0x01, "codec config packet: {header:?}");
    assert_eq!(header[6], VideoCodec::H264.format_option());
    assert_eq!(payload, H264_RECORD);

    // Кадр: ChaCha20-Poly1305, AAD — заголовок, nonce — счётчик 0.
    let (header, payload) = read_video_packet(&mut video).await?;
    let len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    assert_eq!(len, payload.len());
    let plain = crypto::aead::open(&video_key, &0u64.to_le_bytes(), &header, &payload)
        .context("video frame authentication")?;
    assert_eq!(plain, H264_FRAME);
    let pts = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let keyframe = header[5] == 0x10;

    let mut plain_audio = Vec::new();
    if expect_audio {
        // Звук: RTP 0x60, SSRC 0, ChaCha с shk и nonce в хвосте.
        let mut datagram = vec![0u8; 2048];
        let (n, _) = tokio::time::timeout(
            Duration::from_secs(5),
            audio_socket.recv_from(&mut datagram),
        )
        .await
        .context("audio frame timeout")??;
        let packet = &datagram[..n];
        assert_eq!(packet[0], 0x80);
        assert_eq!(packet[1], 0x60);
        assert_eq!(&packet[8..12], &[0, 0, 0, 0], "SSRC is zero");
        let nonce_at = packet.len() - 8;
        let nonce: [u8; 8] = packet[nonce_at..].try_into().unwrap();
        plain_audio = crypto::aead::open(&audio_shk, &nonce, &packet[4..12], &packet[12..nonce_at])
            .context("audio frame authentication")?;
        assert_eq!(plain_audio, AUDIO_FRAME);

        // TimeAnnounce идёт на control-порт.
        let (_, _) = tokio::time::timeout(
            Duration::from_secs(5),
            audio_control_socket.recv_from(&mut datagram),
        )
        .await
        .context("audio sync timeout")??;
        assert_eq!(datagram[0], 0x80);
        assert_eq!(datagram[1], 0xd4);
        assert_eq!(&datagram[2..4], &4u16.to_be_bytes());
    }

    if close_after_first_frame {
        video.shutdown().await?;
        drop(video);
    } else {
        drop(video);
    }

    // TEARDOWN от клиента (после обнаружения разрыва или остановки).
    let request = conn.request().await?;
    assert_eq!(request.method, "TEARDOWN");
    conn.reply(&request, Vec::new()).await?;
    // Держим сокет, пока клиент сам не закроет его: иначе Windows отдаёт RST.
    let mut probe = [0u8; 256];
    let _ = tokio::time::timeout(Duration::from_secs(2), conn.stream.read(&mut probe)).await;

    Ok(Round {
        audio_shk,
        video_id,
        video_latency,
        audio_stream_id: audio_stream_id.ok_or_else(|| anyhow!("no audio id"))?,
        first_frame: H264_FRAME.to_vec(),
        keyframe,
        pts,
        audio_plain: plain_audio,
    })
}

#[tokio::test]
async fn mirror_sender_pairs_sets_up_streams_and_reconnects() {
    let accessory = PairIdentity::from_seed("accessory:mirror", &[7u8; 32]);
    let controller = PairIdentity::from_seed("controller:mirror", &[9u8; 32]);
    let fixture = Fixture {
        store: MemoryStore::default(),
        audio_socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
        audio_control_socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
        video_listener: TcpListener::bind("127.0.0.1:0").await.unwrap(),
        event_listener: TcpListener::bind("127.0.0.1:0").await.unwrap(),
        timing_socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
    };
    fixture
        .store
        .add(controller.pairing_id.as_bytes(), controller.public_key());
    let credentials = PairingCredentials::new(
        controller.clone(),
        accessory.pairing_id.as_bytes().to_vec(),
        accessory.public_key(),
    )
    .unwrap();

    let control = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = control.local_addr().unwrap();
    let receiver = tokio::spawn({
        let accessory = accessory.clone();
        async move {
            let mut rounds = Vec::new();
            for (close, audio) in [(true, true), (false, false)] {
                let (stream, _) = control.accept().await?;
                stream.set_nodelay(true)?;
                let verify = PairVerify::new(accessory.clone());
                rounds.push(serve_round(stream, Some(verify), &fixture, close, audio).await?);
            }
            Ok::<_, anyhow::Error>(rounds)
        }
    });

    let mut identity = Identity::new("controller:mirror", "Mirror TX");
    identity.model = "iPhone".into();
    identity.source_version = "690.7.1".into();
    let mut config = MirrorConfig::new(
        endpoint,
        identity,
        MirrorAuth::Paired(Box::new(credentials)),
    );
    config.audio = AudioFormat::AAC_ELD;
    let mut session = MirrorSession::connect(config)
        .await
        .expect("mirror connect");

    session
        .send_video_config(VideoCodec::H264, H264_RECORD, 42)
        .await
        .unwrap();
    session
        .send_video_frame(0x0000_0001_8000_0000, 0, true, H264_FRAME)
        .await
        .unwrap();
    session.send_audio_frame(1000, AUDIO_FRAME).await.unwrap();
    session
        .send_audio_sync(0x0000_0001_8000_0000, 1000)
        .await
        .unwrap();

    // Разрыв видеоканала обнаруживается без ожидания следующего кадра.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !session.is_disconnected() {
        assert!(
            std::time::Instant::now() < deadline,
            "disconnect not detected"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    session.reconnect().await.expect("mirror reconnect");
    session
        .send_video_frame(0x0000_0002_0000_0000, 0, false, H264_FRAME)
        .await
        .unwrap();
    session.stop().await.expect("mirror stop");

    let rounds = tokio::time::timeout(Duration::from_secs(10), receiver)
        .await
        .expect("receiver timeout")
        .expect("receiver task")
        .expect("protocol");
    assert_eq!(rounds.len(), 2);
    assert_ne!(rounds[0].video_id, rounds[1].video_id, "new stream id");
    assert_ne!(
        rounds[0].audio_stream_id, rounds[1].audio_stream_id,
        "new audio stream id"
    );
    assert_ne!(
        rounds[0].audio_shk, rounds[1].audio_shk,
        "fresh audio key per session"
    );
    assert_eq!(rounds[0].first_frame, H264_FRAME);
    assert!(rounds[0].keyframe);
    assert_eq!(rounds[0].pts, 0x0000_0001_8000_0000);
    assert_eq!(rounds[0].audio_plain, AUDIO_FRAME);
    assert_eq!(rounds[0].video_latency, 50);
}

#[derive(Clone, Copy, PartialEq)]
enum Cancel {
    AfterPairVerify,
    AfterAudioSetup,
    None,
}

struct EarlyRound {
    session_uuid: Option<String>,
    shk: Option<[u8; 32]>,
    timing_peer: Option<u16>,
}

/// Раунд с отменой на выбранном этапе: pair-verify, затем (если этап позже)
/// аудио-SETUP с event/timing-портами и разрыв соединения без TEARDOWN.
async fn early_round(
    fixture: &Fixture,
    control: TcpStream,
    accessory: PairIdentity,
    cancel: Cancel,
) -> Result<EarlyRound> {
    let mut conn = Conn::new(control);
    let mut verify = PairVerify::new(accessory);
    let mut round = EarlyRound {
        session_uuid: None,
        shk: None,
        timing_peer: None,
    };
    loop {
        let request = conn.request().await?;
        match (request.method.as_str(), request.uri.as_str()) {
            ("POST", "/pair-verify") => match verify.handle(&request.body, &fixture.store)? {
                VerifyOutcome::Continue(body) => conn.reply(&request, body).await?,
                VerifyOutcome::Verified { reply, keys, .. } => {
                    conn.reply(&request, reply).await?;
                    conn.channel = Some(Channel::new(keys));
                    break;
                }
                VerifyOutcome::Rejected(_) => bail!("pair-verify rejected"),
            },
            (method, uri) => bail!("unexpected {method} {uri}"),
        }
    }
    if cancel == Cancel::AfterPairVerify {
        return Ok(round);
    }

    let request = conn.request().await?;
    assert_eq!(request.method, "SETUP");
    let body = plist(&request.body)?;
    let stream = streams(&body)?;
    assert_eq!(
        stream.get("type").and_then(|v| v.as_unsigned_integer()),
        Some(96)
    );
    round.session_uuid = body
        .get("sessionUUID")
        .and_then(|v| v.as_string())
        .map(str::to_owned);
    let mut shk = [0u8; 32];
    shk.copy_from_slice(stream.get("shk").and_then(|v| v.as_data()).unwrap());
    round.shk = Some(shk);
    let reply = dictionary(vec![
        (
            "timingPort",
            plist::Value::Integer((fixture.timing_socket.local_addr()?.port() as u64).into()),
        ),
        (
            "eventPort",
            plist::Value::Integer((fixture.event_listener.local_addr()?.port() as u64).into()),
        ),
        (
            "streams",
            plist::Value::Array(vec![plist::Value::Dictionary(dictionary(vec![
                ("type", plist::Value::Integer(96u64.into())),
                (
                    "dataPort",
                    plist::Value::Integer(
                        (fixture.audio_socket.local_addr()?.port() as u64).into(),
                    ),
                ),
                (
                    "controlPort",
                    plist::Value::Integer(
                        (fixture.audio_control_socket.local_addr()?.port() as u64).into(),
                    ),
                ),
            ]))]),
        ),
    ]);
    conn.reply(&request, binary(reply)).await?;

    let expected_port = body["timingPort"].as_unsigned_integer().unwrap();
    let mut timing_packet = [0u8; 64];
    let (n, peer) = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let (n, peer) = fixture.timing_socket.recv_from(&mut timing_packet).await?;
            if peer.port() as u64 == expected_port {
                break Ok::<_, std::io::Error>((n, peer));
            }
        }
    })
    .await??;
    assert_eq!((n, timing_packet[1]), (32, 0xd2));
    round.timing_peer = Some(peer.port());
    let (mut event, _) =
        tokio::time::timeout(Duration::from_secs(2), fixture.event_listener.accept()).await??;

    if cancel == Cancel::AfterAudioSetup {
        drop(conn);
        // Клиент обязан закрыть канал событий после отмены.
        let mut probe = [0u8; 64];
        let closed = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match event.read(&mut probe).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => continue,
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "event-канал не закрыт после отмены");
        return Ok(round);
    }

    let request = conn.request().await?;
    assert_eq!(request.method, "SETUP");
    let body = plist(&request.body)?;
    let stream = streams(&body)?;
    assert_eq!(
        stream.get("type").and_then(|v| v.as_unsigned_integer()),
        Some(110)
    );
    let reply = dictionary(vec![(
        "streams",
        plist::Value::Array(vec![plist::Value::Dictionary(dictionary(vec![
            ("type", plist::Value::Integer(110u64.into())),
            (
                "dataPort",
                plist::Value::Integer((fixture.video_listener.local_addr()?.port() as u64).into()),
            ),
        ]))]),
    )]);
    conn.reply(&request, binary(reply)).await?;

    let request = conn.request().await?;
    assert_eq!(request.method, "RECORD");
    conn.reply(&request, Vec::new()).await?;
    let request = conn.request().await?;
    assert_eq!(request.method, "TEARDOWN");
    conn.reply(&request, Vec::new()).await?;
    let _ = event;
    Ok(round)
}

#[tokio::test]
async fn cancelled_setup_stages_leave_no_sockets_and_retry_gets_new_parameters() {
    let accessory = PairIdentity::from_seed("accessory:mirror-cancel", &[31u8; 32]);
    let controller = PairIdentity::from_seed("controller:mirror-cancel", &[33u8; 32]);
    let fixture = Fixture {
        store: MemoryStore::default(),
        audio_socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
        audio_control_socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
        video_listener: TcpListener::bind("127.0.0.1:0").await.unwrap(),
        event_listener: TcpListener::bind("127.0.0.1:0").await.unwrap(),
        timing_socket: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
    };
    fixture
        .store
        .add(controller.pairing_id.as_bytes(), controller.public_key());
    let credentials = PairingCredentials::new(
        controller.clone(),
        accessory.pairing_id.as_bytes().to_vec(),
        accessory.public_key(),
    )
    .unwrap();
    let control = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = control.local_addr().unwrap();

    let receiver = tokio::spawn({
        let accessory = accessory.clone();
        async move {
            let mut rounds = Vec::new();
            for (index, cancel) in [
                Cancel::AfterPairVerify,
                Cancel::AfterAudioSetup,
                Cancel::None,
            ]
            .into_iter()
            .enumerate()
            {
                let (stream, _) = control.accept().await?;
                stream.set_nodelay(true)?;
                rounds.push(early_round(&fixture, stream, accessory.clone(), cancel).await?);
                if index == 0 {
                    assert!(
                        tokio::time::timeout(
                            Duration::from_millis(300),
                            fixture.event_listener.accept()
                        )
                        .await
                        .is_err(),
                        "после отмены канал событий не открыт"
                    );
                }
                if index == 1 {
                    let mut probe = [0u8; 64];
                    assert!(
                        tokio::time::timeout(
                            Duration::from_millis(400),
                            fixture.timing_socket.recv_from(&mut probe)
                        )
                        .await
                        .is_err(),
                        "после отмены timing-сокет клиента молчит"
                    );
                }
            }
            Ok::<_, anyhow::Error>(rounds)
        }
    });

    let mut identity = Identity::new("controller:mirror-cancel", "Mirror TX");
    identity.model = "iPhone".into();
    let config = || {
        let mut config = MirrorConfig::new(
            endpoint,
            identity.clone(),
            MirrorAuth::Paired(Box::new(credentials.clone())),
        );
        config.audio = AudioFormat::AAC_ELD;
        config
    };

    assert!(
        MirrorSession::connect(config()).await.is_err(),
        "отмена сразу после pair-verify"
    );
    assert!(
        MirrorSession::connect(config()).await.is_err(),
        "отмена после аудио-SETUP"
    );
    let session = MirrorSession::connect(config())
        .await
        .expect("третья попытка устанавливает сессию");
    session.stop().await.expect("mirror stop");

    let rounds = tokio::time::timeout(Duration::from_secs(10), receiver)
        .await
        .expect("receiver timeout")
        .expect("receiver task")
        .expect("protocol");
    assert!(rounds[0].session_uuid.is_none(), "до SETUP не дошло");
    let second = rounds[1].shk.expect("shk отменённого раунда");
    let third = rounds[2].shk.expect("shk успешного раунда");
    assert_ne!(second, third, "новая попытка — новые ключи потока");
    assert_ne!(rounds[1].session_uuid, rounds[2].session_uuid);
    assert_ne!(
        rounds[1].timing_peer, rounds[2].timing_peer,
        "timing-сокет создаётся заново на каждую попытку"
    );
}

/// Отказ видео-SETUP после удачного аудио-SETUP не оставляет сессию: уходит
/// TEARDOWN с тем же Session, а RECORD не отправляется.
#[tokio::test]
async fn rejected_video_setup_tears_down_the_partial_session() {
    let accessory = PairIdentity::from_seed("accessory:mirror-fail", &[21u8; 32]);
    let controller = PairIdentity::from_seed("controller:mirror-fail", &[23u8; 32]);
    let store = MemoryStore::default();
    store.add(controller.pairing_id.as_bytes(), controller.public_key());
    let credentials = PairingCredentials::new(
        controller.clone(),
        accessory.pairing_id.as_bytes().to_vec(),
        accessory.public_key(),
    )
    .unwrap();

    let event_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let control = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = control.local_addr().unwrap();
    let receiver = tokio::spawn(async move {
        let (stream, _) = control.accept().await?;
        stream.set_nodelay(true)?;
        let mut conn = Conn::new(stream);
        let mut verify = PairVerify::new(accessory.clone());
        let mut session_uuid = None;
        loop {
            let request = conn.request().await?;
            match (request.method.as_str(), request.uri.as_str()) {
                ("POST", "/pair-verify") => match verify.handle(&request.body, &store)? {
                    VerifyOutcome::Continue(body) => conn.reply(&request, body).await?,
                    VerifyOutcome::Verified { reply, keys, .. } => {
                        conn.reply(&request, reply).await?;
                        conn.channel = Some(Channel::new(keys));
                    }
                    VerifyOutcome::Rejected(_) => bail!("pair-verify rejected"),
                },
                ("SETUP", _) => {
                    let body = plist(&request.body)?;
                    let stream = streams(&body)?;
                    match stream.get("type").and_then(|v| v.as_unsigned_integer()) {
                        Some(96) => {
                            session_uuid = body
                                .get("sessionUUID")
                                .and_then(|v| v.as_string())
                                .map(str::to_owned);
                            let reply = dictionary(vec![
                                (
                                    "eventPort",
                                    plist::Value::Integer(
                                        (event_listener.local_addr()?.port() as u64).into(),
                                    ),
                                ),
                                (
                                    "streams",
                                    plist::Value::Array(vec![plist::Value::Dictionary(
                                        dictionary(vec![
                                            ("type", plist::Value::Integer(96u64.into())),
                                            ("dataPort", plist::Value::Integer(7000u64.into())),
                                            ("controlPort", plist::Value::Integer(7001u64.into())),
                                        ]),
                                    )]),
                                ),
                            ]);
                            conn.reply(&request, binary(reply)).await?;
                        }
                        Some(110) => conn.reply_status(&request, 500, Vec::new()).await?,
                        other => bail!("unexpected stream type {other:?}"),
                    }
                }
                ("RECORD", _) => bail!("RECORD must not follow a rejected video SETUP"),
                ("TEARDOWN", _) => {
                    return Ok::<_, anyhow::Error>((
                        session_uuid,
                        request.headers.get("Session").map(str::to_owned),
                    ));
                }
                (method, uri) => bail!("unexpected {method} {uri}"),
            }
        }
    });

    let mut identity = Identity::new("controller:mirror-fail", "Mirror TX");
    identity.model = "iPhone".into();
    let mut config = MirrorConfig::new(
        endpoint,
        identity,
        MirrorAuth::Paired(Box::new(credentials)),
    );
    config.audio = AudioFormat::AAC_ELD;
    let error = MirrorSession::connect(config)
        .await
        .err()
        .expect("video SETUP must fail");
    assert!(format!("{error:#}").contains("video SETUP"), "{error:#}");

    let (session_uuid, torn_down) = tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .expect("receiver timeout")
        .expect("receiver task")
        .expect("protocol");
    assert!(session_uuid.is_some(), "audio SETUP carries sessionUUID");
    assert_eq!(
        torn_down, session_uuid,
        "TEARDOWN names the partial session"
    );
}
