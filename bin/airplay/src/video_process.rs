//! One viewer process per video stream; no global pipe surviving disconnect.
use mirror::{AnnexBWriter, Codec, VideoSink, VideoSinkFactory};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};

pub fn factory(
    program: PathBuf,
    args: Vec<String>,
    framed: bool,
) -> Result<VideoSinkFactory, String> {
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
                    writer: Some(if framed {
                        Input::Framed(input)
                    } else {
                        Input::AnnexB(AnnexBWriter::new(input))
                    }),
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

enum Input {
    AnnexB(AnnexBWriter<ChildStdin>),
    Framed(ChildStdin),
}
impl Input {
    fn packet(&mut self, kind: u8, codec: u8, pts: u64, data: &[u8]) -> std::io::Result<()> {
        if let Self::Framed(input) = self {
            if data.len() > 32 * 1024 * 1024 {
                return Err(std::io::Error::other("video frame too large"));
            }
            let mut header = [0u8; 20];
            header[..4].copy_from_slice(b"MRV1");
            header[4] = kind;
            header[5] = codec;
            header[8..12].copy_from_slice(&(data.len() as u32).to_le_bytes());
            header[12..20].copy_from_slice(&pts.to_le_bytes());
            input.write_all(&header)?;
            input.write_all(data)?;
        }
        Ok(())
    }
}

struct Viewer {
    writer: Option<Input>,
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
        let Some(writer) = &mut self.writer else {
            return;
        };
        if let Input::AnnexB(out) = writer {
            out.config(codec, record);
            return;
        }
        if let Some(data) = mirror::parameter_sets(codec, record) {
            let tag = match codec {
                Codec::H264 => 1,
                Codec::H265 => 2,
            };
            if let Err(error) = writer.packet(1, tag, 0, &data) {
                tracing::error!(%error,"viewer config write failed");
                self.close();
            }
        }
    }
    fn frame(&mut self, pts_ns: u64, nals: &[u8]) {
        let Some(writer) = &mut self.writer else {
            return;
        };
        if let Input::AnnexB(out) = writer {
            out.frame(pts_ns, nals);
            return;
        }
        let mut data = nals.to_vec();
        mirror::avcc_to_annex_b(&mut data);
        if let Err(error) = writer.packet(2, 0, pts_ns, &data) {
            tracing::error!(%error,"viewer frame write failed");
            self.close();
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
