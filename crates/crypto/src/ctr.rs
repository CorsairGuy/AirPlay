//! AES-128-CTR для сопряжения AirPlay 1 (legacy pair-verify).

use aes::Aes128;
use ctr::cipher::{KeyIvInit, StreamCipher};

pub type Aes128Ctr = ctr::Ctr128BE<Aes128>;

pub fn aes128_ctr(key: &[u8; 16], iv: &[u8; 16]) -> Aes128Ctr {
    Aes128Ctr::new(key.into(), iv.into())
}

/// Применяет поток к данным на месте (шифрование и расшифровка совпадают).
pub fn apply(cipher: &mut Aes128Ctr, data: &mut [u8]) {
    cipher.apply_keystream(data);
}
