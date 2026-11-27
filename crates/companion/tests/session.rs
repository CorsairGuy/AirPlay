//! Сессия Companion Link против локального сервера: корреляция `_x`, неверный
//! ответ, таймаут, обрыв, pair-verify и повторное подключение.
//!
//! Только loopback: это проверка нашего клиента и разбора ответов, а не обмен
//! с Apple TV.

use std::time::Duration;

use companion::auth::Credentials;
use companion::frame::FrameType;
use companion::link::Link;
use companion::opack::Value;
use companion::session::Session;
use companion::{Error, commands};
use crypto::aead;
use crypto::kdf::hkdf_sha512;
use pair::Identity;
use pair::tlv8::{self, Tlv8};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use x25519_dalek::{PublicKey, StaticSecret};

const DEADLINE: Duration = Duration::from_secs(2);

#[tokio::test]
async fn stalled_or_cancelled_write_closes_the_session() {
    let (stream, _peer) = tokio::io::duplex(1);
    let mut session = Session::plain(stream);
    let result = session
        .request(
            "test",
            Value::Str("x".repeat(128)),
            Duration::from_millis(20),
        )
        .await;
    assert!(matches!(result, Err(Error::Timeout)));
    assert!(!session.is_open());

    let (stream, _peer) = tokio::io::duplex(1);
    let mut session = Session::plain(stream);
    let result = tokio::time::timeout(
        Duration::from_millis(20),
        session.request("test", Value::Str("x".repeat(128)), DEADLINE),
    )
    .await;
    assert!(result.is_err());
    assert!(!session.is_open());
}

struct Peer {
    stream: TcpStream,
    link: Link,
}

impl Peer {
    async fn accept(listener: &TcpListener) -> Self {
        let (stream, _) = listener.accept().await.expect("accept");
        Self {
            stream,
            link: Link::plain(),
        }
    }

    async fn recv(&mut self) -> Value {
        loop {
            if let Some(frame) = self.link.poll().expect("poll") {
                return Link::payload(&frame).expect("payload");
            }
            let mut buffer = [0u8; 8192];
            let read = self.stream.read(&mut buffer).await.expect("read");
            assert!(read > 0, "peer closed the connection");
            self.link.feed(&buffer[..read]).expect("feed");
        }
    }

    async fn send(&mut self, value: &Value) {
        let wire = self.link.send(FrameType::Encrypted, value).expect("frame");
        self.stream.write_all(&wire).await.expect("write");
    }

    async fn recv_auth(&mut self, expected: FrameType) -> Value {
        loop {
            if let Some(frame) = self.link.poll().expect("poll auth") {
                assert_eq!(frame.frame_type, expected);
                return companion::opack::unpack(&frame.payload).expect("auth payload");
            }
            let mut buffer = [0u8; 8192];
            let n = self.stream.read(&mut buffer).await.expect("read auth");
            assert!(n > 0);
            self.link.feed(&buffer[..n]).expect("feed auth");
        }
    }

    async fn send_auth(&mut self, value: &Value) {
        let frame = self
            .link
            .send(FrameType::PairVerifyNext, value)
            .expect("auth frame");
        self.stream.write_all(&frame).await.expect("write auth");
    }
}

fn xid_of(value: &Value) -> u32 {
    value.get("_x").and_then(Value::as_int).expect("xid") as u32
}

fn response(xid: u32, identifier: &str, content: Value) -> Value {
    Value::object(vec![
        ("_i", Value::Str(identifier.into())),
        ("_t", Value::Int(commands::RESPONSE)),
        ("_c", content),
        ("_x", Value::Int(xid as i64)),
    ])
}

fn event(identifier: &str) -> Value {
    Value::object(vec![
        ("_i", Value::Str(identifier.into())),
        ("_t", Value::Int(commands::EVENT)),
    ])
}

async fn pair() -> (TcpListener, std::net::SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    (listener, addr)
}

#[tokio::test]
async fn replies_are_correlated_and_stale_frames_are_ignored() {
    let (listener, addr) = pair().await;
    let server = tokio::spawn(async move {
        let mut peer = Peer::accept(&listener).await;
        let request = peer.recv().await;
        assert_eq!(
            request.get("_i").and_then(Value::as_str),
            Some("_sessionStart")
        );
        let xid = xid_of(&request);
        // Событие без `_x` и запоздавший ответ чужого обмена не должны
        // подмениться ответом на наш запрос.
        peer.send(&event("_attention")).await;
        peer.send(&response(
            xid.wrapping_add(100),
            "_sessionStart",
            Value::object(vec![("_sid", Value::Int(1))]),
        ))
        .await;
        peer.send(&response(
            xid,
            "_sessionStart",
            Value::object(vec![("_sid", Value::Int(42))]),
        ))
        .await;
    });

    let stream = TcpStream::connect(addr).await.expect("connect");
    let mut session = Session::plain(stream);
    let sid = session
        .open_remote(7, DEADLINE)
        .await
        .expect("session start");
    assert_eq!(sid.remote, 42);
    assert_eq!(sid.local, 7);
    assert_eq!(sid.wire(), (42u64 << 32) | 7);
    server.await.expect("server");
}

#[tokio::test]
async fn a_response_for_another_request_is_rejected() {
    let (listener, addr) = pair().await;
    let server = tokio::spawn(async move {
        let mut peer = Peer::accept(&listener).await;
        let request = peer.recv().await;
        peer.send(&response(xid_of(&request), "_mcc", Value::empty_object()))
            .await;
    });

    let stream = TcpStream::connect(addr).await.expect("connect");
    let mut session = Session::plain(stream);
    let error = session
        .request("_hidC", Value::empty_object(), DEADLINE)
        .await
        .expect_err("a wrong response must fail");
    assert!(matches!(error, Error::Session(_)), "{error}");
    server.await.expect("server");
}

#[tokio::test]
async fn a_timeout_is_reported_and_the_session_stays_usable() {
    let (listener, addr) = pair().await;
    let server = tokio::spawn(async move {
        let mut peer = Peer::accept(&listener).await;
        let late = peer.recv().await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        peer.send(&response(
            xid_of(&late),
            "_sessionStart",
            Value::object(vec![("_sid", Value::Int(5))]),
        ))
        .await;
        let next = peer.recv().await;
        peer.send(&response(xid_of(&next), "_hidC", Value::empty_object()))
            .await;
    });

    let stream = TcpStream::connect(addr).await.expect("connect");
    let mut session = Session::plain(stream);
    let error = session
        .open_remote(1, Duration::from_millis(50))
        .await
        .expect_err("silence must time out");
    assert!(matches!(error, Error::Timeout), "{error}");
    assert!(session.is_open(), "a timeout is not a disconnect");
    session
        .request("_hidC", Value::empty_object(), DEADLINE)
        .await
        .expect("the session still works");
    server.await.expect("server");
}

#[tokio::test]
async fn a_disconnect_is_reported_and_blocks_further_requests() {
    let (listener, addr) = pair().await;
    let server = tokio::spawn(async move {
        let mut peer = Peer::accept(&listener).await;
        let request = peer.recv().await;
        assert_eq!(request.get("_i").and_then(Value::as_str), Some("_hidC"));
        drop(peer);
    });

    let stream = TcpStream::connect(addr).await.expect("connect");
    let mut session = Session::plain(stream);
    let error = session
        .request("_hidC", Value::empty_object(), DEADLINE)
        .await
        .expect_err("a closed connection must fail");
    assert!(matches!(error, Error::Closed), "{error}");
    assert!(!session.is_open());
    let again = session
        .request("_hidC", Value::empty_object(), DEADLINE)
        .await
        .expect_err("a dead session must not be reused");
    assert!(matches!(again, Error::Closed), "{again}");
    server.await.expect("server");
}

struct Accessory {
    identity: Identity,
    secret: StaticSecret,
}

impl Accessory {
    fn new(seed: u8) -> Self {
        Self {
            identity: Identity::from_seed("companion-accessory", &[seed; 32]),
            secret: StaticSecret::random_from_rng(rand::rngs::OsRng),
        }
    }

    /// Ответ M2 и ключи канала `(запись приёмника, чтение приёмника)`.
    fn reply(&self, start: &Value) -> (Value, [u8; 32], [u8; 32]) {
        let tlv =
            Tlv8::decode(start.get("_pd").and_then(Value::as_data).expect("M1")).expect("TLV");
        let client_public: [u8; 32] = tlv
            .get(tlv8::PUBLIC_KEY)
            .expect("client key")
            .try_into()
            .expect("32 bytes");
        let server_public = PublicKey::from(&self.secret).to_bytes();
        let shared = self
            .secret
            .diffie_hellman(&PublicKey::from(client_public))
            .to_bytes();
        let key = hkdf_sha512(
            &shared,
            b"Pair-Verify-Encrypt-Salt",
            b"Pair-Verify-Encrypt-Info",
        );
        let id = self.identity.pairing_id.as_bytes();
        let signature = self
            .identity
            .sign(&[server_public.as_slice(), id, &client_public].concat());
        let sub = Tlv8::new()
            .with(tlv8::IDENTIFIER, id.to_vec())
            .with(tlv8::SIGNATURE, signature.to_vec())
            .encode();
        let tlv = Tlv8::new()
            .with(tlv8::STATE, [2])
            .with(tlv8::PUBLIC_KEY, server_public.to_vec())
            .with(
                tlv8::ENCRYPTED_DATA,
                aead::seal(&key, b"PV-Msg02", &[], &sub),
            )
            .encode();
        (
            Value::object(vec![("_pd", Value::Data(tlv))]),
            hkdf_sha512(&shared, b"", b"ServerEncrypt-main"),
            hkdf_sha512(&shared, b"", b"ClientEncrypt-main"),
        )
    }
}

fn credentials(accessory: &Accessory) -> Credentials {
    let client = Identity::from_seed("companion-client", &[9; 32]);
    Credentials::new(
        b"client-id",
        client.seed(),
        accessory.identity.pairing_id.as_bytes().to_vec(),
        accessory.identity.public_key(),
    )
}

/// Один раунд pair-verify и зашифрованный запрос `_sessionStart`.
async fn serve_verified(peer: &mut Peer, accessory: &Accessory) {
    let m1 = peer.recv_auth(FrameType::PairVerifyStart).await;
    let (m2, server_write, server_read) = accessory.reply(&m1);
    peer.send_auth(&m2).await;

    let m3 = peer.recv_auth(FrameType::PairVerifyNext).await;
    assert!(m3.get("_pd").is_some());
    let m4 = Tlv8::new().with(tlv8::STATE, [4]).encode();
    peer.send_auth(&Value::object(vec![("_pd", Value::Data(m4))]))
        .await;

    // Дальше канал зашифрован; свежие счётчики, как и у клиента.
    peer.link = Link::encrypted(server_write, server_read);
    let start = peer.recv().await;
    assert_eq!(
        start.get("_i").and_then(Value::as_str),
        Some("_sessionStart")
    );
    peer.send(&response(
        xid_of(&start),
        "_sessionStart",
        Value::object(vec![("_sid", Value::Int(77))]),
    ))
    .await;
    let stop = peer.recv().await;
    assert_eq!(stop.get("_i").and_then(Value::as_str), Some("_sessionStop"));
    // pyatv 0.18.0 отправляет в `_sessionStop` склейку (remote << 32) | local.
    let sid = stop
        .get("_c")
        .and_then(|content| content.get("_sid"))
        .and_then(Value::as_int);
    assert_eq!(sid, Some(((77u64 << 32) | 3) as i64));
    peer.send(&response(
        xid_of(&stop),
        "_sessionStop",
        Value::empty_object(),
    ))
    .await;
}

#[tokio::test]
async fn pair_verify_enables_the_encrypted_channel_and_reconnect_starts_over() {
    let (listener, addr) = pair().await;
    let accessory = Accessory::new(21);
    let credentials = credentials(&accessory);
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let mut peer = Peer::accept(&listener).await;
            serve_verified(&mut peer, &accessory).await;
        }
    });

    for round in 0..2 {
        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut session = Session::connect(stream, &credentials, DEADLINE)
            .await
            .expect("pair-verify");
        assert!(session.is_encrypted(), "round {round} must encrypt");
        let sid = session
            .open_remote(3, DEADLINE)
            .await
            .expect("session start");
        assert_eq!(sid.remote, 77);
        session
            .close_remote(sid, DEADLINE)
            .await
            .expect("session stop");
        assert!(!session.is_open(), "close_remote shuts the stream down");
    }
    server.await.expect("server");
}

#[tokio::test]
async fn pair_verify_rejection_leaves_nothing_behind() {
    let (listener, addr) = pair().await;
    let server = tokio::spawn(async move {
        let mut peer = Peer::accept(&listener).await;
        let _m1 = peer.recv_auth(FrameType::PairVerifyStart).await;
        let error = Tlv8::new()
            .with(tlv8::STATE, [2])
            .with(tlv8::ERROR, [0x02])
            .encode();
        peer.send_auth(&Value::object(vec![("_pd", Value::Data(error))]))
            .await;
    });

    let accessory = Accessory::new(22);
    let stream = TcpStream::connect(addr).await.expect("connect");
    assert!(
        Session::connect(stream, &credentials(&accessory), DEADLINE)
            .await
            .is_err(),
        "a rejected pair-verify must not produce a session"
    );
    server.await.expect("server");
}

#[tokio::test]
async fn unexpected_auth_frame_type_is_refused() {
    let (listener, addr) = pair().await;
    let server = tokio::spawn(async move {
        let mut peer = Peer::accept(&listener).await;
        let _hello = peer.recv_auth(FrameType::PairVerifyStart).await;
        // Обычный кадр вместо PairVerifyNext: тип обязан проверяться до OPACK.
        let wire = peer
            .link
            .send(FrameType::Encrypted, &Value::empty_object())
            .expect("frame");
        peer.stream.write_all(&wire).await.expect("write");
    });

    let accessory = Accessory::new(23);
    let error = match Session::connect(
        TcpStream::connect(addr).await.expect("connect"),
        &credentials(&accessory),
        DEADLINE,
    )
    .await
    {
        Ok(_) => panic!("wrong auth frame type must fail"),
        Err(error) => error,
    };
    assert!(matches!(error, Error::Session(_)), "{error}");
    server.await.expect("server");
}

#[tokio::test]
async fn non_response_and_rejected_commands_fail_without_closing() {
    for rejected in [false, true] {
        let (listener, addr) = pair().await;
        let server = tokio::spawn(async move {
            let mut peer = Peer::accept(&listener).await;
            let request = peer.recv().await;
            let xid = xid_of(&request);
            let reply = if rejected {
                Value::object(vec![
                    ("_i", Value::Str("_hidC".into())),
                    ("_t", Value::Int(commands::RESPONSE)),
                    ("_x", Value::Int(xid as i64)),
                    ("_em", Value::Str("denied".into())),
                ])
            } else {
                // Тот же `_x` и `_i`, но это событие, а не ответ.
                Value::object(vec![
                    ("_i", Value::Str("_hidC".into())),
                    ("_t", Value::Int(commands::EVENT)),
                    ("_x", Value::Int(xid as i64)),
                ])
            };
            peer.send(&reply).await;
        });

        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut session = Session::plain(stream);
        let error = session
            .request("_hidC", Value::empty_object(), DEADLINE)
            .await
            .expect_err("protocol refusal must fail the request");
        assert!(matches!(error, Error::Session(_)), "{error}");
        assert!(session.is_open(), "protocol refusal is not a disconnect");
        server.await.expect("server");
    }
}
