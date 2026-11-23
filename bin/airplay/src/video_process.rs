//! One viewer process per video stream; no global pipe surviving disconnect.
use mirror::{AnnexBWriter, Codec, VideoSink, VideoSinkFactory};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

pub fn factory(program: PathBuf, args: Vec<String>) -> Result<VideoSinkFactory, String> {
    if !program.is_absolute() || !program.is_file() {
        return Err("--video-player: expected an existing absolute executable path".into());
    }
    if args.len() > 64
        || args
            .iter()
            .any(|arg| arg.len() > 4096 || arg.contains('\0'))
    {
        return Err("--video-player-arg: too many or oversized arguments".into());
    }
    Ok(VideoSinkFactory::new(move |_| {
        let mut command = Command::new(&program);
        command
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        match command.spawn() {
            Ok(mut child) => {
                let input = child.stdin.take().expect("piped child stdin");
                tracing::info!(pid = child.id(), "video viewer opened");
                Box::new(Viewer {
                    writer: Some(AnnexBWriter::new(input)),
                    child: Some(child),
                })
            }
            Err(error) => {
                tracing::error!(%error, "cannot start video viewer");
                Box::new(mirror::CountingSink(0))
            }
        }
    }))
}

struct Viewer {
    writer: Option<AnnexBWriter<ChildStdin>>,
    child: Option<Child>,
}

impl Viewer {
    fn close(&mut self) {
        self.writer.take(); // EOF belongs to this stream, never the daemon stdout.
        if let Some(mut child) = self.child.take() {
            let pid = child.id();
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
            // Do not block the async receiver while waiting for OS process cleanup.
            if let Err(error) = std::thread::Builder::new()
                .name("video-viewer-reap".into())
                .spawn(move || {
                    let _ = child.wait();
                })
            {
                tracing::warn!(%error, "cannot start viewer reaper");
            }
            tracing::info!(pid, "video viewer closed");
        }
    }
}

impl VideoSink for Viewer {
    fn config(&mut self, codec: Codec, record: &[u8]) {
        if let Some(writer) = &mut self.writer {
            writer.config(codec, record);
        }
    }
    fn frame(&mut self, pts_ns: u64, nals: &[u8]) {
        if let Some(writer) = &mut self.writer {
            writer.frame(pts_ns, nals);
        }
    }
    fn end(&mut self) {
        self.close();
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.close();
    }
}
