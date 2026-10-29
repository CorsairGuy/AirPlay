//! HomeKit pairing (HAP), как его использует AirPlay 2.
//!
//! Серверная сторона (приёмник): [`PairSetup`] и [`PairVerify`].
//! Клиентская сторона (отправитель): [`ControllerSetup`] и [`ControllerVerify`].

use crypto::aead;
use crypto::kdf::{ControlKeys, hkdf_sha512};
use crypto::srp;
use rand::RngCore;
use rand::rngs::OsRng;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::Error;
use crate::identity::{Identity, verify_signature};
use crate::store::PairingStore;
use crate::tlv8::{self, Tlv8};

/// PIN, который iOS использует для transient-сопряжения.
pub const TRANSIENT_PIN: &str = "3939";
const USERNAME: &[u8] = b"Pair-Setup";

fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    OsRng.fill_bytes(&mut out);
    out
}

fn setup_key(session_key: &[u8]) -> [u8; 32] {
    hkdf_sha512(
        session_key,
        b"Pair-Setup-Encrypt-Salt",
        b"Pair-Setup-Encrypt-Info",
    )
}

fn error_reply(state: u8) -> Vec<u8> {
    Tlv8::new()
        .with(tlv8::STATE, [state])
        .with(tlv8::ERROR, [tlv8::ERROR_AUTHENTICATION])
        .encode()
}

/// Что сделать после ответа на шаг pair-setup.
pub enum SetupOutcome {
    /// Отправить ответ и ждать следующий шаг.
    Continue(Vec<u8>),
    /// Transient-сопряжение завершено: отправить ответ открытым текстом,
    /// затем включить шифрование канала этими ключами.
    Transient { reply: Vec<u8>, keys: ControlKeys },
    /// Постоянное сопряжение завершено, контроллер сохранён в хранилище.
    Paired {
        reply: Vec<u8>,
        controller_id: Vec<u8>,
    },
    /// Ошибка проверки (например, неверный PIN): отправить ответ и забыть сессию.
    Rejected(Vec<u8>),
}

/// Сервер pair-setup. Один экземпляр на соединение.
pub struct PairSetup {
    identity: Identity,
    pin: String,
    srp: Option<srp::Server>,
    transient: bool,
    session_key: Option<Vec<u8>>,
}

impl PairSetup {
    /// `pin` — PIN для постоянного сопряжения; transient всегда использует 3939.
    pub fn new(identity: Identity, pin: impl Into<String>) -> Self {
        Self {
            identity,
            pin: pin.into(),
            srp: None,
            transient: false,
            session_key: None,
        }
    }

    pub fn handle(&mut self, body: &[u8], store: &dyn PairingStore) -> Result<SetupOutcome, Error> {
        let msg = Tlv8::decode(body)?;
        match msg.state()? {
            1 => self.m1(&msg),
            3 => self.m3(&msg),
            5 => self.m5(&msg, store),
            s => Err(Error::UnexpectedState(s)),
        }
    }

    fn m1(&mut self, msg: &Tlv8) -> Result<SetupOutcome, Error> {
        self.transient = msg.get_uint(tlv8::FLAGS).unwrap_or(0) & tlv8::FLAG_TRANSIENT != 0;
        let pin = if self.transient {
            TRANSIENT_PIN
        } else {
            &self.pin
        };
        let salt: [u8; 16] = random();
        let server = srp::Server::<sha2::Sha512>::new(
            srp::Group::rfc5054_3072(),
            USERNAME,
            pin.as_bytes(),
            &salt,
            &random::<64>(),
        );
        let reply = Tlv8::new()
            .with(tlv8::STATE, [2])
            .with(tlv8::SALT, salt)
            .with(tlv8::PUBLIC_KEY, server.public_key())
            .encode();
        self.srp = Some(server);
        self.session_key = None;
        Ok(SetupOutcome::Continue(reply))
    }

    fn m3(&mut self, msg: &Tlv8) -> Result<SetupOutcome, Error> {
        let server = self.srp.take().ok_or(Error::UnexpectedState(3))?;
        let a = msg.require(tlv8::PUBLIC_KEY, "нет PublicKey в M3")?;
        let proof = msg.require(tlv8::PROOF, "нет Proof в M3")?;
        let Ok(verified) = server.verify(a, proof) else {
            return Ok(SetupOutcome::Rejected(error_reply(4)));
        };
        let reply = Tlv8::new()
            .with(tlv8::STATE, [4])
            .with(tlv8::PROOF, verified.server_proof)
            .encode();
        if self.transient {
            let keys = ControlKeys::accessory(&verified.session_key);
            return Ok(SetupOutcome::Transient { reply, keys });
        }
        self.session_key = Some(verified.session_key);
        Ok(SetupOutcome::Continue(reply))
    }

    fn m5(&mut self, msg: &Tlv8, store: &dyn PairingStore) -> Result<SetupOutcome, Error> {
        let session_key = self.session_key.take().ok_or(Error::UnexpectedState(5))?;
        let key = setup_key(&session_key);
        let encrypted = msg.require(tlv8::ENCRYPTED_DATA, "нет EncryptedData в M5")?;
        let Ok(plain) = aead::open(&key, b"PS-Msg05", &[], encrypted) else {
            return Ok(SetupOutcome::Rejected(error_reply(6)));
        };
        let sub = Tlv8::decode(&plain)?;
        let id = sub.require(tlv8::IDENTIFIER, "нет Identifier в M5")?;
        let ltpk = sub.require(tlv8::PUBLIC_KEY, "нет PublicKey в M5")?;
        let signature = sub.require(tlv8::SIGNATURE, "нет Signature в M5")?;
        let device_x = hkdf_sha512(
            &session_key,
            b"Pair-Setup-Controller-Sign-Salt",
            b"Pair-Setup-Controller-Sign-Info",
        );
        let info = [device_x.as_slice(), id, ltpk].concat();
        if verify_signature(ltpk, &info, signature).is_err() {
            return Ok(SetupOutcome::Rejected(error_reply(6)));
        }
        store.add(
            id,
            ltpk.try_into()
                .expect("длина проверена при проверке подписи"),
        );

        let acc_x = hkdf_sha512(
            &session_key,
            b"Pair-Setup-Accessory-Sign-Salt",
            b"Pair-Setup-Accessory-Sign-Info",
        );
        let our_id = self.identity.pairing_id.as_bytes();
        let our_pk = self.identity.public_key();
        let our_sig = self
            .identity
            .sign(&[acc_x.as_slice(), our_id, &our_pk].concat());
        let sub = Tlv8::new()
            .with(tlv8::IDENTIFIER, our_id)
            .with(tlv8::PUBLIC_KEY, our_pk)
            .with(tlv8::SIGNATURE, our_sig)
            .encode();
        let reply = Tlv8::new()
            .with(tlv8::STATE, [6])
            .with(
                tlv8::ENCRYPTED_DATA,
                aead::seal(&key, b"PS-Msg06", &[], &sub),
            )
            .encode();
        Ok(SetupOutcome::Paired {
            reply,
            controller_id: id.to_vec(),
        })
    }
}

pub enum VerifyOutcome {
    Continue(Vec<u8>),
    /// Проверка пройдена: отправить ответ открытым текстом, затем включить шифрование.
    Verified {
        reply: Vec<u8>,
        keys: ControlKeys,
        shared: [u8; 32],
    },
    Rejected(Vec<u8>),
}

/// Сервер pair-verify. Один экземпляр на соединение.
pub struct PairVerify {
    identity: Identity,
    pending: Option<VerifyPending>,
}

struct VerifyPending {
    shared: [u8; 32],
    session_key: [u8; 32],
    client_pub: [u8; 32],
    server_pub: [u8; 32],
}

impl PairVerify {
    pub fn new(identity: Identity) -> Self {
        Self {
            identity,
            pending: None,
        }
    }

    pub fn handle(
        &mut self,
        body: &[u8],
        store: &dyn PairingStore,
    ) -> Result<VerifyOutcome, Error> {
        let msg = Tlv8::decode(body)?;
        match msg.state()? {
            1 => self.m1(&msg),
            3 => self.m3(&msg, store),
            s => Err(Error::UnexpectedState(s)),
        }
    }

    fn m1(&mut self, msg: &Tlv8) -> Result<VerifyOutcome, Error> {
        let client_pub: [u8; 32] = msg
            .require(tlv8::PUBLIC_KEY, "нет PublicKey в M1")?
            .try_into()
            .map_err(|_| Error::Malformed("длина ключа X25519"))?;
        let secret = StaticSecret::random_from_rng(OsRng);
        let server_pub = PublicKey::from(&secret).to_bytes();
        let shared = secret
            .diffie_hellman(&PublicKey::from(client_pub))
            .to_bytes();
        let session_key = hkdf_sha512(
            &shared,
            b"Pair-Verify-Encrypt-Salt",
            b"Pair-Verify-Encrypt-Info",
        );

        let our_id = self.identity.pairing_id.as_bytes();
        let signature = self
            .identity
            .sign(&[server_pub.as_slice(), our_id, &client_pub].concat());
        let sub = Tlv8::new()
            .with(tlv8::IDENTIFIER, our_id)
            .with(tlv8::SIGNATURE, signature)
            .encode();
        let reply = Tlv8::new()
            .with(tlv8::STATE, [2])
            .with(tlv8::PUBLIC_KEY, server_pub)
            .with(
                tlv8::ENCRYPTED_DATA,
                aead::seal(&session_key, b"PV-Msg02", &[], &sub),
            )
            .encode();
        self.pending = Some(VerifyPending {
            shared,
            session_key,
            client_pub,
            server_pub,
        });
        Ok(VerifyOutcome::Continue(reply))
    }

    fn m3(&mut self, msg: &Tlv8, store: &dyn PairingStore) -> Result<VerifyOutcome, Error> {
        let p = self.pending.take().ok_or(Error::UnexpectedState(3))?;
        let encrypted = msg.require(tlv8::ENCRYPTED_DATA, "нет EncryptedData в M3")?;
        let Ok(plain) = aead::open(&p.session_key, b"PV-Msg03", &[], encrypted) else {
            return Ok(VerifyOutcome::Rejected(error_reply(4)));
        };
        let sub = Tlv8::decode(&plain)?;
        let id = sub.require(tlv8::IDENTIFIER, "нет Identifier в M3")?;
        let signature = sub.require(tlv8::SIGNATURE, "нет Signature в M3")?;
        let Some(ltpk) = store.get(id) else {
            return Ok(VerifyOutcome::Rejected(error_reply(4)));
        };
        let info = [p.client_pub.as_slice(), id, &p.server_pub].concat();
        if verify_signature(&ltpk, &info, signature).is_err() {
            return Ok(VerifyOutcome::Rejected(error_reply(4)));
        }
        Ok(VerifyOutcome::Verified {
            reply: Tlv8::new().with(tlv8::STATE, [4]).encode(),
            keys: ControlKeys::accessory(&p.shared),
            shared: p.shared,
        })
    }
}

/// Клиент pair-setup (сторона отправителя).
pub struct ControllerSetup {
    identity: Identity,
    pin: String,
    transient: bool,
    proof: Option<srp::ClientProof>,
}

/// Данные о сопряжённом приёмнике после M6.
pub struct Accessory {
    pub id: Vec<u8>,
    pub public_key: [u8; 32],
}

impl ControllerSetup {
    pub fn new(identity: Identity, pin: impl Into<String>, transient: bool) -> Self {
        let pin = if transient {
            TRANSIENT_PIN.into()
        } else {
            pin.into()
        };
        Self {
            identity,
            pin,
            transient,
            proof: None,
        }
    }

    pub fn m1(&self) -> Vec<u8> {
        let mut tlv = Tlv8::new().with(tlv8::STATE, [1]).with(tlv8::METHOD, [0]);
        if self.transient {
            tlv = tlv.with(tlv8::FLAGS, [tlv8::FLAG_TRANSIENT as u8]);
        }
        tlv.encode()
    }

    /// Обрабатывает M2, возвращает M3.
    pub fn m3(&mut self, m2: &[u8]) -> Result<Vec<u8>, Error> {
        let msg = Tlv8::decode(m2)?;
        let salt = msg.require(tlv8::SALT, "нет Salt в M2")?;
        let b = msg.require(tlv8::PUBLIC_KEY, "нет PublicKey в M2")?;
        let client = srp::Client::<sha2::Sha512>::new(
            srp::Group::rfc5054_3072(),
            USERNAME,
            self.pin.as_bytes(),
            &random::<64>(),
        );
        let proof = client.process(salt, b)?;
        let reply = Tlv8::new()
            .with(tlv8::STATE, [3])
            .with(tlv8::PUBLIC_KEY, proof.public_key.clone())
            .with(tlv8::PROOF, proof.proof.clone())
            .encode();
        self.proof = Some(proof);
        Ok(reply)
    }

    /// Проверяет M4. Для transient возвращает ключи канала, иначе `None`
    /// (дальше нужно отправить [`Self::m5`]).
    pub fn check_m4(&self, m4: &[u8]) -> Result<Option<ControlKeys>, Error> {
        let msg = Tlv8::decode(m4)?;
        if msg.get(tlv8::ERROR).is_some() {
            return Err(Error::Auth("приёмник отклонил PIN"));
        }
        let proof = self.proof.as_ref().ok_or(Error::UnexpectedState(4))?;
        proof.verify_server(msg.require(tlv8::PROOF, "нет Proof в M4")?)?;
        Ok(self
            .transient
            .then(|| ControlKeys::controller(&proof.session_key)))
    }

    pub fn m5(&self) -> Result<Vec<u8>, Error> {
        let proof = self.proof.as_ref().ok_or(Error::UnexpectedState(5))?;
        let device_x = hkdf_sha512(
            &proof.session_key,
            b"Pair-Setup-Controller-Sign-Salt",
            b"Pair-Setup-Controller-Sign-Info",
        );
        let id = self.identity.pairing_id.as_bytes();
        let pk = self.identity.public_key();
        let sub = Tlv8::new()
            .with(tlv8::IDENTIFIER, id)
            .with(tlv8::PUBLIC_KEY, pk)
            .with(
                tlv8::SIGNATURE,
                self.identity.sign(&[device_x.as_slice(), id, &pk].concat()),
            )
            .encode();
        Ok(Tlv8::new()
            .with(tlv8::STATE, [5])
            .with(
                tlv8::ENCRYPTED_DATA,
                aead::seal(&setup_key(&proof.session_key), b"PS-Msg05", &[], &sub),
            )
            .encode())
    }

    pub fn check_m6(&self, m6: &[u8]) -> Result<Accessory, Error> {
        let proof = self.proof.as_ref().ok_or(Error::UnexpectedState(6))?;
        let msg = Tlv8::decode(m6)?;
        if msg.get(tlv8::ERROR).is_some() {
            return Err(Error::Auth("приёмник отклонил M5"));
        }
        let plain = aead::open(
            &setup_key(&proof.session_key),
            b"PS-Msg06",
            &[],
            msg.require(tlv8::ENCRYPTED_DATA, "нет EncryptedData в M6")?,
        )?;
        let sub = Tlv8::decode(&plain)?;
        let id = sub.require(tlv8::IDENTIFIER, "нет Identifier в M6")?;
        let pk = sub.require(tlv8::PUBLIC_KEY, "нет PublicKey в M6")?;
        let sig = sub.require(tlv8::SIGNATURE, "нет Signature в M6")?;
        let acc_x = hkdf_sha512(
            &proof.session_key,
            b"Pair-Setup-Accessory-Sign-Salt",
            b"Pair-Setup-Accessory-Sign-Info",
        );
        verify_signature(pk, &[acc_x.as_slice(), id, pk].concat(), sig)?;
        Ok(Accessory {
            id: id.to_vec(),
            public_key: pk.try_into().expect("длина проверена"),
        })
    }
}

/// Клиент pair-verify (сторона отправителя).
pub struct ControllerVerify {
    identity: Identity,
    accessory: Accessory,
    secret: StaticSecret,
    shared: Option<[u8; 32]>,
}

impl ControllerVerify {
    pub fn new(identity: Identity, accessory: Accessory) -> Self {
        Self {
            identity,
            accessory,
            secret: StaticSecret::random_from_rng(OsRng),
            shared: None,
        }
    }

    pub fn m1(&self) -> Vec<u8> {
        Tlv8::new()
            .with(tlv8::STATE, [1])
            .with(tlv8::PUBLIC_KEY, PublicKey::from(&self.secret).to_bytes())
            .encode()
    }

    /// Проверяет M2 и возвращает M3.
    pub fn m3(&mut self, m2: &[u8]) -> Result<Vec<u8>, Error> {
        let msg = Tlv8::decode(m2)?;
        let server_pub: [u8; 32] = msg
            .require(tlv8::PUBLIC_KEY, "нет PublicKey в M2")?
            .try_into()
            .map_err(|_| Error::Malformed("длина ключа X25519"))?;
        let shared = self
            .secret
            .diffie_hellman(&PublicKey::from(server_pub))
            .to_bytes();
        let key = hkdf_sha512(
            &shared,
            b"Pair-Verify-Encrypt-Salt",
            b"Pair-Verify-Encrypt-Info",
        );
        let plain = aead::open(
            &key,
            b"PV-Msg02",
            &[],
            msg.require(tlv8::ENCRYPTED_DATA, "нет EncryptedData в M2")?,
        )?;
        let sub = Tlv8::decode(&plain)?;
        let id = sub.require(tlv8::IDENTIFIER, "нет Identifier в M2")?;
        if id != self.accessory.id {
            return Err(Error::Auth("чужой приёмник"));
        }
        let client_pub = PublicKey::from(&self.secret).to_bytes();
        let info = [server_pub.as_slice(), id, &client_pub].concat();
        verify_signature(
            &self.accessory.public_key,
            &info,
            sub.require(tlv8::SIGNATURE, "нет Signature в M2")?,
        )?;

        let our_id = self.identity.pairing_id.as_bytes();
        let sub = Tlv8::new()
            .with(tlv8::IDENTIFIER, our_id)
            .with(
                tlv8::SIGNATURE,
                self.identity
                    .sign(&[client_pub.as_slice(), our_id, &server_pub].concat()),
            )
            .encode();
        self.shared = Some(shared);
        Ok(Tlv8::new()
            .with(tlv8::STATE, [3])
            .with(
                tlv8::ENCRYPTED_DATA,
                aead::seal(&key, b"PV-Msg03", &[], &sub),
            )
            .encode())
    }

    pub fn check_m4(&self, m4: &[u8]) -> Result<ControlKeys, Error> {
        let msg = Tlv8::decode(m4)?;
        if msg.get(tlv8::ERROR).is_some() {
            return Err(Error::Auth("приёмник отклонил pair-verify"));
        }
        let shared = self.shared.ok_or(Error::UnexpectedState(4))?;
        Ok(ControlKeys::controller(&shared))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::MemoryStore;

    fn accessory() -> Identity {
        Identity::from_seed("acc-1", &[1; 32])
    }

    fn controller() -> Identity {
        Identity::from_seed("ctl-1", &[2; 32])
    }

    fn reply(o: SetupOutcome) -> Vec<u8> {
        match o {
            SetupOutcome::Continue(r) => r,
            _ => panic!("ожидался Continue"),
        }
    }

    #[test]
    fn transient_setup_gives_matching_keys() {
        let store = MemoryStore::default();
        let mut server = PairSetup::new(accessory(), "0000");
        let mut client = ControllerSetup::new(controller(), "", true);
        let m2 = reply(server.handle(&client.m1(), &store).unwrap());
        let m3 = client.m3(&m2).unwrap();
        let SetupOutcome::Transient { reply, keys } = server.handle(&m3, &store).unwrap() else {
            panic!("ожидалось transient-сопряжение");
        };
        let ctl_keys = client.check_m4(&reply).unwrap().unwrap();
        assert_eq!(keys.read, ctl_keys.write);
        assert_eq!(keys.write, ctl_keys.read);
    }

    #[test]
    fn pin_setup_then_verify() {
        let store = MemoryStore::default();
        let mut server = PairSetup::new(accessory(), "1234");
        let mut client = ControllerSetup::new(controller(), "1234", false);
        let m2 = reply(server.handle(&client.m1(), &store).unwrap());
        let m4 = reply(server.handle(&client.m3(&m2).unwrap(), &store).unwrap());
        assert!(client.check_m4(&m4).unwrap().is_none());
        let SetupOutcome::Paired {
            reply: m6,
            controller_id,
        } = server.handle(&client.m5().unwrap(), &store).unwrap()
        else {
            panic!("ожидалось сопряжение");
        };
        assert_eq!(controller_id, b"ctl-1");
        let acc = client.check_m6(&m6).unwrap();
        assert_eq!(acc.id, b"acc-1");

        let mut verify = PairVerify::new(accessory());
        let mut cv = ControllerVerify::new(controller(), acc);
        let VerifyOutcome::Continue(m2) = verify.handle(&cv.m1(), &store).unwrap() else {
            panic!()
        };
        let m3 = cv.m3(&m2).unwrap();
        let VerifyOutcome::Verified { reply, keys, .. } = verify.handle(&m3, &store).unwrap()
        else {
            panic!("ожидалась успешная проверка");
        };
        let ctl = cv.check_m4(&reply).unwrap();
        assert_eq!(keys.read, ctl.write);
    }

    #[test]
    fn wrong_pin_is_rejected() {
        let store = MemoryStore::default();
        let mut server = PairSetup::new(accessory(), "1234");
        let mut client = ControllerSetup::new(controller(), "9999", false);
        let m2 = reply(server.handle(&client.m1(), &store).unwrap());
        assert!(matches!(
            server.handle(&client.m3(&m2).unwrap(), &store).unwrap(),
            SetupOutcome::Rejected(_)
        ));
    }

    #[test]
    fn unknown_controller_fails_verify() {
        let store = MemoryStore::default();
        let mut verify = PairVerify::new(accessory());
        let acc = Accessory {
            id: b"acc-1".to_vec(),
            public_key: accessory().public_key(),
        };
        let mut cv = ControllerVerify::new(controller(), acc);
        let VerifyOutcome::Continue(m2) = verify.handle(&cv.m1(), &store).unwrap() else {
            panic!()
        };
        let m3 = cv.m3(&m2).unwrap();
        assert!(matches!(
            verify.handle(&m3, &store).unwrap(),
            VerifyOutcome::Rejected(_)
        ));
    }
}
