//! Постоянное состояние приёмника: ключ устройства и сопряжённые контроллеры.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
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
    match fs::read_to_string(&path) {
        Ok(text) => {
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
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("не прочитать {}", path.display())),
    }
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    let identity = Identity::from_seed(new_uuid(), &seed);
    fs::create_dir_all(dir).with_context(|| format!("не создать каталог {}", dir.display()))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("не создать {}", path.display()))?;
    write!(file, "{}\n{}\n", hex::encode(seed), identity.pairing_id)
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
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e).with_context(|| format!("не прочитать {}", path.display())),
        };
        for line in text.lines() {
            let (id, pk) = line.split_once(' ').context("invalid pairing record")?;
            let id = hex::decode(id).context("invalid pairing identifier")?;
            let key: [u8; 32] = hex::decode(pk.trim())
                .context("invalid pairing key")?
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid pairing key length"))?;
            if id.is_empty() || mem.get(&id).is_some() {
                bail!("empty or duplicate pairing identifier");
            }
            mem.add(&id, key);
        }
        Ok(Self {
            path,
            mem,
            write: Mutex::new(()),
        })
    }

    // Caller holds `write`; publish the complete snapshot before changing memory.
    fn persist(&self, entries: &[(Vec<u8>, [u8; 32])]) -> std::io::Result<()> {
        let text: String = entries
            .iter()
            .map(|(key, pk)| format!("{} {}\n", hex::encode(key), hex::encode(pk)))
            .collect();
        let temporary = self
            .path
            .with_file_name(format!(".pairings-{}.tmp", uuid::Uuid::new_v4()));
        let mut created = false;
        let result: std::io::Result<()> = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            created = true;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            // D25: тестовый subprocess обрывается на выбранной стороне rename.
            #[cfg(test)]
            if std::env::var("AIRPLAY_TEST_CRASH").as_deref() == Ok("before-rename") {
                std::process::abort();
            }
            fs::rename(&temporary, &self.path)?;
            #[cfg(test)]
            if std::env::var("AIRPLAY_TEST_CRASH").as_deref() == Ok("after-rename") {
                std::process::abort();
            }
            Ok(())
        })();
        if result.is_err() && created {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

impl PairingStore for FileStore {
    fn get(&self, id: &[u8]) -> Option<[u8; 32]> {
        let _guard = self.write.lock().unwrap();
        self.mem.get(id)
    }

    fn len(&self) -> usize {
        let _guard = self.write.lock().unwrap();
        self.mem.len()
    }

    fn remove(&self, id: &[u8]) -> std::io::Result<bool> {
        let _guard = self.write.lock().unwrap();
        let entries = self.mem.entries();
        if !entries.iter().any(|(key, _)| key.as_slice() == id) {
            return Ok(false);
        }
        let remaining: Vec<_> = entries
            .into_iter()
            .filter(|(key, _)| key.as_slice() != id)
            .collect();
        self.persist(&remaining)?;
        self.mem.remove(id)?;
        Ok(true)
    }

    fn add(&self, id: &[u8], public_key: [u8; 32]) {
        if let Err(e) = self.try_add(id, public_key) {
            tracing::error!("не удалось сохранить сопряжение: {e}");
        }
    }

    fn try_add(&self, id: &[u8], public_key: [u8; 32]) -> std::io::Result<()> {
        if id.is_empty() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                "empty pairing identifier",
            ));
        }
        let _guard = self.write.lock().unwrap();
        let mut entries = self.mem.entries();
        entries.retain(|(key, _)| key.as_slice() != id);
        entries.push((id.to_vec(), public_key));
        self.persist(&entries)?;
        self.mem.add(id, public_key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("airplay-identity-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn identity_is_stable_across_reopen() {
        let dir = test_dir();
        let first = load_identity(&dir).unwrap();
        let second = load_identity(&dir).unwrap();
        assert_eq!(first.pairing_id, second.pairing_id);
        assert_eq!(first.seed(), second.seed());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_identity_does_not_get_replaced() {
        let dir = test_dir();
        fs::create_dir(dir.join("identity")).unwrap();
        assert!(load_identity(&dir).is_err());
        assert!(dir.join("identity").is_dir());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unreadable_pairings_are_not_treated_as_empty() {
        let dir = test_dir();
        fs::create_dir(dir.join("pairings")).unwrap();
        assert!(FileStore::open(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_pairings_are_not_silently_discarded() {
        let dir = test_dir();
        fs::write(dir.join("pairings"), "damaged record\n").unwrap();
        assert!(FileStore::open(&dir).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("pairings")).unwrap(),
            "damaged record\n"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn revoked_pairing_stays_removed_after_restart() {
        let dir = test_dir();
        let store = FileStore::open(&dir).unwrap();
        store.add(b"one", [1; 32]);
        store.add(b"two", [2; 32]);
        assert!(store.remove(b"one").unwrap());
        assert!(!store.remove(b"one").unwrap());
        let reopened = FileStore::open(&dir).unwrap();
        assert_eq!(reopened.get(b"one"), None);
        assert_eq!(reopened.get(b"two"), Some([2; 32]));
        fs::remove_dir_all(dir).unwrap();
    }

    /// D25: дочерний процесс с тем же тестовым бинарником пишет FileStore и
    /// обрывается до или после rename. Родитель проверяет, что после
    /// перезапуска виден только целый старый или новый снимок.
    #[test]
    fn crash_child_writes_and_aborts() {
        let Some(dir) = std::env::var_os("AIRPLAY_TEST_CRASH_DIR") else {
            return;
        };
        let store = FileStore::open(Path::new(&dir)).unwrap();
        store.try_add(b"new", [7; 32]).unwrap();
        panic!("crash hook did not fire");
    }

    fn run_crash_child(dir: &Path, when: &str) {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "state::tests::crash_child_writes_and_aborts",
                "--nocapture",
            ])
            .env("AIRPLAY_TEST_CRASH", when)
            .env("AIRPLAY_TEST_CRASH_DIR", dir)
            .status()
            .unwrap();
        assert!(!status.success(), "дочерний процесс должен быть прерван");
    }

    #[test]
    fn subprocess_crash_before_rename_keeps_the_old_snapshot() {
        let dir = test_dir();
        let store = FileStore::open(&dir).unwrap();
        store.try_add(b"one", [1; 32]).unwrap();
        run_crash_child(&dir, "before-rename");
        assert!(
            fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains(".tmp")),
            "незавершённый временный файл остаётся"
        );
        let reopened = FileStore::open(&dir).unwrap();
        assert_eq!(reopened.get(b"one"), Some([1; 32]));
        assert_eq!(reopened.get(b"new"), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn subprocess_crash_after_rename_keeps_the_new_snapshot() {
        let dir = test_dir();
        let store = FileStore::open(&dir).unwrap();
        store.try_add(b"one", [1; 32]).unwrap();
        run_crash_child(&dir, "after-rename");
        let reopened = FileStore::open(&dir).unwrap();
        assert_eq!(reopened.get(b"one"), Some([1; 32]));
        assert_eq!(reopened.get(b"new"), Some([7; 32]));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn abandoned_temporary_snapshots_do_not_restore_revoked_pairings() {
        let dir = test_dir();
        let store = FileStore::open(&dir).unwrap();
        store.try_add(b"one", [1; 32]).unwrap();
        store.try_add(b"two", [2; 32]).unwrap();
        let before_revoke = fs::read(dir.join("pairings")).unwrap();
        assert!(store.remove(b"one").unwrap());
        let stale = dir.join(format!(".pairings-{}.tmp", uuid::Uuid::new_v4()));
        let partial = dir.join(format!(".pairings-{}.tmp", uuid::Uuid::new_v4()));
        fs::write(&stale, before_revoke).unwrap();
        fs::write(&partial, b"partial record").unwrap();

        let reopened = FileStore::open(&dir).unwrap();
        assert_eq!(reopened.get(b"one"), None);
        assert_eq!(reopened.get(b"two"), Some([2; 32]));
        assert!(stale.exists() && partial.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_revoke_preserves_in_memory_pairing() {
        let dir = test_dir();
        let store = FileStore::open(&dir).unwrap();
        store.mem.add(b"one", [1; 32]);
        fs::create_dir(dir.join("pairings")).unwrap();
        assert!(store.remove(b"one").is_err());
        assert_eq!(store.get(b"one"), Some([1; 32]));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_add_does_not_change_memory_or_overwrite_an_existing_key() {
        let dir = test_dir();
        let store = FileStore::open(&dir).unwrap();
        store.try_add(b"one", [1; 32]).unwrap();
        fs::remove_file(dir.join("pairings")).unwrap();
        fs::create_dir(dir.join("pairings")).unwrap();
        assert!(store.try_add(b"one", [2; 32]).is_err());
        assert!(store.try_add(b"two", [3; 32]).is_err());
        assert_eq!(store.get(b"one"), Some([1; 32]));
        assert_eq!(store.get(b"two"), None);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_revokes_on_shared_store_persist_every_removal() {
        let dir = test_dir();
        let store = std::sync::Arc::new(FileStore::open(&dir).unwrap());
        for i in 0..16u8 {
            store.add(format!("ctl-{i}").as_bytes(), [i; 32]);
        }
        let threads: Vec<_> = (0..16u8)
            .map(|i| {
                let store = store.clone();
                std::thread::spawn(move || {
                    assert!(store.remove(format!("ctl-{i}").as_bytes()).unwrap())
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(store.is_empty());
        assert!(FileStore::open(&dir).unwrap().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
