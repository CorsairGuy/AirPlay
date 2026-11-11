//! Outgoing pairings keyed by stable receiver UID, not IP or PIN.
//! Share one application-owned instance per private directory via Arc.
use anyhow::{Context, Result, bail};
use sender::PairingCredentials;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone)]
struct Peer {
    id: Vec<u8>,
    key: [u8; 32],
}
pub struct SenderStore {
    dir: PathBuf,
    controller: pair::Identity,
    peers: Mutex<BTreeMap<String, Peer>>,
}

fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}
fn private(path: &Path, directory: bool) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    if m.file_type().is_symlink() || (directory && !m.is_dir()) || (!directory && !m.is_file()) {
        bail!("invalid pairing state file type");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid takes no arguments and has no memory requirements.
        if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            bail!("pairing state is not private");
        }
    }
    Ok(())
}
fn create_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn read(path: &Path, limit: u64) -> Result<String> {
    private(path, false)?;
    let mut text = String::new();
    File::open(path)?
        .take(limit + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > limit {
        bail!("pairing state exceeds size limit");
    }
    Ok(text)
}
fn uid(id: &str) -> Result<String> {
    if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        bail!("invalid receiver UID");
    }
    Ok(uuid::Uuid::parse_str(id)
        .ok()
        .filter(|_| id.len() == 36)
        .map(|v| v.to_string())
        .unwrap_or_else(|| id.into()))
}
impl SenderStore {
    pub fn open(dir: &Path) -> Result<Self> {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir)?;
        private(dir, true)?;
        let path = dir.join("identity");
        if !exists(&path)? {
            let seed = rand::random::<[u8; 32]>();
            let id = uuid::Uuid::new_v4().to_string();
            match create_file(&path) {
                Ok(mut file) => {
                    file.write_all(format!("{}\n{id}\n", hex::encode(seed)).as_bytes())?;
                    file.sync_all()?;
                }
                Err(e)
                    if e.downcast_ref::<std::io::Error>()
                        .is_some_and(|e| e.kind() == std::io::ErrorKind::AlreadyExists) => {}
                Err(e) => return Err(e),
            }
        }
        let text = read(&path, 4096)?;
        let parts: Vec<_> = text.split_whitespace().collect();
        if parts.len() != 2 || parts[1].len() > 256 {
            bail!("invalid sender identity file");
        }
        let seed: [u8; 32] = hex::decode(parts[0])
            .ok()
            .and_then(|v| v.try_into().ok())
            .context("invalid sender identity seed")?;
        let controller = pair::Identity::from_seed(parts[1], &seed);
        let mut peers = BTreeMap::new();
        let records = dir.join("receivers");
        if exists(&records)? {
            for line in read(&records, 1_048_576)?.lines() {
                let f: Vec<_> = line.split_whitespace().collect();
                if f.len() != 3 {
                    bail!("invalid receiver pairing record");
                }
                let id = uid(&String::from_utf8(hex::decode(f[0])?)?)?;
                let accessory = hex::decode(f[1])?;
                let key: [u8; 32] = hex::decode(f[2])
                    .ok()
                    .and_then(|v| v.try_into().ok())
                    .context("invalid receiver public key")?;
                if accessory.is_empty()
                    || accessory.len() > 256
                    || peers.insert(id, Peer { id: accessory, key }).is_some()
                    || peers.len() > 1024
                {
                    bail!("invalid or duplicate receiver pairing");
                }
            }
        }
        Ok(Self {
            dir: dir.into(),
            controller,
            peers: Mutex::new(peers),
        })
    }
    pub fn controller(&self) -> pair::Identity {
        self.controller.clone()
    }
    pub fn get(&self, id: &str) -> Result<Option<PairingCredentials>> {
        let peers = self
            .peers
            .lock()
            .map_err(|_| anyhow::anyhow!("pairing store lock failed"))?;
        peers
            .get(&uid(id)?)
            .map(|p| PairingCredentials::new(self.controller.clone(), p.id.clone(), p.key))
            .transpose()
    }
    pub fn save(&self, id: &str, credentials: &PairingCredentials) -> Result<()> {
        if credentials.controller_id() != self.controller.pairing_id
            || credentials.controller_public_key() != self.controller.public_key()
        {
            bail!("pairing belongs to another controller");
        }
        let mut peers = self
            .peers
            .lock()
            .map_err(|_| anyhow::anyhow!("pairing store lock failed"))?;
        let mut next = peers.clone();
        next.insert(
            uid(id)?,
            Peer {
                id: credentials.accessory_id().to_vec(),
                key: credentials.accessory_public_key(),
            },
        );
        if next.len() > 1024 {
            bail!("too many stored receivers");
        }
        self.write(&next)?;
        *peers = next;
        Ok(())
    }
    pub fn remove(&self, id: &str) -> Result<bool> {
        let mut peers = self
            .peers
            .lock()
            .map_err(|_| anyhow::anyhow!("pairing store lock failed"))?;
        let mut next = peers.clone();
        if next.remove(&uid(id)?).is_none() {
            return Ok(false);
        }
        self.write(&next)?;
        *peers = next;
        Ok(true)
    }
    fn write(&self, peers: &BTreeMap<String, Peer>) -> Result<()> {
        private(&self.dir, true)?;
        let path = self.dir.join("receivers");
        if exists(&path)? {
            private(&path, false)?;
        }
        let temporary = self
            .dir
            .join(format!("receivers.{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = create_file(&temporary)?;
            let mut bytes = 0usize;
            for (id, p) in peers {
                bytes += id.len() * 2 + p.id.len() * 2 + 64 + 3;
                if bytes > 1_048_576 {
                    bail!("pairing state exceeds size limit");
                }
                writeln!(
                    file,
                    "{} {} {}",
                    hex::encode(id),
                    hex::encode(&p.id),
                    hex::encode(p.key)
                )?;
            }
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &path)?;
            #[cfg(unix)]
            File::open(&self.dir)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(
                std::env::temp_dir().join(format!("airplay-sender-store-{}", uuid::Uuid::new_v4())),
            )
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn restart_preserves_keys_and_rejects_another_controller() {
        let dir = Temp::new();
        let store = SenderStore::open(&dir.0).unwrap();
        let public = store.controller().public_key();
        let credentials =
            PairingCredentials::new(store.controller(), b"accessory".to_vec(), [8; 32]).unwrap();
        let id = "ABCDEF01-2345-6789-ABCD-EF0123456789";
        store.save(id, &credentials).unwrap();
        drop(store);
        let store = SenderStore::open(&dir.0).unwrap();
        assert_eq!(store.controller().public_key(), public);
        assert_eq!(
            store
                .get(&id.to_ascii_lowercase())
                .unwrap()
                .unwrap()
                .accessory_id(),
            b"accessory"
        );
        let other = PairingCredentials::new(
            pair::Identity::from_seed("other", &[9; 32]),
            b"accessory".to_vec(),
            [8; 32],
        )
        .unwrap();
        assert!(store.save("other", &other).is_err());
        assert!(store.get("other").unwrap().is_none());
        assert!(store.remove(id).unwrap());
        drop(store);
        assert!(
            SenderStore::open(&dir.0)
                .unwrap()
                .get(id)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn oversized_write_keeps_previous_records() {
        let dir = Temp::new();
        let store = SenderStore::open(&dir.0).unwrap();
        let credentials =
            PairingCredentials::new(store.controller(), b"accessory".to_vec(), [8; 32]).unwrap();
        store.save("receiver", &credentials).unwrap();
        let before = fs::read(dir.0.join("receivers")).unwrap();
        let mut peers = BTreeMap::new();
        for index in 0..1024 {
            peers.insert(
                format!("{index:0256}"),
                Peer {
                    id: vec![8; 256],
                    key: [8; 32],
                },
            );
        }
        assert!(store.write(&peers).is_err());
        assert_eq!(fs::read(dir.0.join("receivers")).unwrap(), before);
        assert!(
            SenderStore::open(&dir.0)
                .unwrap()
                .get("receiver")
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn corrupt_records_do_not_rotate_identity() {
        let dir = Temp::new();
        let store = SenderStore::open(&dir.0).unwrap();
        let identity = fs::read(dir.0.join("identity")).unwrap();
        let credentials =
            PairingCredentials::new(store.controller(), b"accessory".to_vec(), [8; 32]).unwrap();
        store.save("receiver", &credentials).unwrap();
        drop(store);
        fs::write(dir.0.join("receivers"), "broken").unwrap();
        assert!(SenderStore::open(&dir.0).is_err());
        assert_eq!(fs::read(dir.0.join("identity")).unwrap(), identity);
        fs::write(dir.0.join("identity"), "broken identity").unwrap();
        assert!(SenderStore::open(&dir.0).is_err());
        assert_eq!(
            fs::read_to_string(dir.0.join("identity")).unwrap(),
            "broken identity"
        );
    }
    #[cfg(unix)]
    #[test]
    fn files_are_private_and_symlinks_are_rejected() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
        let dir = Temp::new();
        let store = SenderStore::open(&dir.0).unwrap();
        assert_eq!(fs::metadata(&dir.0).unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            fs::metadata(dir.0.join("identity")).unwrap().mode() & 0o777,
            0o600
        );
        let c =
            PairingCredentials::new(store.controller(), b"accessory".to_vec(), [8; 32]).unwrap();
        store.save("receiver", &c).unwrap();
        assert_eq!(
            fs::metadata(dir.0.join("receivers")).unwrap().mode() & 0o777,
            0o600
        );
        fs::set_permissions(dir.0.join("receivers"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(SenderStore::open(&dir.0).is_err());
        fs::set_permissions(dir.0.join("receivers"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(dir.0.join("receivers")).unwrap();
        symlink("missing", dir.0.join("receivers")).unwrap();
        assert!(SenderStore::open(&dir.0).is_err());
        assert!(store.save("receiver", &c).is_err());
        assert!(store.save("new-receiver", &c).is_err());
        assert!(store.get("new-receiver").unwrap().is_none());
    }
}
