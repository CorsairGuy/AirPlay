//! Одно соединение с отправителем на порту 7000: RTSP-запросы, сопряжение,
//! переход на шифрованный канал. Без ввода-вывода — байты на вход и выход.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::Result;
use crypto::Channel;
use crypto::kdf::ControlKeys;
use device::DeviceInfo;
use pair::hap::{PairSetup, PairVerify, SetupOutcome, VerifyOutcome};
use pair::legacy::{LegacyOutcome, LegacyServer};
use pair::{Identity, PairingStore};
use rtsp::{Request, Response};
use tracing::{debug, info, warn};

use crate::media::{Media, Player};

pub const SERVER: &str = "AirTunes/366.0";
const PUBLIC: &str = "ANNOUNCE, SETUP, RECORD, PAUSE, FLUSH, TEARDOWN, OPTIONS, GET_PARAMETER, SET_PARAMETER, POST, GET";
pub(crate) const BPLIST: &str = "application/x-apple-binary-plist";
pub(crate) const OCTETS: &str = "application/octet-stream";

/// Общее для всех соединений.
pub struct Context {
    pub identity: Identity,
    pub device: DeviceInfo,
    pub store: Box<dyn PairingStore>,
    /// PIN для постоянного HAP-сопряжения.
    pub pin: String,
    /// Только постоянное сопряжение по PIN, без transient.
    pub require_pin: bool,
    pub player: Arc<Player>,
    /// Объявление в Bonjour; обновляется, когда меняется состояние сопряжения.
    pub mdns: std::sync::Mutex<Option<mdns::Advertiser>>,
    pub port: u16,
    /// Зеркалирование экрана: куда отдавать видео.
    pub mirror: Option<mirror::VideoSinkFactory>,
}

impl Context {
    pub fn services(&self) -> Vec<mdns::Service> {
        let device = self.device();
        vec![
            mdns::Service {
                service_type: mdns::AIRPLAY,
                instance: device.name.clone(),
                port: self.port,
                txt: device.airplay_txt(),
            },
            mdns::Service {
                service_type: mdns::RAOP,
                instance: device.raop_instance(),
                port: self.port,
                txt: device.raop_txt(),
            },
        ]
    }

    /// После первого сопряжения в режиме PIN меняются флаги: объявим заново.
    fn paired(&self) {
        if !self.require_pin {
            return;
        }
        let services = self.services();
        if let Some(adv) = self.mdns.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
            && let Err(e) = adv.update(services)
        {
            warn!("Bonjour: {e}");
        }
    }

    /// Описание приёмника с флагами по текущему состоянию сопряжения.
    pub fn device(&self) -> DeviceInfo {
        let mut d = self.device.clone();
        if self.require_pin {
            d.require_pin(!self.store.is_empty());
        }
        d
    }
}

pub struct Session {
    ctx: Arc<Context>,
    peer: String,
    /// Сырые байты из сокета (шифротекст, если канал зашифрован).
    wire: Vec<u8>,
    /// Открытый текст, ожидающий разбора в запросы.
    plain: Vec<u8>,
    channel: Option<Channel>,
    setup: Option<PairSetup>,
    verify: Option<PairVerify>,
    legacy: Option<LegacyServer>,
    media: Media,
}

impl Session {
    pub fn new(ctx: Arc<Context>, peer: SocketAddr, local: IpAddr) -> Self {
        Self {
            media: Media::new(&ctx, local, peer.ip()),
            peer: peer.to_string(),
            ctx,
            wire: Vec::new(),
            plain: Vec::new(),
            channel: None,
            setup: None,
            verify: None,
            legacy: None,
        }
    }

    #[cfg(test)]
    fn encrypted(&self) -> bool {
        self.channel.is_some()
    }

    /// Принимает байты из сокета, дописывает в `out` то, что нужно отправить.
    pub fn on_data(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<()> {
        self.wire.extend_from_slice(data);
        loop {
            match &mut self.channel {
                Some(ch) => ch.open(&mut self.wire, &mut self.plain)?,
                None => self.plain.append(&mut self.wire),
            }
            let Some(req) = rtsp::decode_request(&mut self.plain)? else {
                return Ok(());
            };
            let (mut resp, keys) = self.handle(&req);
            resp.headers.insert("Server", SERVER);
            let mut bytes = Vec::new();
            resp.encode(&mut bytes);
            match &mut self.channel {
                Some(ch) => ch.seal(&bytes, out),
                None => out.extend_from_slice(&bytes),
            }
            if let Some(keys) = keys {
                // Ответ на последний шаг сопряжения ушёл открытым текстом,
                // всё дальше — по шифрованному каналу, в том числе то, что
                // уже лежит в буфере.
                self.channel = Some(Channel::new(keys));
                self.wire = std::mem::take(&mut self.plain);
                info!(peer = %self.peer, "канал зашифрован");
            }
        }
    }

    fn handle(&mut self, req: &Request) -> (Response, Option<ControlKeys>) {
        debug!(peer = %self.peer, "{} {}", req.method, req.uri);
        match (req.method.as_str(), req.path()) {
            ("OPTIONS", _) => {
                let mut resp = Response::to(req, 200);
                resp.headers.insert("Public", PUBLIC);
                (resp, None)
            }
            ("GET", "/info") => (
                Response::to(req, 200).with_body(BPLIST, self.ctx.device().info_plist()),
                None,
            ),
            ("POST", "/pair-pin-start") => {
                info!(peer = %self.peer, pin = %self.ctx.pin, "запрошен PIN для сопряжения");
                (Response::to(req, 200), None)
            }
            ("POST", "/pair-setup") if req.body.len() == 32 => (self.legacy_setup(req), None),
            ("POST", "/pair-setup") => self.hap_setup(req),
            ("POST", "/pair-verify") if is_legacy_verify(req) => (self.legacy_verify(req), None),
            ("POST", "/pair-verify") => self.hap_verify(req),
            _ => {
                let ctx = self.ctx.clone();
                if let Some(resp) = self.media.handle(&ctx, req) {
                    return (resp, None);
                }
                warn!(peer = %self.peer, "ещё не поддерживается: {} {}", req.method, req.uri);
                (Response::to(req, 501), None)
            }
        }
    }

    fn hap_setup(&mut self, req: &Request) -> (Response, Option<ControlKeys>) {
        let ctx = self.ctx.clone();
        let setup = self.setup.get_or_insert_with(|| {
            let setup = PairSetup::new(ctx.identity.clone(), ctx.pin.clone());
            if ctx.require_pin {
                setup.without_transient()
            } else {
                setup
            }
        });
        match setup.handle(&req.body, ctx.store.as_ref()) {
            Ok(SetupOutcome::Continue(body)) => {
                (Response::to(req, 200).with_body(OCTETS, body), None)
            }
            Ok(SetupOutcome::Transient {
                reply,
                keys,
                shared,
            }) => {
                info!(peer = %self.peer, "transient-сопряжение выполнено");
                self.media.set_shared(shared);
                self.setup = None;
                (Response::to(req, 200).with_body(OCTETS, reply), Some(keys))
            }
            Ok(SetupOutcome::Paired {
                reply,
                controller_id,
            }) => {
                info!(peer = %self.peer, controller = %String::from_utf8_lossy(&controller_id), "сопряжение по PIN выполнено");
                self.setup = None;
                ctx.paired();
                (Response::to(req, 200).with_body(OCTETS, reply), None)
            }
            Ok(SetupOutcome::Rejected(reply)) => {
                warn!(peer = %self.peer, "pair-setup отклонён: неверный PIN или подпись");
                self.setup = None;
                (Response::to(req, 200).with_body(OCTETS, reply), None)
            }
            Err(e) => {
                warn!(peer = %self.peer, "pair-setup: {e}");
                self.setup = None;
                (Response::to(req, 400), None)
            }
        }
    }

    fn hap_verify(&mut self, req: &Request) -> (Response, Option<ControlKeys>) {
        let ctx = self.ctx.clone();
        let verify = self
            .verify
            .get_or_insert_with(|| PairVerify::new(ctx.identity.clone()));
        match verify.handle(&req.body, ctx.store.as_ref()) {
            Ok(VerifyOutcome::Continue(body)) => {
                (Response::to(req, 200).with_body(OCTETS, body), None)
            }
            Ok(VerifyOutcome::Verified {
                reply,
                keys,
                shared,
            }) => {
                info!(peer = %self.peer, "pair-verify выполнен");
                self.media.set_shared(shared.to_vec());
                self.verify = None;
                (Response::to(req, 200).with_body(OCTETS, reply), Some(keys))
            }
            Ok(VerifyOutcome::Rejected(reply)) => {
                warn!(peer = %self.peer, "pair-verify отклонён: контроллер не сопряжён");
                self.verify = None;
                (Response::to(req, 200).with_body(OCTETS, reply), None)
            }
            Err(e) => {
                warn!(peer = %self.peer, "pair-verify: {e}");
                self.verify = None;
                (Response::to(req, 400), None)
            }
        }
    }

    fn legacy_setup(&mut self, req: &Request) -> Response {
        let ctx = self.ctx.clone();
        let legacy = self
            .legacy
            .get_or_insert_with(|| LegacyServer::new(ctx.identity.clone()));
        match legacy.pair_setup(&req.body) {
            Ok(body) => Response::to(req, 200).with_body(OCTETS, body),
            Err(e) => {
                warn!(peer = %self.peer, "legacy pair-setup: {e}");
                Response::to(req, 400)
            }
        }
    }

    fn legacy_verify(&mut self, req: &Request) -> Response {
        let ctx = self.ctx.clone();
        let legacy = self
            .legacy
            .get_or_insert_with(|| LegacyServer::new(ctx.identity.clone()));
        match legacy.pair_verify(&req.body) {
            Ok(LegacyOutcome::Continue(body)) => Response::to(req, 200).with_body(OCTETS, body),
            Ok(LegacyOutcome::Verified { shared }) => {
                info!(peer = %self.peer, "legacy pair-verify выполнен");
                self.media.set_legacy_shared(shared);
                Response::to(req, 200)
            }
            Err(e) => {
                warn!(peer = %self.peer, "legacy pair-verify: {e}");
                Response::to(req, 403)
            }
        }
    }
}

/// Legacy pair-verify: 68 байт, первые 4 — флаг шага и три нуля, без
/// заголовка `X-Apple-HKP`, который ставят клиенты HAP.
fn is_legacy_verify(req: &Request) -> bool {
    req.headers.get("X-Apple-HKP").is_none()
        && req.body.len() == 68
        && req.body[1..4] == [0, 0, 0]
        && req.body[0] <= 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::kdf::ControlKeys;
    use device::Features;
    use pair::MemoryStore;
    use pair::hap::{ControllerSetup, ControllerVerify};
    use pair::legacy::LegacyClient;

    fn ctx() -> Arc<Context> {
        let identity = Identity::from_seed("acc-uuid", &[5; 32]);
        Arc::new(Context {
            device: DeviceInfo {
                name: "Тест".into(),
                device_id: "02:00:00:00:00:01".into(),
                model: "AirPlay".into(),
                features: Features::AUDIO_RECEIVER,
                status_flags: 4,
                pairing_id: identity.pairing_id.clone(),
                public_key: identity.public_key(),
                source_version: "366.0".into(),
                display: None,
            },
            identity,
            store: Box::new(MemoryStore::default()),
            pin: "1234".into(),
            require_pin: false,
            mdns: std::sync::Mutex::new(None),
            port: 7000,
            mirror: None,
            player: Arc::new(Player::new(
                audio_stream::Config {
                    device: "null".into(),
                    ..Default::default()
                },
                timesync::TimeSync::idle(1),
            )),
        })
    }

    /// Минимальный клиент поверх сессии: отправляет запрос, читает ответ.
    struct Client {
        session: Session,
        channel: Option<Channel>,
        cseq: u32,
    }

    impl Client {
        fn new(ctx: Arc<Context>) -> Self {
            Self {
                session: Session::new(
                    ctx,
                    "127.0.0.1:50000".parse().unwrap(),
                    IpAddr::from([127, 0, 0, 1]),
                ),
                channel: None,
                cseq: 0,
            }
        }

        fn call(
            &mut self,
            method: &str,
            uri: &str,
            headers: &[(&str, &str)],
            body: Vec<u8>,
        ) -> Response {
            self.cseq += 1;
            let mut req = Request::new(method, uri, "RTSP/1.0");
            req.headers.insert("CSeq", self.cseq.to_string());
            for (k, v) in headers {
                req.headers.insert(*k, *v);
            }
            req.body = body;
            let mut bytes = Vec::new();
            req.encode(&mut bytes);
            let mut wire = Vec::new();
            match &mut self.channel {
                Some(ch) => ch.seal(&bytes, &mut wire),
                None => wire = bytes,
            }
            let mut out = Vec::new();
            self.session.on_data(&wire, &mut out).unwrap();
            let mut plain = Vec::new();
            match &mut self.channel {
                Some(ch) => ch.open(&mut out, &mut plain).unwrap(),
                None => plain = out,
            }
            let resp = rtsp::decode_response(&mut plain)
                .unwrap()
                .expect("полный ответ");
            assert_eq!(
                resp.headers.get("CSeq"),
                Some(self.cseq.to_string().as_str())
            );
            resp
        }

        fn encrypt(&mut self, keys: ControlKeys) {
            self.channel = Some(Channel::new(keys));
        }
    }

    #[test]
    fn info_is_a_binary_plist() {
        let mut c = Client::new(ctx());
        let resp = c.call("GET", "/info", &[], vec![]);
        assert_eq!(resp.status, 200);
        let v = plist::Value::from_reader(std::io::Cursor::new(resp.body)).unwrap();
        assert_eq!(v.as_dictionary().unwrap()["name"].as_string(), Some("Тест"));
    }

    /// Как iPhone: transient-сопряжение, SETUP сессии и буферизованного
    /// потока, шифрованный AAC по TCP, якорь, громкость, TEARDOWN.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn buffered_audio_end_to_end() {
        use plist::{Dictionary, Value};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let ctx = ctx();
        let mut c = Client::new(ctx.clone());
        let mut setup = ControllerSetup::new(Identity::from_seed("ctl", &[6; 32]), "", true);
        let hkp = [("X-Apple-HKP", "4")];
        let m2 = c.call("POST", "/pair-setup", &hkp, setup.m1()).body;
        let m4 = c
            .call("POST", "/pair-setup", &hkp, setup.m3(&m2).unwrap())
            .body;
        c.encrypt(setup.check_m4(&m4).unwrap().unwrap());
        let shared = setup.session_key().unwrap().to_vec();

        let to_plist = |d: Dictionary| {
            let mut out = Vec::new();
            Value::Dictionary(d).to_writer_binary(&mut out).unwrap();
            out
        };
        let reply = |r: Response| {
            assert_eq!(r.status, 200);
            plist::from_bytes::<Value>(&r.body)
                .unwrap()
                .into_dictionary()
                .unwrap()
        };

        // Сессия: канал событий присылает updateInfo.
        let mut d = Dictionary::new();
        d.insert("timingProtocol".into(), "PTP".into());
        let session = reply(c.call("SETUP", "rtsp://x/1", &[], to_plist(d)));
        assert!(session.contains_key("timingPeerInfo"));
        let event_port = session["eventPort"].as_unsigned_integer().unwrap() as u16;
        let mut events = tokio::net::TcpStream::connect(("127.0.0.1", event_port))
            .await
            .unwrap();
        let mut wire = vec![0u8; 4096];
        let n = events.read(&mut wire).await.unwrap();
        let mut wire = wire[..n].to_vec();
        let mut plain = Vec::new();
        Channel::new(ControlKeys::controller_events(&shared))
            .open(&mut wire, &mut plain)
            .unwrap();
        let info = rtsp::decode_request(&mut plain).unwrap().unwrap();
        assert_eq!(info.uri, "/command");

        // Поток AAC 44,1 кГц.
        let shk = [3u8; 32];
        let mut s = Dictionary::new();
        s.insert("type".into(), Value::Integer(103.into()));
        s.insert("shk".into(), Value::Data(shk.to_vec()));
        let mut d = Dictionary::new();
        d.insert("streams".into(), Value::Array(vec![s.into()]));
        let streams = reply(c.call("SETUP", "rtsp://x/1", &[], to_plist(d)));
        let stream = streams["streams"].as_array().unwrap()[0]
            .as_dictionary()
            .unwrap();
        let data_port = stream["dataPort"].as_unsigned_integer().unwrap() as u16;

        let frames = adts_frames(include_bytes!(
            "../../../crates/codec/tests/data/sine440.adts"
        ));
        let cipher = rtp::AudioCipher::new(&shk);
        let mut wire = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            let h = rtp::Header {
                marker: false,
                payload_type: 0x67,
                seq: i as u16,
                timestamp: 1000 + i as u32 * 1024,
                ssrc: 0x1600_0000,
            };
            rtp::frame_buffered(&cipher.seal(&h, f, i as u64), &mut wire);
        }
        let mut data = tokio::net::TcpStream::connect(("127.0.0.1", data_port))
            .await
            .unwrap();
        data.write_all(&wire).await.unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while ctx.player.active_packets() != Some(frames.len()) {
            assert!(std::time::Instant::now() < deadline, "пакеты не дошли");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        assert_eq!(c.call("RECORD", "rtsp://x/1", &[], vec![]).status, 200);
        let volume = c.call(
            "SET_PARAMETER",
            "rtsp://x/1",
            &[("Content-Type", "text/parameters")],
            b"volume: -10.0\r\n".to_vec(),
        );
        assert_eq!(volume.status, 200);
        let mut d = Dictionary::new();
        d.insert("rate".into(), Value::Real(1.0));
        d.insert("rtpTime".into(), Value::Integer(1000.into()));
        d.insert("networkTimeSecs".into(), Value::Integer(1.into()));
        d.insert("networkTimeFrac".into(), Value::Integer(0.into()));
        d.insert("networkTimeTimelineID".into(), Value::Integer(42.into()));
        let anchor = c.call("SETRATEANCHORTIME", "rtsp://x/1", &[], to_plist(d));
        assert_eq!(anchor.status, 200);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while ctx.player.active_packets() != Some(0) {
            assert!(std::time::Instant::now() < deadline, "звук не проигран");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(c.call("TEARDOWN", "rtsp://x/1", &[], vec![]).status, 200);
        assert_eq!(ctx.player.active_packets(), None);
    }

    fn adts_frames(mut data: &[u8]) -> Vec<&[u8]> {
        let mut frames = Vec::new();
        while data.len() > 7 && data[0] == 0xFF && data[1] & 0xF0 == 0xF0 {
            let len = ((data[3] as usize & 3) << 11)
                | ((data[4] as usize) << 3)
                | (data[5] as usize >> 5);
            let header = if data[1] & 1 == 0 { 9 } else { 7 };
            frames.push(&data[header..len]);
            data = &data[len..];
        }
        frames
    }

    /// Видео сохраняется для проверки в тесте.
    #[derive(Default, Clone)]
    struct Recorded(Arc<std::sync::Mutex<Vec<Packet>>>);

    /// `true` — параметры декодера, `false` — кадр.
    type Packet = (bool, Vec<u8>);

    impl mirror::VideoSink for Recorded {
        fn config(&mut self, _codec: mirror::Codec, record: &[u8]) {
            self.0.lock().unwrap().push((true, record.to_vec()));
        }
        fn frame(&mut self, _pts_ns: u64, nals: &[u8]) {
            self.0.lock().unwrap().push((false, nals.to_vec()));
        }
    }

    /// Зеркалирование: FairPlay → SETUP с ekey и NTP → поток 110 → кадры,
    /// расшифрованные ключом, выведенным из ekey.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mirroring_end_to_end() {
        use plist::{Dictionary, Value};
        use tokio::io::AsyncWriteExt;

        let recorded = Recorded::default();
        let sink = recorded.clone();
        let mut ctx = ctx();
        Arc::get_mut(&mut ctx).unwrap().mirror = Some(mirror::VideoSinkFactory::new(move |_| {
            Box::new(sink.clone())
        }));
        let mut c = Client::new(ctx);

        // FairPlay: векторы из тестов ap_fairplay, ekey → 51e601d6...
        let mut m1 = [0u8; 16];
        m1[4] = 3;
        m1[14] = 1;
        assert_eq!(c.call("POST", "/fp-setup", &[], m1.to_vec()).status, 200);
        let mut keymsg = [0x41u8; 164];
        keymsg[..5].copy_from_slice(b"FPLY\x03");
        keymsg[12] = 1;
        assert_eq!(
            c.call("POST", "/fp-setup", &[], keymsg.to_vec()).status,
            200
        );
        let mut ekey = [0x42u8; 72];
        ekey[..4].copy_from_slice(b"FPLY");
        let aes: [u8; 16] = hex::decode("51e601d65942f9bd660b57bf98800cdf")
            .unwrap()
            .try_into()
            .unwrap();

        let to_plist = |d: Dictionary| {
            let mut out = Vec::new();
            Value::Dictionary(d).to_writer_binary(&mut out).unwrap();
            out
        };
        let mut d = Dictionary::new();
        d.insert("ekey".into(), Value::Data(ekey.to_vec()));
        d.insert("eiv".into(), Value::Data(vec![0; 16]));
        d.insert("timingProtocol".into(), "NTP".into());
        d.insert("timingPort".into(), Value::Integer(0.into()));
        let r = c.call("SETUP", "rtsp://x/1", &[], to_plist(d));
        assert_eq!(r.status, 200);
        let session = plist::from_bytes::<Value>(&r.body)
            .unwrap()
            .into_dictionary()
            .unwrap();
        assert!(session["timingPort"].as_unsigned_integer().unwrap() > 0);

        let id = 7_u64;
        let mut s = Dictionary::new();
        s.insert("type".into(), Value::Integer(110.into()));
        s.insert("streamConnectionID".into(), Value::Integer(id.into()));
        let mut d = Dictionary::new();
        d.insert("streams".into(), Value::Array(vec![s.into()]));
        let r = c.call("SETUP", "rtsp://x/1", &[], to_plist(d));
        assert_eq!(r.status, 200);
        let streams = plist::from_bytes::<Value>(&r.body)
            .unwrap()
            .into_dictionary()
            .unwrap();
        let port = streams["streams"].as_array().unwrap()[0]
            .as_dictionary()
            .unwrap()["dataPort"]
            .as_unsigned_integer()
            .unwrap() as u16;

        let packet = |kind: u16, body: &[u8]| {
            let mut p = vec![0u8; mirror::HEADER_LEN];
            p[0..4].copy_from_slice(&(body.len() as u32).to_le_bytes());
            p[4..6].copy_from_slice(&kind.to_le_bytes());
            p.extend_from_slice(body);
            p
        };
        let config = [
            1u8, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 2, 0x67, 0x64, 1, 0, 1, 0x68,
        ];
        let frames: Vec<Vec<u8>> = vec![
            vec![0, 0, 0, 3, 0x65, 1, 2],
            vec![0, 0, 0, 20, 0x41].into_iter().chain(0..20).collect(),
        ];
        let (key, iv) = mirror::stream_key_iv(&aes, id);
        let mut cipher = crypto::ctr::aes128_ctr(&key, &iv);
        let mut wire = packet(1, &config);
        for f in &frames {
            let mut enc = f.clone();
            crypto::ctr::apply(&mut cipher, &mut enc);
            wire.extend(packet(0, &enc));
        }
        let mut conn = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        conn.write_all(&wire).await.unwrap();
        drop(conn);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while recorded.0.lock().unwrap().len() < 3 {
            assert!(std::time::Instant::now() < deadline, "видео не дошло");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let got = recorded.0.lock().unwrap().clone();
        assert_eq!(got[0], (true, config.to_vec()));
        assert_eq!(got[1], (false, frames[0].clone()));
        assert_eq!(got[2], (false, frames[1].clone()));
    }

    #[test]
    fn transient_pairing_switches_to_encrypted_channel() {
        let mut c = Client::new(ctx());
        let mut setup = ControllerSetup::new(Identity::from_seed("ctl", &[6; 32]), "", true);
        let hkp = [("X-Apple-HKP", "4")];
        let m2 = c.call("POST", "/pair-setup", &hkp, setup.m1()).body;
        let m4 = c
            .call("POST", "/pair-setup", &hkp, setup.m3(&m2).unwrap())
            .body;
        let keys = setup.check_m4(&m4).unwrap().unwrap();
        assert!(c.session.encrypted());
        c.encrypt(keys);
        assert_eq!(c.call("GET", "/info", &[], vec![]).status, 200);
    }

    #[test]
    fn pin_pairing_then_verify_on_new_connection() {
        let ctx = ctx();
        let ctl = Identity::from_seed("ctl", &[6; 32]);
        let hkp = [("X-Apple-HKP", "3")];

        let mut c = Client::new(ctx.clone());
        let mut setup = ControllerSetup::new(ctl.clone(), "1234", false);
        let m2 = c.call("POST", "/pair-setup", &hkp, setup.m1()).body;
        let m4 = c
            .call("POST", "/pair-setup", &hkp, setup.m3(&m2).unwrap())
            .body;
        assert!(setup.check_m4(&m4).unwrap().is_none());
        let m6 = c
            .call("POST", "/pair-setup", &hkp, setup.m5().unwrap())
            .body;
        let acc = setup.check_m6(&m6).unwrap();
        assert!(!c.session.encrypted());

        let mut c = Client::new(ctx);
        let mut verify = ControllerVerify::new(ctl, acc);
        let m2 = c.call("POST", "/pair-verify", &hkp, verify.m1()).body;
        let m4 = c
            .call("POST", "/pair-verify", &hkp, verify.m3(&m2).unwrap())
            .body;
        c.encrypt(verify.check_m4(&m4).unwrap());
        assert_eq!(c.call("GET", "/info", &[], vec![]).status, 200);
    }

    #[test]
    fn pin_only_mode() {
        let mut ctx = ctx();
        Arc::get_mut(&mut ctx).unwrap().require_pin = true;
        let info = |c: &mut Client| {
            let body = c.call("GET", "/info", &[], vec![]).body;
            let d = plist::from_bytes::<plist::Value>(&body)
                .unwrap()
                .into_dictionary()
                .unwrap();
            let flags = d["statusFlags"].as_unsigned_integer().unwrap();
            let features = d["features"].as_unsigned_integer().unwrap();
            (flags, features & Features::SYSTEM_PAIRING != 0)
        };
        let hkp = [("X-Apple-HKP", "3")];
        let mut c = Client::new(ctx.clone());
        assert_eq!(info(&mut c), (0x204, false));

        // transient отклоняется сразу на M2.
        let setup = ControllerSetup::new(Identity::from_seed("ctl", &[6; 32]), "", true);
        let m2 = c.call("POST", "/pair-setup", &hkp, setup.m1()).body;
        assert!(
            pair::tlv8::Tlv8::decode(&m2)
                .unwrap()
                .get(pair::tlv8::ERROR)
                .is_some()
        );

        let mut setup = ControllerSetup::new(Identity::from_seed("ctl", &[6; 32]), "1234", false);
        let m2 = c.call("POST", "/pair-setup", &hkp, setup.m1()).body;
        let m4 = c
            .call("POST", "/pair-setup", &hkp, setup.m3(&m2).unwrap())
            .body;
        setup.check_m4(&m4).unwrap();
        c.call("POST", "/pair-setup", &hkp, setup.m5().unwrap());
        assert_eq!(info(&mut c), (0x604, false));
    }

    #[test]
    fn legacy_pairing() {
        let mut c = Client::new(ctx());
        let client = LegacyClient::new(Identity::from_seed("ctl", &[7; 32]));
        let server_ed: [u8; 32] = c
            .call("POST", "/pair-setup", &[], client.pair_setup())
            .body
            .try_into()
            .unwrap();
        let reply = c.call("POST", "/pair-verify", &[], client.verify1()).body;
        let (msg2, _) = client.verify2(&server_ed, &reply).unwrap();
        assert_eq!(c.call("POST", "/pair-verify", &[], msg2).status, 200);
    }

    #[test]
    fn unknown_request_is_not_implemented() {
        let mut c = Client::new(ctx());
        assert_eq!(c.call("ANNOUNCE", "rtsp://x/1", &[], vec![]).status, 501);
    }
}
