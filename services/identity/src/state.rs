//! Постоянное состояние приёмника: ключ устройства и сопряжённые контроллеры.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result, bail};
use pair::{Identity, MemoryStore, PairingStore, Revoke};
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

/// Сериализация пар в тот же формат, что читает [`FileStore::open`].
fn encode_entries(entries: &[(Vec<u8>, [u8; 32])]) -> String {
    entries
        .iter()
        .map(|(id, pk)| format!("{} {}\n", hex::encode(id), hex::encode(pk)))
        .collect()
}

/// Атомарная запись: временный файл рядом, затем замена. На Windows
/// `rename` поверх существующего файла не работает, поэтому замена делается
/// через удаление цели; при любой ошибке исходный файл остаётся прежним.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(_) if path.exists() => {
            fs::remove_file(path)?;
            fs::rename(&tmp, path).inspect_err(|_| {
                let _ = fs::remove_file(&tmp);
            })
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
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
        let previous = self.mem.get(id);
        self.mem.add(id, public_key);
        let text = encode_entries(&self.mem.entries());
        if let Err(e) = write_atomic(&self.path, &text) {
            // Память не должна обещать больше, чем лежит на диске.
            match previous {
                Some(old) => self.mem.add(id, old),
                None => {
                    self.mem.remove(id);
                }
            }
            tracing::error!(
                "не удалось сохранить сопряжения в {}: {e}",
                self.path.display()
            );
        }
    }

    fn remove(&self, id: &[u8]) -> Revoke {
        let _guard = self.write.lock().unwrap();
        let Some(old) = self.mem.get(id) else {
            return Revoke::NotFound;
        };
        self.mem.remove(id);
        let text = encode_entries(&self.mem.entries());
        if let Err(e) = write_atomic(&self.path, &text) {
            // Отзыв не сохранён — восстанавливаем доверие в памяти.
            self.mem.add(id, old);
            tracing::error!(
                "не удалось сохранить отзыв в {}: {e}",
                self.path.display()
            );
            return Revoke::Failed;
        }
        Revoke::Revoked
    }
}
