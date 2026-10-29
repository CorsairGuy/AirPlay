//! Долговременный ключ устройства (Ed25519) и его идентификатор.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::Error;

#[derive(Clone)]
pub struct Identity {
    /// Идентификатор для сопряжения (в AirPlay — значение `pi`).
    pub pairing_id: String,
    key: SigningKey,
}

impl Identity {
    pub fn from_seed(pairing_id: impl Into<String>, seed: &[u8; 32]) -> Self {
        Self {
            pairing_id: pairing_id.into(),
            key: SigningKey::from_bytes(seed),
        }
    }

    pub fn seed(&self) -> [u8; 32] {
        self.key.to_bytes()
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.key.sign(msg).to_bytes()
    }
}

pub fn verify_signature(public: &[u8], msg: &[u8], signature: &[u8]) -> Result<(), Error> {
    let public: [u8; 32] = public
        .try_into()
        .map_err(|_| Error::Malformed("длина ключа Ed25519"))?;
    let signature: [u8; 64] = signature
        .try_into()
        .map_err(|_| Error::Malformed("длина подписи Ed25519"))?;
    VerifyingKey::from_bytes(&public)
        .map_err(|_| Error::Auth("некорректный ключ Ed25519"))?
        .verify(msg, &Signature::from_bytes(&signature))
        .map_err(|_| Error::Auth("подпись не сходится"))
}
