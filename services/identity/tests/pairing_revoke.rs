//! Отзыв доверия в файловом хранилище (C43/C60).
//!
//! Временные каталоги, синтетические ключи. Проверяется, что отзыв переживает
//! перезапуск, различает «не найдено» и «ошибка записи», не теряет другие пары
//! и оставляет корректный файл без временных остатков.

use std::fs;
use std::path::{Path, PathBuf};

use identity::state::FileStore;
use pair::{PairingStore, Revoke};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "airplay-revoke-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn revoke_persists_across_restart_and_keeps_other_pairs() {
    let dir = TempDir::new("persist");
    {
        let store = FileStore::open(dir.path()).unwrap();
        store.add(b"ctl-1", [1; 32]);
        store.add(b"ctl-2", [2; 32]);
        assert_eq!(store.remove(b"ctl-1"), Revoke::Revoked);
        assert_eq!(store.len(), 1);
        // Файл остаётся разбираемым, временный файл убран.
        assert!(dir.path().join("pairings").exists());
        assert!(!dir.path().join("pairings.tmp").exists());
    }
    let reopened = FileStore::open(dir.path()).unwrap();
    assert_eq!(reopened.len(), 1);
    assert_eq!(reopened.get(b"ctl-1"), None);
    assert_eq!(reopened.get(b"ctl-2"), Some([2; 32]));
}

#[test]
fn removing_an_unknown_controller_reports_not_found() {
    let dir = TempDir::new("missing");
    let store = FileStore::open(dir.path()).unwrap();
    store.add(b"ctl-1", [1; 32]);
    assert_eq!(store.remove(b"nobody"), Revoke::NotFound);
    assert_eq!(store.len(), 1);
}

#[test]
fn a_failed_write_is_reported_and_trust_is_restored_in_memory() {
    let dir = TempDir::new("fail");
    let store = FileStore::open(dir.path()).unwrap();
    store.add(b"ctl-1", [1; 32]);
    assert_eq!(store.get(b"ctl-1"), Some([1; 32]));
    // Портим путь записи уже после успешного сопряжения: каталог на месте файла.
    fs::remove_file(dir.path().join("pairings")).unwrap();
    fs::create_dir(dir.path().join("pairings")).unwrap();
    assert_eq!(store.remove(b"ctl-1"), Revoke::Failed);
    assert_eq!(
        store.get(b"ctl-1"),
        Some([1; 32]),
        "после неудачной записи доверие восстановлено"
    );
    assert_eq!(store.len(), 1);
}

#[test]
fn deleting_the_record_is_idempotent_after_success() {
    let dir = TempDir::new("idempotent");
    let store = FileStore::open(dir.path()).unwrap();
    store.add(b"ctl-1", [7; 32]);
    assert_eq!(store.remove(b"ctl-1"), Revoke::Revoked);
    assert_eq!(store.remove(b"ctl-1"), Revoke::NotFound);
    assert_eq!(store.len(), 0);
    // Пустой файл читается как пустое хранилище.
    let reopened = FileStore::open(dir.path()).unwrap();
    assert!(reopened.is_empty());
}

#[test]
fn concurrent_revokes_leave_a_consistent_file() {
    let dir = TempDir::new("concurrent");
    let store = std::sync::Arc::new(FileStore::open(dir.path()).unwrap());
    for i in 0..16u8 {
        store.add(format!("ctl-{i}").as_bytes(), [i; 32]);
    }
    let threads: Vec<_> = (0..16u8)
        .map(|i| {
            let store = store.clone();
            std::thread::spawn(move || {
                assert_eq!(store.remove(format!("ctl-{i}").as_bytes()), Revoke::Revoked);
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert!(store.is_empty());
    let reopened = FileStore::open(dir.path()).unwrap();
    assert!(reopened.is_empty(), "файл после конкурентных отзывов согласован");
    assert!(!dir.path().join("pairings.tmp").exists());
}
