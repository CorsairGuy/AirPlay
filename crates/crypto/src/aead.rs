//! ChaCha20-Poly1305 с 8-байтовым nonce, дополненным слева четырьмя нулями.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};

use crate::Error;

pub const TAG_LEN: usize = 16;

/// Nonce из 8 байт (например `b"PV-Msg02"` или счётчик LE).
fn nonce(n: &[u8; 8]) -> Nonce {
    let mut full = [0u8; 12];
    full[4..].copy_from_slice(n);
    full.into()
}

pub fn seal(key: &[u8; 32], n: &[u8; 8], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    ChaCha20Poly1305::new(key.into())
        .encrypt(
            &nonce(n),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("шифрование ChaCha20-Poly1305 не может завершиться ошибкой")
}

pub fn open(key: &[u8; 32], n: &[u8; 8], aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, Error> {
    ChaCha20Poly1305::new(key.into())
        .decrypt(
            &nonce(n),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| Error::Aead)
}
