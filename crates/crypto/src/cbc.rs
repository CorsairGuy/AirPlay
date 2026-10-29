//! AES-128-CBC для звука AirPlay 1 (и звука зеркалирования экрана): каждый
//! пакет расшифровывается заново с тем же IV, шифруются только целые блоки,
//! хвост короче 16 байт идёт открытым.

use aes::Aes128;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};

pub struct AudioCbc {
    cipher: Aes128,
    iv: [u8; 16],
}

impl AudioCbc {
    pub fn new(key: &[u8; 16], iv: &[u8; 16]) -> Self {
        Self {
            cipher: Aes128::new(key.into()),
            iv: *iv,
        }
    }

    pub fn decrypt(&self, data: &mut [u8]) {
        let mut prev = self.iv;
        for block in data.chunks_exact_mut(16) {
            let saved: [u8; 16] = (*block).try_into().unwrap();
            self.cipher.decrypt_block(block.into());
            for (b, p) in block.iter_mut().zip(prev) {
                *b ^= p;
            }
            prev = saved;
        }
    }

    /// Для тестов и отправителя.
    pub fn encrypt(&self, data: &mut [u8]) {
        let mut prev = self.iv;
        for block in data.chunks_exact_mut(16) {
            for (b, p) in block.iter_mut().zip(prev) {
                *b ^= p;
            }
            self.cipher.encrypt_block(block.into());
            prev = (*block).try_into().unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keeps_tail_plain() {
        let c = AudioCbc::new(&[1; 16], &[2; 16]);
        let plain: Vec<u8> = (0..40).collect();
        let mut data = plain.clone();
        c.encrypt(&mut data);
        assert_ne!(data[..32], plain[..32]);
        assert_eq!(data[32..], plain[32..]);
        c.decrypt(&mut data);
        assert_eq!(data, plain);
    }

    /// NIST SP 800-38A F.2.2, первый блок.
    #[test]
    fn nist_vector() {
        let key = hex::decode("2b7e151628aed2a6abf7158809cf4f3c").unwrap();
        let iv = hex::decode("000102030405060708090a0b0c0d0e0f").unwrap();
        let mut ct = hex::decode("7649abac8119b246cee98e9b12e9197d").unwrap();
        AudioCbc::new(&key.try_into().unwrap(), &iv.try_into().unwrap()).decrypt(&mut ct);
        assert_eq!(hex::encode(ct), "6bc1bee22e409f96e93d7e117393172a");
    }
}
