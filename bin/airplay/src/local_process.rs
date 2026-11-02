//! Opt-in process adapter for a timestamped local renderer. No shell or device defaults.
use anyhow::{Context, Result, bail};
use sender::{AudioPacket, LocalPcmPipe, LocalSink};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    process::{Child, ChildStdin, Command},
    time::{sleep_until, timeout},
};

#[derive(Clone, Debug)]
pub struct Options {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub gain: u8,
    pub gate: Option<PathBuf>,
}
impl Options {
    pub fn validate(&self) -> Result<()> {
        if !self.program.is_absolute() {
            bail!("local renderer program must be an absolute path");
        }
        if self.gate.as_ref().is_some_and(|p| !p.is_absolute()) {
            bail!("local gate must be an absolute path");
        }
        if !(1..=4).contains(&self.gain)
            || self.args.len() > 64
            || self.args.iter().any(|v| v.len() > 4096)
        {
            bail!("invalid local renderer arguments or gain");
        }
        if !cfg!(target_os = "linux") {
            bail!("CLOCK_MONOTONIC process adapter currently requires Linux");
        }
        Ok(())
    }
}

pub struct Renderer {
    child: Child,
    pipe: Option<LocalPcmPipe<ChildStdin>>,
    last_end: Option<Instant>,
    gate: Option<PathBuf>,
}

#[cfg(target_os = "linux")]
fn clock_reference() -> Result<(Instant, u64)> {
    for _ in 0..8 {
        let before = Instant::now();
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: ts is writable and CLOCK_MONOTONIC is supported on Linux.
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let after = Instant::now();
        if after.duration_since(before) > Duration::from_micros(200) {
            continue;
        }
        let ns = u64::try_from(ts.tv_sec)?
            .checked_mul(1_000_000_000)
            .and_then(|v| v.checked_add(ts.tv_nsec as u64))
            .context("local kernel clock overflow")?;
        return Ok((before + after.duration_since(before) / 2, ns));
    }
    bail!("could not sample local clock without scheduler interruption")
}
#[cfg(not(target_os = "linux"))]
fn clock_reference() -> Result<(Instant, u64)> {
    bail!("local process clock unsupported on this platform")
}

impl Renderer {
    pub fn spawn(options: Options) -> Result<Self> {
        options.validate()?;
        let (reference, reference_ns) = clock_reference()?;
        let mut child = Command::new(&options.program)
            .args(&options.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("start local renderer")?;
        let stdin = child.stdin.take().context("local renderer stdin missing")?;
        let pipe = LocalPcmPipe::new(stdin, reference, reference_ns)?.with_gain(options.gain)?;
        Ok(Self {
            child,
            pipe: Some(pipe),
            last_end: None,
            gate: options.gate,
        })
    }
}
impl LocalSink for Renderer {
    async fn write(&mut self, packet: Arc<AudioPacket>, play_at: Instant) -> Result<()> {
        if self.child.try_wait()?.is_some() {
            bail!("local renderer exited before EOF");
        }
        self.last_end = play_at.checked_add(Duration::from_nanos(
            sender::FRAMES_PER_PACKET as u64 * 1_000_000_000 / u64::from(sender::SAMPLE_RATE),
        ));
        let pipe = self.pipe.as_mut().context("local renderer pipe closed")?;
        pipe.set_audible(self.gate.as_ref().is_none_or(|path| path.is_file()));
        pipe.write(packet, play_at).await
    }
    async fn finish(&mut self) -> Result<()> {
        self.pipe
            .as_mut()
            .context("local renderer pipe closed")?
            .finish()
            .await?;
        // Do not send EOF while the renderer still has scheduled sound to play.
        if let Some(end) = self.last_end {
            sleep_until(end.into()).await;
        }
        self.pipe.take();
        let status = timeout(Duration::from_millis(200), self.child.wait())
            .await
            .context("local renderer EOF deadline")??;
        if !status.success() {
            bail!("local renderer failed at EOF");
        }
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    fn cat() -> Options {
        Options {
            program: "/bin/cat".into(),
            args: Vec::new(),
            gain: 1,
            gate: None,
        }
    }
    #[tokio::test]
    async fn renderer_exits_after_scheduled_pcm_and_eof() {
        let mut renderer = Renderer::spawn(cat()).unwrap();
        let packet = Arc::new(
            sender::MediaClock::now()
                .packet(0, &[0; sender::PACKET_BYTES])
                .unwrap(),
        );
        let at = Instant::now() + Duration::from_millis(20);
        renderer.write(packet, at).await.unwrap();
        renderer.finish().await.unwrap();
        assert!(Instant::now() >= at);
        assert!(renderer.child.try_wait().unwrap().unwrap().success());
    }
    #[tokio::test]
    async fn dropping_renderer_kills_only_its_child() {
        let renderer = Renderer::spawn(cat()).unwrap();
        let pid = renderer.child.id().unwrap();
        drop(renderer);
        timeout(Duration::from_secs(1), async {
            loop {
                // SAFETY: signal zero only checks the existence of this test child.
                if unsafe { libc::kill(pid as libc::pid_t, 0) } < 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn refuses_relative_program_and_checks_clock_mapping() {
        let mut bad = cat();
        bad.program = "cat".into();
        assert!(bad.validate().is_err());
        let (reference, ns) = clock_reference().unwrap();
        assert!(ns > 0);
        assert!(reference.elapsed() < Duration::from_secs(1));
    }
}
