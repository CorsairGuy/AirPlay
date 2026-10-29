//! Сопряжение AirPlay в обе стороны.
//!
//! - [`hap`] — HomeKit pairing (AirPlay 2): pair-setup по PIN или transient
//!   (PIN 3939, без сохранения ключей) и pair-verify по сохранённым ключам.
//! - [`legacy`] — сопряжение AirPlay 1 (как в UxPlay): обмен ключами Ed25519
//!   и pair-verify с AES-CTR; нужно для зеркалирования.

pub mod hap;
pub mod identity;
pub mod legacy;
pub mod store;
pub mod tlv8;

pub use identity::Identity;
pub use store::{MemoryStore, PairingStore};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("неверный формат сообщения: {0}")]
    Malformed(&'static str),
    #[error("неожиданный шаг сопряжения: {0}")]
    UnexpectedState(u8),
    #[error("ошибка проверки подлинности: {0}")]
    Auth(&'static str),
    #[error(transparent)]
    Crypto(#[from] crypto::Error),
}
