//! Отзыв и записи сопряжений при перезапуске и остатках временного файла (C60).
//!
//! Временные каталоги, синтетические ключи. Проверяется, что после сбоя
//! посередине записи читается прежний полный набор пар, а не половина, и что
//! остаток pairings.tmp не влияет на загрузку.

use std::fs;
use std::path::{Path, PathBuf};

use identity::state::FileStore;
use pair::{PairingStore, Revoke};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "airplay-crash-{tag}-{}-{:?}",
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
fn a_leftover_temporary_file_does_not_change_the_loaded_pairs() {
    let dir = TempDir::new("residue");
    {
        let store = FileStore::open(dir.path()).unwrap();
        store.add(b"ctl-1", [1; 32]);
        store.add(b"ctl-2", [2; 32]);
    }
    // Имитируем сбой после записи временного файла, но до замены.
    fs::write(dir.path().join("pairings.tmp"), b"broken partial").unwrap();

    let reopened = FileStore::open(dir.path()).unwrap();
    assert_eq!(reopened.len(), 2);
    assert_eq!(reopened.get(b"ctl-1"), Some([1; 32]));
    assert_eq!(reopened.get(b"ctl-2"), Some([2; 32]));
}

#[test]
fn a_corrupt_line_does_not_hide_other_pairs() {
    let dir = TempDir::new("corrupt");
    {
        let store = FileStore::open(dir.path()).unwrap();
        store.add(b"ctl-1", [1; 32]);
        store.add(b"ctl-2", [2; 32]);
    }
    let text = fs::read_to_string(dir.path().join("pairings")).unwrap();
    fs::write(
        dir.path().join("pairings"),
        format!("{text}not-a-pair\nzz zz\n"),
    )
    .unwrap();

    let reopened = FileStore::open(dir.path()).unwrap();
    assert_eq!(reopened.len(), 2, "битая строка не должна терять пары");
    assert_eq!(reopened.get(b"ctl-2"), Some([2; 32]));
}

#[test]
fn revoke_survives_restart_and_does_not_reappear_after_a_stale_write() {
    let dir = TempDir::new("stale");
    let store = FileStore::open(dir.path()).unwrap();
    store.add(b"ctl-1", [1; 32]);
    assert_eq!(store.remove(b"ctl-1"), Revoke::Revoked);
    // Старый снимок «до отзыва» не должен вернуть пару после перезапуска.
    fs::write(
        dir.path().join("pairings.tmp"),
        format!("{} {}\n", hex_ct(b"ctl-1"), hex_ct(&[1; 32])),
    )
    .unwrap();

    let reopened = FileStore::open(dir.path()).unwrap();
    assert_eq!(reopened.get(b"ctl-1"), None);
    assert!(reopened.is_empty());
}

fn hex_ct(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
