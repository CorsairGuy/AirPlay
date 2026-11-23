//! Read the MSYS2 NQPTP shared file. Parsing alone does not qualify a clock.
//! Wire reference: mikebrady/nqptp, GPL-2.0; independent implementation.
use clock::{external::Correlation, nqptp::Snapshot};
use std::{
    fs::{File, OpenOptions},
    io,
    net::{IpAddr, SocketAddr, UdpSocket},
    os::windows::{
        fs::{FileExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::{Path, PathBuf},
    sync::atomic::{Ordering, fence},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct ReceiverState {
    owner: Option<u128>,
    generation: u64,
    sample: Option<Snapshot>,
    correlation: Option<Correlation>,
    last_epoch: Option<(u64, u64)>,
    file_identity: Option<(u32, u64)>,
}

/// An explicitly configured, single-active-session NQPTP follower. The caller
/// supplies a bounded sampler of the daemon's actual monotonic clock.
pub struct Receiver {
    state: Arc<Mutex<ReceiverState>>,
    control: Mutex<Control>,
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .owner
            .is_some()
        {
            let _ = self
                .control
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .stop();
        }
    }
}

impl Receiver {
    pub fn start(
        path: PathBuf,
        control: Control,
        mut sample_clock: impl FnMut() -> io::Result<u64> + Send + 'static,
    ) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NQPTP path must be absolute",
            ));
        }
        let state = Arc::new(Mutex::new(ReceiverState {
            owner: None,
            generation: 0,
            sample: None,
            correlation: None,
            last_epoch: None,
            file_identity: None,
        }));
        let weak = Arc::downgrade(&state);
        std::thread::spawn(move || {
            let mut first: Option<crate::raw_correlation::Bracket> = None;
            let mut seen_generation = 0;
            let mut reported_generation = 0;
            loop {
                let Some(shared) = weak.upgrade() else { break };
                let (owner, generation) = {
                    let s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    (s.owner, s.generation)
                };
                if generation != seen_generation {
                    first = None;
                    seen_generation = generation;
                }
                if owner.is_none() {
                    first = None;
                    drop(shared);
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                let bracket = (0..5)
                    .filter_map(|_| {
                        crate::raw_correlation::capture_with(
                            &mut sample_clock,
                            1_000,
                            1_000,
                            20_000,
                            Duration::from_millis(10),
                        )
                        .ok()
                    })
                    .min_by_key(|b| b.uncertainty_ns());
                let sample = Reader::open(&path)
                    .and_then(|r| r.snapshot().map(|sample| (r.identity, sample)));
                let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                if s.owner == owner && s.generation == generation {
                    if let Some(bracket) = bracket.filter(|b| b.uncertainty_ns() <= 500_000) {
                        if let Some(previous) = first {
                            if bracket.at().duration_since(previous.at()) >= Duration::from_secs(2)
                            {
                                let result = crate::raw_correlation::estimate(
                                    previous,
                                    bracket,
                                    1_000,
                                    Duration::from_secs(4),
                                    Duration::from_millis(10),
                                );
                                if let Err(ref error) = result {
                                    tracing::debug!(%error, "NQPTP clock calibration failed");
                                }
                                s.correlation = result.ok();
                                first = Some(bracket);
                            }
                        } else {
                            first = Some(bracket);
                        }
                    }
                    let next = sample.ok();
                    if let Some((identity, _)) = next {
                        if s.file_identity.is_some_and(|previous| previous != identity) {
                            s.owner = None;
                            s.sample = None;
                            s.correlation = None;
                            s.generation = s.generation.saturating_add(1);
                            first = None;
                            tracing::warn!("NQPTP shared file replaced; session clock revoked");
                            drop(s);
                            drop(shared);
                            std::thread::sleep(Duration::from_millis(200));
                            continue;
                        }
                        s.file_identity = Some(identity);
                    }
                    let next = next.map(|(_, sample)| sample);
                    let next_epoch = next.map(|v| (v.clock_id, v.master_start_time));
                    if s.last_epoch
                        .zip(next_epoch)
                        .is_some_and(|(old, new)| old != new)
                    {
                        s.generation = s.generation.saturating_add(1);
                        s.correlation = None;
                        first = None;
                    }
                    if next_epoch.is_some() {
                        s.last_epoch = next_epoch;
                    }
                    s.sample = next;
                    if s.sample.is_some()
                        && s.correlation.is_some()
                        && reported_generation != s.generation
                    {
                        tracing::info!("NQPTP follower sample and clock calibration available");
                        reported_generation = s.generation;
                    }
                } else {
                    first = None;
                }
                drop(s);
                drop(shared);
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        Ok(Self {
            state,
            control: Mutex::new(control),
        })
    }

    pub fn select(&self, owner: u128, master: IpAddr) -> io::Result<()> {
        if owner == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid session owner",
            ));
        }
        let control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.generation = state
            .generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("session generation exhausted"))?;
        state.owner = None;
        state.sample = None;
        state.correlation = None;
        state.last_epoch = None;
        state.file_identity = None;
        control.select(master)?;
        state.owner = Some(owner);
        Ok(())
    }

    pub fn release(&self, owner: u128) -> io::Result<()> {
        let control = self.control.lock().unwrap_or_else(|e| e.into_inner());
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.owner != Some(owner) {
            return Ok(());
        }
        state.owner = None;
        state.sample = None;
        state.correlation = None;
        state.last_epoch = None;
        state.file_identity = None;
        state.generation = state.generation.saturating_add(1);
        control.stop()
    }

    pub fn reading(&self) -> Option<clock::domain::Reading> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let sample = s.sample?;
        let correlation = s.correlation?;
        let before = clock::now_ns();
        let at = Instant::now();
        let after = clock::now_ns();
        let local = before.checked_add(after.checked_sub(before)? / 2)?;
        let offset = follower_offset(
            sample,
            &correlation,
            at,
            at,
            local,
            Duration::from_secs(2),
            10_000_000,
        )
        .ok()?;
        let ns = u64::try_from(i128::from(local) + i128::from(offset)).ok()?;
        let domain = clock::domain::Domain::new(sample.clock_id, s.generation).ok()?;
        Some(domain.reading(ns))
    }
}

pub struct Reader {
    file: File,
    path: PathBuf,
    identity: (u32, u64),
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetFileInformationByHandle(
        handle: std::os::windows::io::RawHandle,
        data: *mut [u32; 13],
    ) -> i32;
}

fn file_identity(file: &File) -> io::Result<(u32, u64)> {
    let mut data = [0u32; 13];
    // SAFETY: the handle remains open and the output buffer is the Win32
    // BY_HANDLE_FILE_INFORMATION layout, thirteen naturally aligned DWORDs.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut data) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((data[7], (u64::from(data[11]) << 32) | u64::from(data[12])))
}

fn open_shared(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2 | 0x4)
        .open(path)
}

/// Convert a fresh follower sample only after the caller has measured the
/// external clock correlation and independently checked the daemon epoch.
pub fn follower_offset(
    snapshot: Snapshot,
    correlation: &Correlation,
    produced: Instant,
    now: Instant,
    local_ns: u64,
    max_age: Duration,
    max_uncertainty_ns: u64,
) -> io::Result<i64> {
    if snapshot.clock_id == 0
        || snapshot.local_time == 0
        || max_age.is_zero()
        || max_age > Duration::from_secs(10)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid NQPTP follower sample",
        ));
    }
    let raw_now = correlation
        .raw_at(produced, now, max_uncertainty_ns)
        .map_err(io::Error::other)?;
    let age = raw_now
        .checked_sub(snapshot.local_time)
        .ok_or_else(|| io::Error::other("NQPTP sample is in the future"))?;
    if u128::from(age) > max_age.as_nanos() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "NQPTP sample expired",
        ));
    }
    let master_ns = snapshot.master_time(raw_now);
    i64::try_from(i128::from(master_ns) - i128::from(local_ns)).map_err(io::Error::other)
}

/// Control one explicitly named receiver clock. This does not attest the daemon
/// or assert that a selected clock has been published.
pub struct Control {
    socket: UdpSocket,
    destination: SocketAddr,
    name: String,
}

impl Control {
    pub fn new(destination: SocketAddr, name: &str) -> io::Result<Self> {
        if !destination.ip().is_loopback()
            || destination.port() == 0
            || !destination.is_ipv4()
            || !name.starts_with('/')
            || name.len() > 128
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"/-_".contains(&b))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid NQPTP control endpoint or name",
            ));
        }
        Ok(Self {
            socket: UdpSocket::bind("127.0.0.1:0")?,
            destination,
            name: name.into(),
        })
    }

    fn send(&self, command: &str) -> io::Result<()> {
        let message = format!("{} {}\0", self.name, command);
        if self.socket.send_to(message.as_bytes(), self.destination)? != message.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short NQPTP command",
            ));
        }
        Ok(())
    }

    pub fn select(&self, master: IpAddr) -> io::Result<()> {
        if master.is_unspecified() || master.is_multicast() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid NQPTP master",
            ));
        }
        self.send(&format!("T {}", master.to_canonical()))?;
        if let Err(error) = self.send("B") {
            let _ = self.send("T");
            return Err(error);
        }
        Ok(())
    }

    pub fn stop(&self) -> io::Result<()> {
        let end = self.send("E");
        let clear = self.send("T");
        end.and(clear)
    }
}

impl Reader {
    pub fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() || !std::fs::symlink_metadata(path)?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NQPTP file must be an absolute regular path",
            ));
        }
        // MSYS maps this file writable and keeps it open.
        let file = open_shared(path)?;
        if !matches!(file.metadata()?.len(), 72 | 88) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported NQPTP size",
            ));
        }
        let identity = file_identity(&file)?;
        let this = Self {
            file,
            path: path.to_path_buf(),
            identity,
        };
        this.check_identity()?;
        Ok(this)
    }

    fn check_identity(&self) -> io::Result<()> {
        if !std::fs::symlink_metadata(&self.path)?.is_file()
            || file_identity(&self.file)? != self.identity
            || file_identity(&open_shared(&self.path)?)? != self.identity
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "NQPTP file identity changed",
            ));
        }
        Ok(())
    }

    pub fn snapshot(&self) -> io::Result<Snapshot> {
        self.check_identity()?;
        let len = self.file.metadata()?.len() as usize;
        let width = match len {
            72 => 32,
            88 => 40,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "NQPTP size changed",
                ));
            }
        };
        let mut buf = [0u8; 88];
        read_exact_at(&self.file, &mut buf[..2], 0)?;
        read_exact_at(&self.file, &mut buf[8..8 + width], 8)?;
        fence(Ordering::SeqCst);
        read_exact_at(
            &self.file,
            &mut buf[8 + width..8 + 2 * width],
            (8 + width) as u64,
        )?;
        fence(Ordering::SeqCst);
        let mut header = [0; 2];
        read_exact_at(&self.file, &mut header, 0)?;
        if header != buf[..2] || self.file.metadata()?.len() != len as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "NQPTP file changed",
            ));
        }
        self.check_identity()?;
        if !matches!((u16::from_ne_bytes(header), len), (10, 72) | (11, 88)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "NQPTP ABI mismatch",
            ));
        }
        Snapshot::parse(&buf[..len]).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "NQPTP sample torn or unpublished",
            )
        })
    }
}

fn read_exact_at(file: &File, mut buf: &mut [u8], mut pos: u64) -> io::Result<()> {
    while !buf.is_empty() {
        match file.seek_read(buf, pos) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "NQPTP sample truncated",
                ));
            }
            Ok(n) => {
                pos += n as u64;
                buf = &mut buf[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follower_mapping_rejects_stale_or_future_samples() {
        let now = Instant::now();
        let sample = Snapshot {
            version: 11,
            clock_id: 7,
            local_time: 990,
            offset: (-20i64) as u64,
            clock_change_offset: Some(0),
            master_start_time: 100,
        };
        let correlation = Correlation {
            at: now,
            raw_ns: 1000,
            rate_ppb: 0,
            rate_error_ppb: 0,
            uncertainty_ns: 1,
            valid_for: Duration::from_secs(1),
        };
        assert_eq!(
            follower_offset(
                sample,
                &correlation,
                now,
                now,
                900,
                Duration::from_nanos(20),
                10
            )
            .unwrap(),
            80
        );
        assert_eq!(
            follower_offset(
                sample,
                &correlation,
                now,
                now,
                900,
                Duration::from_nanos(5),
                10
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::TimedOut
        );
        let future = Snapshot {
            local_time: 1001,
            ..sample
        };
        assert!(
            follower_offset(
                future,
                &correlation,
                now,
                now,
                900,
                Duration::from_nanos(20),
                10
            )
            .is_err()
        );
    }

    #[test]
    fn receiver_control_sends_named_start_and_cleanup() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let control = Control::new(server.local_addr().unwrap(), "/test-clock").unwrap();
        control.select("192.0.2.8".parse().unwrap()).unwrap();
        control.stop().unwrap();
        let mut buf = [0u8; 256];
        for expected in [
            "/test-clock T 192.0.2.8\0",
            "/test-clock B\0",
            "/test-clock E\0",
            "/test-clock T\0",
        ] {
            let (n, _) = server.recv_from(&mut buf).unwrap();
            assert_eq!(&buf[..n], expected.as_bytes());
        }
        assert!(Control::new("192.0.2.1:9000".parse().unwrap(), "/x").is_err());
        assert!(control.select("0.0.0.0".parse().unwrap()).is_err());
    }
    #[test]
    fn reads_shared_sample_and_rejects_torn_copy() {
        let path =
            std::env::temp_dir().join(format!("codex-nqptp-reader-{}.bin", std::process::id()));
        let mut b = [0u8; 88];
        b[..2].copy_from_slice(&11u16.to_ne_bytes());
        for start in [8, 48] {
            for (i, v) in [5u64, 100, 7, 0, 20].into_iter().enumerate() {
                b[start + i * 8..start + (i + 1) * 8].copy_from_slice(&v.to_ne_bytes());
            }
        }
        std::fs::write(&path, b).unwrap();
        let reader = Reader::open(&path).unwrap();
        assert_eq!(reader.snapshot().unwrap().master_time(100), 107);
        b[48] ^= 1;
        std::fs::write(&path, b).unwrap();
        assert_eq!(
            reader.snapshot().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        drop(reader);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_replaced_clock_file() {
        let path =
            std::env::temp_dir().join(format!("codex-nqptp-replaced-{}.bin", std::process::id()));
        let old = path.with_extension("old");
        let mut b = [0u8; 88];
        b[..2].copy_from_slice(&11u16.to_ne_bytes());
        std::fs::write(&path, b).unwrap();
        let reader = Reader::open(&path).unwrap();
        std::fs::rename(&path, &old).unwrap();
        std::fs::write(&path, b).unwrap();
        assert_eq!(
            reader.snapshot().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        drop(reader);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(old).unwrap();
    }

    #[test]
    fn receiver_revokes_when_clock_file_is_replaced() {
        let path =
            std::env::temp_dir().join(format!("codex-nqptp-owner-{}.bin", std::process::id()));
        let old = path.with_extension("old");
        let mut b = [0u8; 88];
        b[..2].copy_from_slice(&11u16.to_ne_bytes());
        for start in [8, 48] {
            for (i, v) in [7u64, 1_000_000_000, 0, 0, 1].into_iter().enumerate() {
                b[start + i * 8..start + (i + 1) * 8].copy_from_slice(&v.to_ne_bytes());
            }
        }
        std::fs::write(&path, b).unwrap();
        let endpoint = UdpSocket::bind("127.0.0.1:0").unwrap();
        let control = Control::new(endpoint.local_addr().unwrap(), "/replacement-test").unwrap();
        let start = Instant::now();
        let receiver = Receiver::start(path.clone(), control, move || {
            Ok(1_000_000_000 + start.elapsed().as_nanos() as u64)
        })
        .unwrap();
        receiver.select(1, "127.0.0.1".parse().unwrap()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while receiver.state.lock().unwrap().file_identity.is_none() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        std::fs::rename(&path, &old).unwrap();
        std::fs::write(&path, b).unwrap();
        while receiver.state.lock().unwrap().owner.is_some() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(receiver.reading().is_none());
        drop(receiver);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(old).unwrap();
    }

    #[test]
    #[ignore = "requires a running MSYS2 NQPTP test client"]
    fn reads_live_unpublished_file() {
        let path = std::env::var_os("AIRPLAY_NQPTP_TEST_FILE").expect("test file path");
        let reader = Reader::open(Path::new(&path)).unwrap();
        assert_eq!(
            reader.snapshot().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    #[ignore = "requires the local synthetic PTP master and NQPTP"]
    fn reads_live_published_file() {
        let path = std::env::var_os("AIRPLAY_NQPTP_TEST_FILE").expect("test file path");
        let reader = Reader::open(Path::new(&path)).unwrap();
        let sample = reader.snapshot().unwrap();
        assert_eq!(sample.clock_id, 0x1020304050607080);
        assert!(sample.local_time > 0);
    }

    #[test]
    #[ignore = "requires NQPTP and the local synthetic PTP master"]
    fn control_drives_live_receiver_clock() {
        let path = std::env::var_os("AIRPLAY_NQPTP_TEST_FILE").expect("test file path");
        let peer: IpAddr = std::env::var("AIRPLAY_NQPTP_TEST_PEER")
            .unwrap()
            .parse()
            .unwrap();
        let control =
            Control::new("127.0.0.1:9000".parse().unwrap(), "/codex-receiver-check").unwrap();
        control.select(peer).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut found = false;
        while Instant::now() < deadline {
            if let Ok(reader) = Reader::open(Path::new(&path))
                && let Ok(sample) = reader.snapshot()
                && sample.clock_id == 0x1020304050607080
            {
                found = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        control.stop().unwrap();
        assert!(found, "NQPTP did not publish the test clock");
    }

    #[test]
    #[ignore = "requires the local PTP source and MSYS2 clock test helper"]
    fn live_follower_uses_measured_clock_correlation() {
        use std::{
            io::{BufRead, BufReader, Write},
            process::{Command, Stdio},
        };
        let perl = std::env::var_os("AIRPLAY_NQPTP_TEST_PERL").unwrap();
        let worker = std::env::var_os("AIRPLAY_NQPTP_TEST_WORKER").unwrap();
        let path = std::env::var_os("AIRPLAY_NQPTP_TEST_FILE").unwrap();
        let peer: IpAddr = std::env::var("AIRPLAY_NQPTP_TEST_PEER")
            .unwrap()
            .parse()
            .unwrap();
        let mut child = Command::new(perl)
            .args([
                "-I",
                "C:/msys64/usr/lib/perl5/core_perl",
                "-I",
                "C:/msys64/usr/share/perl5/core_perl",
            ])
            .arg(worker)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut read_clock = || -> io::Result<u64> {
            input.write_all(b"\n")?;
            input.flush()?;
            let mut line = String::new();
            output.read_line(&mut line)?;
            let seconds: f64 = line.trim().parse().map_err(io::Error::other)?;
            Ok((seconds * 1_000_000_000.0) as u64)
        };
        read_clock().unwrap(); // process startup is outside the measured bracket
        let control =
            Control::new("127.0.0.1:9000".parse().unwrap(), "/codex-receiver-check").unwrap();
        control.select(peer).unwrap();
        let first = crate::raw_correlation::capture_with(
            &mut read_clock,
            1_000,
            1_000,
            20_000,
            Duration::from_millis(10),
        )
        .unwrap();
        std::thread::sleep(Duration::from_secs(2));
        let second = crate::raw_correlation::capture_with(
            &mut read_clock,
            1_000,
            1_000,
            20_000,
            Duration::from_millis(10),
        )
        .unwrap();
        let correlation = crate::raw_correlation::estimate(
            first,
            second,
            1000,
            Duration::from_secs(2),
            Duration::from_millis(10),
        )
        .unwrap();
        let reader = Reader::open(Path::new(&path)).unwrap();
        let sample = reader.snapshot().unwrap();
        assert_eq!(sample.clock_id, 0x1020304050607080);
        let before = clock::now_ns();
        let instant = Instant::now();
        let after = clock::now_ns();
        let offset = follower_offset(
            sample,
            &correlation,
            instant,
            instant,
            before + (after - before) / 2,
            Duration::from_secs(2),
            10_000_000,
        )
        .unwrap();
        assert_ne!(offset, 0);
        control.stop().unwrap();
        drop(read_clock);
        drop(input);
        let _ = child.wait();
    }

    #[test]
    #[ignore = "requires NQPTP, local PTP master and MSYS2 clock test helper"]
    fn live_receiver_tracks_and_releases_clock() {
        use std::{
            io::{BufRead, BufReader, Write},
            process::{Command, Stdio},
        };
        let perl = std::env::var_os("AIRPLAY_NQPTP_TEST_PERL").unwrap();
        let worker = std::env::var_os("AIRPLAY_NQPTP_TEST_WORKER").unwrap();
        let path = std::env::var_os("AIRPLAY_NQPTP_TEST_FILE").unwrap();
        let peer: IpAddr = std::env::var("AIRPLAY_NQPTP_TEST_PEER")
            .unwrap()
            .parse()
            .unwrap();
        let mut child = Command::new(perl)
            .args([
                "-I",
                "C:/msys64/usr/lib/perl5/core_perl",
                "-I",
                "C:/msys64/usr/share/perl5/core_perl",
            ])
            .arg(worker)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut sample = move || -> io::Result<u64> {
            input.write_all(b"\n")?;
            input.flush()?;
            let mut line = String::new();
            output.read_line(&mut line)?;
            let seconds: f64 = line.trim().parse().map_err(io::Error::other)?;
            Ok((seconds * 1_000_000_000.0) as u64)
        };
        sample().unwrap();
        let control =
            Control::new("127.0.0.1:9000".parse().unwrap(), "/codex-receiver-check").unwrap();
        let receiver = Receiver::start(Path::new(&path).to_path_buf(), control, sample).unwrap();
        receiver.select(1, peer).unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let reading = loop {
            if let Some(reading) = receiver.reading() {
                break reading;
            }
            if Instant::now() >= deadline {
                let state = receiver.state.lock().unwrap();
                panic!(
                    "NQPTP not ready: sample={} correlation={} generation={}",
                    state.sample.is_some(),
                    state.correlation.is_some(),
                    state.generation
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(reading.domain().id(), 0x1020304050607080);
        receiver.release(2).unwrap();
        assert!(receiver.reading().is_some());
        receiver.release(1).unwrap();
        assert!(receiver.reading().is_none());
        drop(receiver);
        let _ = child.wait();
    }
}
