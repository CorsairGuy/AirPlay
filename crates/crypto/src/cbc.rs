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

    /// Независимый вектор, сгенерированный OpenSSL 3.5.9:
    /// `openssl enc -aes-128-cbc -K 000102030405060708090a0b0c0d0e0f
    /// -iv 0f0e0d0c0b0a09080706050403020100 -nopad` на 48 байтах `00..2f`.
    /// Проверяет whole-block CBC, открытый хвост, неверный ключ и пустой ввод.
    #[test]
    fn openssl_vector_encrypts_whole_blocks_and_leaves_tail() {
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let iv: [u8; 16] = std::array::from_fn(|i| (15 - i) as u8);
        let expected = hex::decode(
            "03a9c8fe778fb8a8668359542ad4d584bce873fe4bc2ba36d6d8742b27cdd457\
             0f12246df175346786982b0e092470f3",
        )
        .unwrap();
        let cipher = AudioCbc::new(&key, &iv);
        let plain: Vec<u8> = (0..48).collect();

        let mut data = plain.clone();
        cipher.encrypt(&mut data);
        assert_eq!(data, expected, "CBC должен совпасть с OpenSSL");
        cipher.decrypt(&mut data);
        assert_eq!(data, plain);

        let mut tail = vec![0xAAu8; 15];
        cipher.encrypt(&mut tail);
        assert_eq!(tail, vec![0xAAu8; 15], "хвост короче блока открыт");
        let mut tail = vec![0xAAu8; 47];
        cipher.encrypt(&mut tail);
        assert_eq!(tail[32..], [0xAAu8; 15], "неполный блок открыт");

        let mut empty: Vec<u8> = Vec::new();
        cipher.encrypt(&mut empty);
        cipher.decrypt(&mut empty);
        assert!(empty.is_empty());

        let mut wrong = expected.clone();
        AudioCbc::new(&[0xFF; 16], &iv).decrypt(&mut wrong);
        assert_ne!(wrong, plain, "чужой ключ не даёт открытый текст");
    }

}
