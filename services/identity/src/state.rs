//! Постоянное состояние приёмника: ключ устройства и сопряжённые контроллеры.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use pair::{Identity, MemoryStore, PairingStore};
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha512};

/// Каталог состояния по умолчанию.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AIRPLAY_STATE_DIR") {
        return dir.into();
    }
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return Path::new(&local).join("airplay");
        }
    }
    match std::env::var_os("HOME") {
        Some(home) => Path::new(&home).join(".local/state/airplay"),
        None => PathBuf::from("airplay-state"),
    }
}

/// Случайный UUID версии 4.
fn new_uuid() -> String {
    let mut b = [0u8; 16];
    OsRng.fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Загружает ключ устройства или создаёт новый при первом запуске.
pub fn load_identity(dir: &Path) -> Result<Identity> {
    let path = dir.join("identity");
    if let Ok(text) = fs::read_to_string(&path) {
        let mut lines = text.lines();
        let (Some(seed), Some(id)) = (lines.next(), lines.next()) else {
            bail!("повреждён файл {}", path.display());
        };
        let seed: [u8; 32] = hex::decode(seed.trim())
            .ok()
            .and_then(|s| s.try_into().ok())
            .with_context(|| format!("повреждён ключ в {}", path.display()))?;
        return Ok(Identity::from_seed(id.trim(), &seed));
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let identity = Identity::from_seed(new_uuid(), &seed);
    fs::create_dir_all(dir).with_context(|| format!("не создать каталог {}", dir.display()))?;
    fs::write(
        &path,
        format!("{}\n{}\n", hex::encode(seed), identity.pairing_id),
    )
    .with_context(|| format!("не записать {}", path.display()))?;
    Ok(identity)
}

/// Стабильный псевдо-MAC из ключа устройства (локально администрируемый адрес).
pub fn device_id(identity: &Identity) -> String {
    let h = Sha512::digest(identity.seed());
    let mut mac = [0u8; 6];
    mac.copy_from_slice(&h[..6]);
    mac[0] = (mac[0] | 0x02) & 0xfe;
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Сопряжённые контроллеры в файле `pairings`: строка `hex(id) hex(ключ)`.
pub struct FileStore {
    path: PathBuf,
    mem: MemoryStore,
    write: Mutex<()>,
}

impl FileStore {
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("pairings");
        let mem = MemoryStore::default();
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let Some((id, pk)) = line.split_once(' ') else {
                    continue;
                };
                if let (Ok(id), Ok(Ok(pk))) = (
                    hex::decode(id),
                    hex::decode(pk.trim()).map(<[u8; 32]>::try_from),
                ) {
                    mem.add(&id, pk);
                }
            }
        }
        Ok(Self {
            path,
            mem,
            write: Mutex::new(()),
        })
    }
}

impl PairingStore for FileStore {
    fn get(&self, id: &[u8]) -> Option<[u8; 32]> {
        self.mem.get(id)
    }

    fn len(&self) -> usize {
        self.mem.len()
    }

    fn add(&self, id: &[u8], public_key: [u8; 32]) {
        let _guard = self.write.lock().unwrap();
        self.mem.add(id, public_key);
        let text: String = self
            .mem
            .entries()
            .iter()
            .map(|(id, pk)| format!("{} {}\n", hex::encode(id), hex::encode(pk)))
            .collect();
        if let Err(e) = fs::write(&self.path, text) {
            tracing::error!(
                "не удалось сохранить сопряжения в {}: {e}",
                self.path.display()
            );
        }
    }
}
