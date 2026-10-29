//! Криптография AirPlay: HKDF, SRP-6a 3072, ChaCha20-Poly1305 и шифрованный
//! канал HAP, AES-CTR для сопряжения AirPlay 1.

pub mod aead;
pub mod cbc;
pub mod channel;
pub mod ctr;
pub mod kdf;
pub mod srp;

pub use channel::Channel;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ошибка проверки подлинности (AEAD)")]
    Aead,
    #[error("кадр длиннее допустимого: {0} байт")]
    FrameTooLong(usize),
    #[error("SRP: {0}")]
    Srp(&'static str),
}
