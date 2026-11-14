//! Linux daemon-instance attestation, separate from SHM and clock-role evidence.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
#[derive(PartialEq, Eq)]
struct Executable {
    dev: u64,
    ino: u64,
    len: u64,
    uid: u32,
    mode: u32,
    mtime: (i64, i64),
    ctime: (i64, i64),
}
fn executable(path: &Path) -> Result<Executable> {
    let m = fs::metadata(path)?;
    ensure!(
        m.is_file() && m.len() <= 32 * 1024 * 1024,
        "invalid daemon executable"
    );
    Ok(Executable {
        dev: m.dev(),
        ino: m.ino(),
        len: m.len(),
        uid: m.uid(),
        mode: m.mode(),
        mtime: (m.mtime(), m.mtime_nsec()),
        ctime: (m.ctime(), m.ctime_nsec()),
    })
}
fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "oversized daemon evidence");
    Ok(bytes)
}
fn start(path: &Path) -> Result<u64> {
    let bytes = bounded(&path.join("stat"), 4096)?;
    let text = std::str::from_utf8(&bytes)?;
    let (_, fields) = text.rsplit_once(") ").context("invalid daemon stat")?;
    Ok(fields
        .split_whitespace()
        .nth(19)
        .context("missing daemon start ticks")?
        .parse()?)
}
/// Hash is checked once against an explicit trusted build manifest. Poll checks
/// pinned process start and executable metadata. This is NOT role/port ownership
/// or proof of exclusive NQPTP command authority; those remain independent.
pub struct DaemonIdentity {
    process: PathBuf,
    uid: u32,
    start: u64,
    executable: Executable,
    instance: u128,
}
impl DaemonIdentity {
    pub fn verify(
        proc_root: &Path,
        pid: u32,
        process_uid: u32,
        executable_uid: u32,
        expected_sha256: [u8; 32],
    ) -> Result<Self> {
        ensure!(
            proc_root.is_absolute() && pid > 1,
            "invalid daemon process path/PID"
        );
        let process = proc_root.join(pid.to_string());
        ensure!(
            fs::metadata(&process)?.uid() == process_uid,
            "daemon process owner mismatch"
        );
        let started = start(&process)?;
        let exe = process.join("exe");
        let identity = executable(&exe)?;
        ensure!(
            identity.uid == executable_uid && identity.mode & 0o022 == 0,
            "daemon executable is not trusted-owned"
        );
        let bytes = bounded(&exe, 32 * 1024 * 1024)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        ensure!(digest == expected_sha256, "daemon build digest mismatch");
        ensure!(
            identity == executable(&exe)? && started == start(&process)?,
            "daemon changed during attestation"
        );
        let boot = bounded(&proc_root.join("sys/kernel/random/boot_id"), 64)?;
        ensure!(
            std::str::from_utf8(&boot)?.trim().len() == 36,
            "invalid boot identity"
        );
        let mut hash = Sha256::new();
        hash.update(&boot);
        hash.update(pid.to_be_bytes());
        hash.update(started.to_be_bytes());
        let digest = hash.finalize();
        let instance = u128::from_be_bytes(digest[..16].try_into()?);
        ensure!(instance != 0, "invalid daemon instance");
        let result = Self {
            process,
            uid: process_uid,
            start: started,
            executable: identity,
            instance,
        };
        result.check()?;
        Ok(result)
    }
    pub fn instance(&self) -> u128 {
        self.instance
    }
    pub fn check(&self) -> Result<u128> {
        ensure!(
            fs::metadata(&self.process)?.uid() == self.uid
                && start(&self.process)? == self.start
                && executable(&self.process.join("exe"))? == self.executable,
            "daemon instance/executable changed"
        );
        Ok(self.instance)
    }
}
