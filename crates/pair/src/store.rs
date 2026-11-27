//! Хранилище сопряжённых контроллеров: идентификатор → ключ Ed25519.

use std::collections::HashMap;
use std::sync::Mutex;

pub trait PairingStore: Send + Sync {
    fn get(&self, id: &[u8]) -> Option<[u8; 32]>;
    fn add(&self, id: &[u8], public_key: [u8; 32]);
    /// Add a controller only if persistence succeeded. Legacy implementations
    /// must opt in rather than claiming that a fallible write succeeded.
    fn try_add(&self, _id: &[u8], _public_key: [u8; 32]) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "fallible pairing writes are unsupported",
        ))
    }
    /// Revoke a stored controller. Unsupported stores must report an error.
    fn remove(&self, _id: &[u8]) -> std::io::Result<bool> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "pairing revocation is unsupported",
        ))
    }
    /// Сколько контроллеров сопряжено.
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
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
    fn try_add(&self, id: &[u8], public_key: [u8; 32]) -> std::io::Result<()> {
        if id.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "empty pairing identifier",
            ));
        }
        self.add(id, public_key);
        Ok(())
    }
    fn remove(&self, id: &[u8]) -> std::io::Result<bool> {
        Ok(self.0.lock().unwrap().remove(id).is_some())
    }
    fn get(&self, id: &[u8]) -> Option<[u8; 32]> {
        self.0.lock().unwrap().get(id).copied()
    }

    fn add(&self, id: &[u8], public_key: [u8; 32]) {
        self.0.lock().unwrap().insert(id.to_vec(), public_key);
    }

    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}
