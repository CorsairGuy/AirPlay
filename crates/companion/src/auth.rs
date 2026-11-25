//! Pair-verify Companion Link: TLV8 внутри OPACK-кадра `_pd`, X25519,
//! ChaCha20-Poly1305 с nonce `PV-Msg02`/`PV-Msg03`, затем вывод двух
//! ключей канала (`ClientEncrypt-main` — запись, `ServerEncrypt-main` — чтение).

use crypto::aead;
use crypto::kdf::hkdf_sha512;
use pair::Identity;
use pair::identity::verify_signature;
use pair::tlv8::{self, Tlv8};
use rand::rngs::OsRng;
use x25519_dalek::{PublicKey, StaticSecret};

use crate::Error;
use crate::opack::Value;

/// Ключ, под которым pair-verify TLV лежит в OPACK-словаре.
pub const PAIRING_DATA: &str = "_pd";

/// Долговременные ключи обеих сторон.
#[derive(Clone)]
pub struct Credentials {
    /// Идентификатор нашего пульта (обычно UUID строкой).
    pub client_id: Vec<u8>,
    /// Долговременный ключ подписи пульта (Ed25519, seed).
    pub client_seed: [u8; 32],
    /// Идентификатор приёмника из сохранённых данных сопряжения.
    pub accessory_id: Vec<u8>,
    /// Долговременный публичный ключ приёмника (Ed25519).
    pub accessory_ltpk: [u8; 32],
}

impl Credentials {
    pub fn new(
        client_id: impl Into<Vec<u8>>,
        client_seed: [u8; 32],
        accessory_id: impl Into<Vec<u8>>,
        accessory_ltpk: [u8; 32],
    ) -> Self {
        Self {
            client_id: client_id.into(),
            client_seed,
            accessory_id: accessory_id.into(),
            accessory_ltpk,
        }
    }
}

/// Клиентская сторона pair-verify Companion.
pub struct PairVerify {
    credentials: Credentials,
    identity: Identity,
    secret: StaticSecret,
    public: [u8; 32],
    shared: Option<[u8; 32]>,
    confirmed: bool,
}

impl PairVerify {
    pub fn new(credentials: Credentials) -> Self {
        // Идентификатор в `Identity` не используется: подписываем точные
        // байты `client_id`, поэтому строка-заглушка безопасна.
        let identity = Identity::from_seed("companion-client", &credentials.client_seed);
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret).to_bytes();
        Self {
            credentials,
            identity,
            secret,
            public,
            shared: None,
            confirmed: false,
        }
    }

    /// Содержимое кадра `PairVerifyStart`: TLV с нашим эфемерным ключом.
    pub fn start(&self) -> Value {
        let tlv = Tlv8::new()
            .with(tlv8::STATE, [1])
            .with(tlv8::PUBLIC_KEY, self.public.to_vec())
            .encode();
        Value::object(vec![
            (PAIRING_DATA, Value::Data(tlv)),
            ("_auTy", Value::Int(4)),
        ])
    }

    /// Обрабатывает ответ приёмника и возвращает содержимое кадра
    /// `PairVerifyNext` с нашим доказательством.
    pub fn finish(&mut self, response: &Value) -> Result<Value, Error> {
        if self.shared.is_some() { return Err(Error::Pair(pair::Error::Auth("pair-verify уже начат"))); }
        let tlv_bytes = response
            .get(PAIRING_DATA)
            .and_then(Value::as_data)
            .ok_or(Error::NoPairingData)?;
        let tlv = Tlv8::decode(tlv_bytes)?;
        if tlv.get(tlv8::ERROR).is_some() {
            return Err(Error::Pair(pair::Error::Auth(
                "приёмник отклонил pair-verify",
            )));
        }
        if tlv.state()? != 2 {
            return Err(Error::Pair(pair::Error::UnexpectedState(tlv.state()?)));
        }
        let server_public: [u8; 32] = tlv
            .require(tlv8::PUBLIC_KEY, "нет PublicKey в ответе pair-verify")?
            .try_into()
            .map_err(|_| Error::Pair(pair::Error::Malformed("длина ключа X25519")))?;
        let encrypted = tlv.require(tlv8::ENCRYPTED_DATA, "нет EncryptedData")?;

        let shared = self
            .secret
            .diffie_hellman(&PublicKey::from(server_public))
            .to_bytes();
        let key = hkdf_sha512(
            &shared,
            b"Pair-Verify-Encrypt-Salt",
            b"Pair-Verify-Encrypt-Info",
        );
        let plain = aead::open(&key, b"PV-Msg02", &[], encrypted)?;
        let inner = Tlv8::decode(&plain)?;
        let accessory_id = inner.require(tlv8::IDENTIFIER, "нет Identifier приёмника")?;
        if accessory_id != self.credentials.accessory_id.as_slice() {
            return Err(Error::Pair(pair::Error::Auth("чужой приёмник")));
        }
        let signature = inner.require(tlv8::SIGNATURE, "нет Signature приёмника")?;
        let info = [server_public.as_slice(), accessory_id, &self.public].concat();
        verify_signature(&self.credentials.accessory_ltpk, &info, signature)?;

        self.shared = Some(shared);

        let our_id = self.credentials.client_id.as_slice();
        let proof = self
            .identity
            .sign(&[self.public.as_slice(), our_id, &server_public].concat());
        let sub = Tlv8::new()
            .with(tlv8::IDENTIFIER, our_id.to_vec())
            .with(tlv8::SIGNATURE, proof.to_vec())
            .encode();
        let reply = Tlv8::new()
            .with(tlv8::STATE, [3])
            .with(
                tlv8::ENCRYPTED_DATA,
                aead::seal(&key, b"PV-Msg03", &[], &sub),
            )
            .encode();
        Ok(Value::object(vec![(PAIRING_DATA, Value::Data(reply))]))
    }

    /// Confirm the peer's final response before the encrypted channel is used.
    pub fn confirm(&mut self, response: &Value) -> Result<(), Error> {
        if self.shared.is_none() || self.confirmed { return Err(Error::Pair(pair::Error::Auth("неверный порядок pair-verify"))); }
        let bytes = response.get(PAIRING_DATA).and_then(Value::as_data).ok_or(Error::NoPairingData)?;
        let tlv = Tlv8::decode(bytes)?;
        if tlv.get(tlv8::ERROR).is_some() { return Err(Error::Pair(pair::Error::Auth("приёмник отклонил pair-verify"))); }
        if tlv.state()? != 4 { return Err(Error::Pair(pair::Error::UnexpectedState(tlv.state()?))); }
        self.confirmed = true;
        Ok(())
    }

    /// Ключи доступны после подтверждения приёмника: `(запись, чтение)`.
    pub fn keys(&self) -> Result<([u8; 32], [u8; 32]), Error> {
        if !self.confirmed { return Err(Error::Pair(pair::Error::Auth("pair-verify не подтверждён"))); }
        let shared = self
            .shared
            .ok_or(Error::Pair(pair::Error::Auth("pair-verify не завершён")))?;
        Ok((
            hkdf_sha512(&shared, b"", b"ClientEncrypt-main"),
            hkdf_sha512(&shared, b"", b"ServerEncrypt-main"),
        ))
    }

    /// Общий секрет X25519 (для тестов и диагностики).
    pub fn shared(&self) -> Option<[u8; 32]> {
        self.confirmed.then_some(self.shared).flatten()
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pair::tlv8::ERROR;

    struct Accessory {
        identity: Identity,
        secret: StaticSecret,
    }

    impl Accessory {
        fn new(seed: u8) -> Self {
            Self {
                identity: Identity::from_seed("acc", &[seed; 32]),
                secret: StaticSecret::random_from_rng(OsRng),
            }
        }

        /// Строит ответ M2 на `start`, как это делает приёмник.
        fn reply(&self, start: &Value) -> (Value, [u8; 32]) {
            let tlv = Tlv8::decode(start.get(PAIRING_DATA).unwrap().as_data().unwrap()).unwrap();
            let client_public: [u8; 32] = tlv.get(tlv8::PUBLIC_KEY).unwrap().try_into().unwrap();
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
                Value::object(vec![(PAIRING_DATA, Value::Data(tlv))]),
                shared,
            )
        }
    }

    fn credentials(accessory: &Accessory, client_seed: [u8; 32]) -> Credentials {
        Credentials::new(
            b"client-id",
            client_seed,
            accessory.identity.pairing_id.as_bytes().to_vec(),
            accessory.identity.public_key(),
        )
    }

    #[test]
    fn full_exchange_matches_shared_secret_and_keys() {
        let accessory = Accessory::new(1);
        let client_identity = Identity::from_seed("c", &[9; 32]);
        let mut verify = PairVerify::new(Credentials::new(
            b"client-id",
            client_identity.seed(),
            accessory.identity.pairing_id.as_bytes().to_vec(),
            accessory.identity.public_key(),
        ));
        let start = verify.start();
        // Наш публичный ключ реально лежит в TLV.
        let tlv = Tlv8::decode(start.get(PAIRING_DATA).unwrap().as_data().unwrap()).unwrap();
        assert_eq!(
            tlv.get(tlv8::PUBLIC_KEY).unwrap(),
            verify.public_key().as_slice()
        );
        assert_eq!(tlv.state().unwrap(), 1);

        let (reply, accessory_shared) = accessory.reply(&start);
        let proof = verify.finish(&reply).unwrap();
        assert!(verify.keys().is_err());
        assert!(verify.shared().is_none());
        let rejected = Value::object(vec![(PAIRING_DATA, Value::Data(Tlv8::new().with(tlv8::STATE, [4]).with(ERROR, [2]).encode()))]);
        assert!(verify.confirm(&rejected).is_err());
        assert!(verify.keys().is_err());
        let confirmed = Value::object(vec![(PAIRING_DATA, Value::Data(Tlv8::new().with(tlv8::STATE, [4]).encode()))]);
        verify.confirm(&confirmed).unwrap();
        let shared = verify.shared().unwrap();
        assert_eq!(shared, accessory_shared);

        // M3 расшифровывается тем же ключом и подпись сходится.
        let key = hkdf_sha512(
            &shared,
            b"Pair-Verify-Encrypt-Salt",
            b"Pair-Verify-Encrypt-Info",
        );
        let tlv = Tlv8::decode(proof.get(PAIRING_DATA).unwrap().as_data().unwrap()).unwrap();
        assert_eq!(tlv.state().unwrap(), 3);
        let plain = aead::open(
            &key,
            b"PV-Msg03",
            &[],
            tlv.get(tlv8::ENCRYPTED_DATA).unwrap(),
        )
        .unwrap();
        let inner = Tlv8::decode(&plain).unwrap();
        assert_eq!(inner.get(tlv8::IDENTIFIER).unwrap(), b"client-id");
        verify_signature(
            &client_identity.public_key(),
            &[
                verify.public_key().as_slice(),
                b"client-id".as_slice(),
                PublicKey::from(&accessory.secret).as_bytes(),
            ]
            .concat(),
            inner.get(tlv8::SIGNATURE).unwrap(),
        )
        .unwrap();

        let (write, read) = verify.keys().unwrap();
        assert_eq!(write, hkdf_sha512(&shared, b"", b"ClientEncrypt-main"));
        assert_eq!(read, hkdf_sha512(&shared, b"", b"ServerEncrypt-main"));
        assert_ne!(write, read);
    }

    #[test]
    fn wrong_accessory_or_error_is_rejected() {
        let accessory = Accessory::new(2);
        let other = Accessory::new(3);
        let mut verify = PairVerify::new(credentials(&accessory, [4; 32]));
        let start = verify.start();
        // Ответ подписан другим приёмником — идентификатор не совпадёт.
        let (reply, _) = other.reply(&start);
        assert!(verify.finish(&reply).is_err());
        assert!(verify.keys().is_err());

        let mut verify = PairVerify::new(credentials(&accessory, [5; 32]));
        let error = Tlv8::new()
            .with(tlv8::STATE, [2])
            .with(ERROR, [0x02])
            .encode();
        let reply = Value::object(vec![(PAIRING_DATA, Value::Data(error))]);
        assert!(verify.finish(&reply).is_err());
    }

    #[test]
    fn missing_pairing_data_is_rejected() {
        let accessory = Accessory::new(6);
        let mut verify = PairVerify::new(credentials(&accessory, [7; 32]));
        assert!(verify.finish(&Value::empty_object()).is_err());
        assert!(verify.finish(&Value::Int(1)).is_err());
    }
}
