//! Одно соединение с отправителем на порту 7000: RTSP-запросы, сопряжение,
//! переход на шифрованный канал. Без ввода-вывода — байты на вход и выход.

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

pub const SERVER: &str = "AirTunes/366.0";
const PUBLIC: &str = "ANNOUNCE, SETUP, RECORD, PAUSE, FLUSH, TEARDOWN, OPTIONS, GET_PARAMETER, SET_PARAMETER, POST, GET";
const BPLIST: &str = "application/x-apple-binary-plist";
const OCTETS: &str = "application/octet-stream";

/// Общее для всех соединений.
pub struct Context {
    pub identity: Identity,
    pub device: DeviceInfo,
    pub store: Box<dyn PairingStore>,
    /// PIN для постоянного HAP-сопряжения.
    pub pin: String,
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
}

impl Session {
    pub fn new(ctx: Arc<Context>, peer: String) -> Self {
        Self {
            ctx,
            peer,
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
                Response::to(req, 200).with_body(BPLIST, self.ctx.device.info_plist()),
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
                warn!(peer = %self.peer, "ещё не поддерживается: {} {}", req.method, req.uri);
                (Response::to(req, 501), None)
            }
        }
    }

    fn hap_setup(&mut self, req: &Request) -> (Response, Option<ControlKeys>) {
        let ctx = self.ctx.clone();
        let setup = self
            .setup
            .get_or_insert_with(|| PairSetup::new(ctx.identity.clone(), ctx.pin.clone()));
        match setup.handle(&req.body, ctx.store.as_ref()) {
            Ok(SetupOutcome::Continue(body)) => {
                (Response::to(req, 200).with_body(OCTETS, body), None)
            }
            Ok(SetupOutcome::Transient { reply, keys }) => {
                info!(peer = %self.peer, "transient-сопряжение выполнено");
                self.setup = None;
                (Response::to(req, 200).with_body(OCTETS, reply), Some(keys))
            }
            Ok(SetupOutcome::Paired {
                reply,
                controller_id,
            }) => {
                info!(peer = %self.peer, controller = %String::from_utf8_lossy(&controller_id), "сопряжение по PIN выполнено");
                self.setup = None;
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
            Ok(VerifyOutcome::Verified { reply, keys, .. }) => {
                info!(peer = %self.peer, "pair-verify выполнен");
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
            Ok(LegacyOutcome::Verified { .. }) => {
                info!(peer = %self.peer, "legacy pair-verify выполнен");
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
            },
            identity,
            store: Box::new(MemoryStore::default()),
            pin: "1234".into(),
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
                session: Session::new(ctx, "test".into()),
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
        assert_eq!(c.call("SETUP", "rtsp://x/1", &[], vec![]).status, 501);
    }
}
