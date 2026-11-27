//! Хранилище сопряжённых контроллеров: идентификатор → ключ Ed25519.

use std::collections::HashMap;
use std::sync::Mutex;

pub trait PairingStore: Send + Sync {
    fn get(&self, id: &[u8]) -> Option<[u8; 32]>;
    fn add(&self, id: &[u8], public_key: [u8; 32]);
    /// Сколько контроллеров сопряжено.
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Отозвать доверие одного контроллера. `true` — запись была удалена.
    ///
    /// По умолчанию хранилище отзыв не поддерживает и возвращает `false`:
    /// вызывающий обязан считать `false` отказом, а не успешным отзывом.
    /// Повторный отзыв неизвестного идентификатора тоже даёт `false`.
    fn remove(&self, _id: &[u8]) -> bool {
        false
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

    fn remove(&self, id: &[u8]) -> bool {
        self.0.lock().unwrap().remove(id).is_some()
    }
}
