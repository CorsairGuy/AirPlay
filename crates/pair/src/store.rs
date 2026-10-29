//! Хранилище сопряжённых контроллеров: идентификатор → ключ Ed25519.

use std::collections::HashMap;
use std::sync::Mutex;

pub trait PairingStore: Send + Sync {
    fn get(&self, id: &[u8]) -> Option<[u8; 32]>;
    fn add(&self, id: &[u8], public_key: [u8; 32]);
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
}
