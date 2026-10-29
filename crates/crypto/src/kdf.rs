//! Вывод ключей HKDF-SHA512, как в HAP.

use hkdf::Hkdf;
use sha2::Sha512;

/// HKDF-SHA512 с выходом 32 байта (размер ключа ChaCha20).
pub fn hkdf_sha512(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<Sha512>::new(Some(salt), ikm)
        .expand(info, &mut out)
        .expect("32 байта всегда допустимы для HKDF-SHA512");
    out
}

/// Ключи шифрованного канала управления после сопряжения.
#[derive(Clone)]
pub struct ControlKeys {
    /// Ключ, которым мы расшифровываем входящие данные.
    pub read: [u8; 32],
    /// Ключ, которым мы шифруем исходящие данные.
    pub write: [u8; 32],
}

impl ControlKeys {
    /// Ключи для стороны приёмника (accessory): клиент пишет
    /// `Control-Write-Encryption-Key`, мы этим ключом читаем.
    pub fn accessory(shared: &[u8]) -> Self {
        Self {
            read: hkdf_sha512(shared, b"Control-Salt", b"Control-Write-Encryption-Key"),
            write: hkdf_sha512(shared, b"Control-Salt", b"Control-Read-Encryption-Key"),
        }
    }

    /// Канал событий (порт `eventPort` из SETUP): приёмник пишет
    /// `Events-Write-Encryption-Key`, как в shairplay-rust.
    pub fn accessory_events(shared: &[u8]) -> Self {
        Self {
            read: hkdf_sha512(shared, b"Events-Salt", b"Events-Read-Encryption-Key"),
            write: hkdf_sha512(shared, b"Events-Salt", b"Events-Write-Encryption-Key"),
        }
    }

    /// Канал данных удалённого управления (поток 130): соль —
    /// `DataStream-Salt` и `seed` из SETUP десятичным числом. Отправитель
    /// пишет `Output`, мы — `Input` (как в pyatv и shairport-sync).
    pub fn accessory_data(shared: &[u8], seed: u64) -> Self {
        let salt = format!("DataStream-Salt{seed}");
        Self {
            read: hkdf_sha512(shared, salt.as_bytes(), b"DataStream-Output-Encryption-Key"),
            write: hkdf_sha512(shared, salt.as_bytes(), b"DataStream-Input-Encryption-Key"),
        }
    }

    /// Канал данных со стороны отправителя.
    pub fn controller_data(shared: &[u8], seed: u64) -> Self {
        let acc = Self::accessory_data(shared, seed);
        Self {
            read: acc.write,
            write: acc.read,
        }
    }

    /// Ключи для стороны отправителя (controller).
    pub fn controller(shared: &[u8]) -> Self {
        let acc = Self::accessory(shared);
        Self {
            read: acc.write,
            write: acc.read,
        }
    }

    /// Канал событий со стороны отправителя.
    pub fn controller_events(shared: &[u8]) -> Self {
        let acc = Self::accessory_events(shared);
        Self {
            read: acc.write,
            write: acc.read,
        }
    }
}
