//! Local process bridge. Bounded text requests over a private Unix socket.
use crate::send_control::RelayControl;
use anyhow::{Context, Result, bail};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::watch,
    time::timeout,
};

pub struct Server {
    listener: UnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
    control: RelayControl,
}
impl Server {
    pub fn bind(path: &Path, control: RelayControl) -> Result<Self> {
        if !path.is_absolute() {
            bail!("control socket must have an absolute path")
        }
        let parent = std::fs::metadata(path.parent().context("socket parent")?)?;
        // SAFETY: geteuid takes no pointers and has no side effects.
        let uid = unsafe { libc::geteuid() };
        if !parent.is_dir() || parent.uid() != uid || parent.mode() & 0o077 != 0 {
            bail!("control socket parent must be private and owned by this user")
        }
        // Never remove an existing file/socket, including a stale instance.
        let listener = UnixListener::bind(path)?;
        let metadata = std::fs::symlink_metadata(path)?;
        let server = Self {
            listener,
            path: path.into(),
            device: metadata.dev(),
            inode: metadata.ino(),
            control,
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(server)
    }
    pub async fn run(self, mut stop: watch::Receiver<bool>) -> Result<()> {
        loop {
            if *stop.borrow() {
                return Ok(());
            }
            tokio::select! {
                accepted=self.listener.accept()=>{
                    let(stream,_)=accepted?;
                    // Local permissions plus peer identity; there is no TCP listener.
                    if stream.peer_cred()?.uid()!=unsafe{libc::geteuid()}{continue}
                    let _=timeout(Duration::from_secs(1),self.client(stream)).await;
                }
                changed=stop.changed()=>{if changed.is_err()||*stop.borrow(){return Ok(())}},
            }
        }
    }
    async fn client(&self, mut stream: UnixStream) -> Result<()> {
        let mut request = Vec::new();
        let mut byte = [0];
        while request.len() < 256 {
            if stream.read(&mut byte).await? == 0 {
                return Ok(());
            }
            if byte[0] == b'\n' {
                break;
            }
            request.push(byte[0]);
        }
        let reply = if request.len() == 256 {
            // Consume a bounded tail through LF before replying. Closing with
            // unread request data can reset the socket and hide the ERR reply.
            // Very large/incomplete requests are simply closed by this cap/deadline.
            let mut ended = false;
            for _ in 0..256 {
                if stream.read(&mut byte).await? == 0 {
                    return Ok(());
                }
                if byte[0] == b'\n' {
                    ended = true;
                    break;
                }
            }
            if !ended {
                return Ok(());
            }
            "ERR request-too-long\n".to_owned()
        } else {
            match std::str::from_utf8(&request) {
                Ok(command) => self.reply(command),
                Err(_) => "ERR invalid-command\n".into(),
            }
        };
        stream.write_all(reply.as_bytes()).await?;
        Ok(())
    }
    fn reply(&self, command: &str) -> String {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            ["STATUS"] => {
                let (started, targets) = self.control.status_snapshot();
                let mut reply = format!(
                    "OK started={} targets={}\n",
                    u8::from(started),
                    targets.len()
                );
                for target in targets {
                    let (packets, frame) = target
                        .media
                        .map(|m| (m.packets_sent, m.frame_index))
                        .unwrap_or((0, 0));
                    reply.push_str(&format!(
                        "TARGET {} requested_db={:.6} packets={} frame={}\n",
                        target.endpoint,
                        target.requested_volume.as_db(),
                        packets,
                        frame
                    ));
                }
                reply.push_str("END\n");
                reply
            }
            ["VOLUME", endpoint, db] => {
                let parsed = (|| {
                    Ok::<_, anyhow::Error>((endpoint.parse()?, sender::Volume::db(db.parse()?)?))
                })();
                match parsed
                    .and_then(|(endpoint, volume)| self.control.set_volume(endpoint, volume))
                {
                    Ok(()) => "OK queued\n".into(),
                    Err(_) => "ERR invalid-or-inactive-target\n".into(),
                }
            }
            _ => "ERR invalid-command\n".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sender::{MediaProgress, Volume};
    async fn request(path: &Path, command: &str) -> String {
        let mut stream = UnixStream::connect(path).await.unwrap();
        stream.write_all(command.as_bytes()).await.unwrap();
        let mut reply = String::new();
        timeout(Duration::from_secs(2), stream.read_to_string(&mut reply))
            .await
            .unwrap()
            .unwrap();
        reply
    }
    #[tokio::test]
    async fn private_socket_reports_media_and_changes_only_selected_volume() {
        let dir = Path::new("/tmp").join(format!("relay-ipc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("control.sock");
        let control = RelayControl::default();
        let _lease = control.claim(2).unwrap();
        let a = "127.0.0.1:7000".parse().unwrap();
        let b = "127.0.0.2:7000".parse().unwrap();
        let (va, ra) = watch::channel(Volume::db(-30.0).unwrap());
        let (vb, rb) = watch::channel(Volume::db(-30.0).unwrap());
        let (ma, ma_rx) = watch::channel(None);
        let (mb, mb_rx) = watch::channel(None);
        control.register(a, va, ma_rx);
        control.register(b, vb, mb_rx);
        let server = Server::bind(&path, control.clone()).unwrap();
        assert!(Server::bind(&path, control).is_err());
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        let (stop, stopped) = watch::channel(false);
        let running = tokio::spawn(server.run(stopped));
        stop.send_replace(false);
        assert!(
            request(&path, "STATUS\n")
                .await
                .starts_with("OK started=0 targets=2\n")
        );
        assert_eq!(
            request(&path, "VOLUME 127.0.0.1:7000 -20\n").await,
            "OK queued\n"
        );
        assert_eq!(ra.borrow().as_db(), -20.0);
        assert_eq!(rb.borrow().as_db(), -30.0);
        assert!(
            request(&path, "VOLUME 127.0.0.1:7000 NaN\n")
                .await
                .starts_with("ERR")
        );
        assert!(
            request(&path, &format!("{}\n", "X".repeat(256)))
                .await
                .starts_with("ERR request-too-long")
        );
        let event = MediaProgress {
            packets_sent: 1,
            frame_index: 0,
            play_at: std::time::Instant::now(),
        };
        ma.send_replace(Some(event));
        mb.send_replace(Some(event));
        assert!(
            request(&path, "STATUS\n")
                .await
                .starts_with("OK started=1 targets=2\n")
        );
        stop.send_replace(true);
        running.await.unwrap().unwrap();
        assert!(!path.exists());
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn cleanup_does_not_delete_a_replacement_file() {
        let dir = Path::new("/tmp").join(format!("relay-ipc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("control.sock");
        let moved = dir.join("old.sock");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let server = Server::bind(&path, RelayControl::default()).unwrap();
        std::fs::rename(&path, &moved).unwrap();
        std::fs::write(&path, b"keep").unwrap();
        drop(server);
        assert_eq!(std::fs::read(&path).unwrap(), b"keep");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(moved).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.dev() == self.device && m.ino() == self.inode)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

struct Task(tokio::task::JoinHandle<Result<()>>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
pub(crate) async fn run<R: tokio::io::AsyncRead + Unpin>(
    options: crate::send::Options,
    input: R,
    stop: watch::Receiver<bool>,
    path: PathBuf,
) -> Result<()> {
    let control = RelayControl::default();
    let server = Server::bind(&path, control.clone())?;
    let mut serving = Task(tokio::spawn(server.run(stop.clone())));
    tokio::select! {
        result=crate::send::run_controlled(options,input,stop,control)=>result,
        result=&mut serving.0=>{result.context("relay IPC task failed")??;bail!("relay IPC stopped before sender")}
    }
}
