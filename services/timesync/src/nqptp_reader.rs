//! Explicit read-only NQPTP snapshot reader. No mmap, daemon commands or sockets.
//! Protocol reference (ideas/layout only; implementation written independently):
//! mikebrady/nqptp ab8fb67483411640fd7f1e193889482f2421c37c,
//! nqptp-shm-structures.h; provenance: declared GPL-2.0.
//! Main is read BEFORE secondary with a full ordering fence, then compared by
//! the existing codec. No mapping means truncation cannot SIGBUS this process.
//! File identity is not daemon/role attestation; the owner supplies that evidence.
use clock::nqptp::Snapshot;
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::{
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::atomic::{Ordering, fence},
};

pub struct Reader {
    file: File,
    path: PathBuf,
    device: u64,
    inode: u64,
    owner: u32,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn exact(file: &File, mut data: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !data.is_empty() {
        match file.read_at(data, offset) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "NQPTP snapshot truncated",
                ));
            }
            Ok(n) => {
                offset += n as u64;
                data = &mut data[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
impl Reader {
    /// Explicit absolute path and expected daemon uid; no guessed default SHM.
    pub fn open(path: &Path, owner: u32) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(invalid("NQPTP path must be absolute"));
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.uid() != owner || !matches!(metadata.len(), 72 | 88) {
            return Err(invalid("invalid NQPTP file type, owner or size"));
        }
        let reader = Self {
            file,
            path: path.into(),
            device: metadata.dev(),
            inode: metadata.ino(),
            owner,
        };
        reader.identity()?;
        Ok(reader)
    }
    fn identity(&self) -> io::Result<u64> {
        let fd = self.file.metadata()?;
        let path = std::fs::symlink_metadata(&self.path)?;
        if !path.is_file()
            || path.file_type().is_symlink()
            || !fd.is_file()
            || fd.uid() != self.owner
            || path.uid() != self.owner
            || fd.dev() != self.device
            || path.dev() != self.device
            || fd.ino() != self.inode
            || path.ino() != self.inode
            || fd.len() != path.len()
        {
            return Err(invalid("NQPTP file identity changed"));
        }
        Ok(fd.len())
    }
    /// One bounded attempt. Torn/unpublished copies return WouldBlock, not an
    /// invented clock. The owner decides retry deadline and revocation policy.
    /// Same-inode daemon restarts still require external instance-token evidence.
    pub fn snapshot(&self) -> io::Result<Snapshot> {
        let size = self.identity()?;
        let mut header = [0; 2];
        exact(&self.file, &mut header, 0)?;
        let version = u16::from_ne_bytes(header);
        let width = match version {
            10 => 32,
            11 => 40,
            _ => return Err(invalid("unsupported NQPTP ABI")),
        };
        if size != 8 + 2 * width {
            return Err(invalid("NQPTP ABI and file size disagree"));
        }
        let mut bytes = [0u8; 88];
        bytes[..2].copy_from_slice(&header);
        exact(&self.file, &mut bytes[8..8 + width as usize], 8)?;
        fence(Ordering::SeqCst);
        exact(
            &self.file,
            &mut bytes[8 + width as usize..8 + 2 * width as usize],
            8 + width,
        )?;
        fence(Ordering::SeqCst);
        let mut after = [0; 2];
        exact(&self.file, &mut after, 0)?;
        if after != header || self.identity()? != size {
            return Err(invalid("NQPTP ABI/file changed during read"));
        }
        Snapshot::parse(&bytes[..size as usize]).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "torn or unpublished NQPTP snapshot",
            )
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::{fs, io::Write, os::unix::fs::symlink};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "nqptp-reader-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn bytes(version: u16) -> Vec<u8> {
        let fields = if version == 10 {
            vec![9u64, 1_000, 0, 100]
        } else {
            vec![9u64, 1_000, 0, 0, 100]
        };
        let mut out = vec![0; 8 + fields.len() * 16];
        out[..2].copy_from_slice(&version.to_ne_bytes());
        for copy in 0..2 {
            for (i, value) in fields.iter().enumerate() {
                let at = 8 + copy * fields.len() * 8 + i * 8;
                out[at..at + 8].copy_from_slice(&value.to_ne_bytes());
            }
        }
        out
    }
    fn owner() -> u32 {
        unsafe { libc::geteuid() }
    }
    #[test]
    fn both_abis_read_without_mapping_or_modifying_fixture() {
        let t = Temp::new();
        for version in [10, 11] {
            let p = t.0.join(version.to_string());
            let data = bytes(version);
            fs::write(&p, &data).unwrap();
            let s = Reader::open(&p, owner()).unwrap().snapshot().unwrap();
            assert_eq!(s.clock_id, 9);
            assert_eq!(s.version, version);
            assert_eq!(fs::read(p).unwrap(), data);
        }
    }
    #[test]
    fn torn_unpublished_and_wrong_abi_fail() {
        let t = Temp::new();
        let p = t.0.join("snapshot");
        let mut data = bytes(10);
        data[40] ^= 1;
        fs::write(&p, &data).unwrap();
        let r = Reader::open(&p, owner()).unwrap();
        assert_eq!(r.snapshot().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        data[40] ^= 1;
        data[8..16].fill(0);
        data[40..48].fill(0);
        fs::write(&p, &data).unwrap();
        assert!(r.snapshot().is_err());
        data[..2].copy_from_slice(&12u16.to_ne_bytes());
        fs::write(&p, &data).unwrap();
        assert_eq!(r.snapshot().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }
    #[test]
    fn replacement_truncation_and_symlink_are_rejected() {
        let t = Temp::new();
        let p = t.0.join("snapshot");
        fs::write(&p, bytes(10)).unwrap();
        let r = Reader::open(&p, owner()).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&p)
            .unwrap()
            .set_len(1)
            .unwrap();
        assert!(r.snapshot().is_err());
        fs::remove_file(&p).unwrap();
        let mut f = File::create(&p).unwrap();
        f.write_all(&bytes(10)).unwrap();
        assert!(r.snapshot().is_err());
        let alias = t.0.join("alias");
        symlink(&p, &alias).unwrap();
        assert!(Reader::open(&alias, owner()).is_err());
        assert!(Reader::open(&p, owner().wrapping_add(1)).is_err());
        assert!(Reader::open(&t.0, owner()).is_err());
    }
}
