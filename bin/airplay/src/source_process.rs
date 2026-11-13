//! Explicit raw-PCM process acquisition. No shell, decoder, route or device default.
use anyhow::{Context, Result, bail};
use std::{
    io,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    task::{Context as TaskContext, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, ReadBuf},
    process::{Child, ChildStdout, Command},
    time::timeout,
};

pub struct Options {
    pub program: PathBuf,
    pub args: Vec<String>,
}
impl Options {
    pub fn validate(&self) -> Result<()> {
        if !self.program.is_absolute()
            || self.args.len() > 64
            || self
                .args
                .iter()
                .any(|arg| arg.len() > 4096 || arg.contains('\0'))
        {
            bail!("invalid PCM acquisition command");
        }
        Ok(())
    }
}
/// Keeps the capture child alive with its reader. Dropping a pipeline kills the
/// child; normal callers must await finish/stop to reap it before replacing it.
/// Output must already be S16BE/44.1kHz/stereo, as required by continuous_source.
pub struct Capture {
    child: Child,
    stdout: ChildStdout,
}
impl Capture {
    pub fn spawn(options: Options) -> Result<Self> {
        options.validate()?;
        let mut child = Command::new(options.program)
            .args(options.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("start PCM acquisition")?;
        let stdout = child
            .stdout
            .take()
            .context("PCM acquisition stdout missing")?;
        Ok(Self { child, stdout })
    }
    /// Call after EOF; nonzero exit is a source error, never successful silence.
    pub async fn finish(mut self) -> Result<()> {
        let status = timeout(Duration::from_secs(1), self.child.wait())
            .await
            .context("PCM acquisition EOF deadline")??;
        if !status.success() {
            bail!("PCM acquisition failed: {status}");
        }
        Ok(())
    }
    pub async fn stop(mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child.start_kill()?;
        }
        timeout(Duration::from_secs(1), self.child.wait())
            .await
            .context("PCM acquisition stop deadline")??;
        Ok(())
    }
}
impl AsyncRead for Capture {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    #[tokio::test]
    async fn nonzero_capture_exit_is_not_successful_eof() {
        let mut source = Capture::spawn(Options {
            program: "/bin/false".into(),
            args: vec![],
        })
        .unwrap();
        let mut bytes = Vec::new();
        source.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
        assert!(source.finish().await.is_err());
    }
    #[tokio::test]
    async fn stop_reaps_capture_before_replacement() {
        let source = Capture::spawn(Options {
            program: "/bin/sleep".into(),
            args: vec!["60".into()],
        })
        .unwrap();
        let pid = source.child.id().unwrap();
        source.stop().await.unwrap();
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
    }
}
