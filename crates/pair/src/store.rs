//! Хранилище сопряжённых контроллеров: идентификатор → ключ Ed25519.
//!
//! Отзыв доверия явно различает три случая: записи не было, хранилище не
//! поддерживает отзыв и ошибку записи. Молчаливый `false` для всех трёх
//! недопустим: вызывающий не должен считать отказ успехом.

use std::collections::HashMap;
use std::sync::Mutex;

/// Результат отзыва доверия.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revoke {
    /// Запись была и удалена.
    Revoked,
    /// Контроллера с таким идентификатором не было.
    NotFound,
    /// Хранилище не поддерживает отзыв (значение по умолчанию).
    Unsupported,
    /// Запись не удалось сохранить; в памяти она восстановлена.
    Failed,
}

pub trait PairingStore: Send + Sync {
    fn get(&self, id: &[u8]) -> Option<[u8; 32]>;
    fn add(&self, id: &[u8], public_key: [u8; 32]);
    /// Сколько контроллеров сопряжено.
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Отозвать доверие одному контроллеру. По умолчанию — `Unsupported`.
    fn remove(&self, _id: &[u8]) -> Revoke {
        Revoke::Unsupported
    }
}

#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<Vec<u8>, [u8; 32]>>);

impl MemoryStore {
    pub fn entries(&self) -> Vec<(Vec<u8>, [u8; 32])> {
        let map = self.0.lock().unwrap();
        map.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }
}

impl PairingStore for MemoryStore {
    fn get(&self, id: &[u8]) -> Option<[u8; 32]> {
        self.0.lock().unwrap().get(id).copied()
    }

    fn add(&self, id: &[u8], public_key: [u8; 32]) {
        self.0.lock().unwrap().insert(id.to_vec(), public_key);
    }

    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    fn remove(&self, id: &[u8]) -> Revoke {
        if self.0.lock().unwrap().remove(id).is_some() {
            Revoke::Revoked
        } else {
            Revoke::NotFound
        }
    }
}
