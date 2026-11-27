//! Paced buffered AAC sender. The caller owns input and receiver selection.
use anyhow::{Context, Result, anyhow, bail};
use media::tx::{AAC_FRAMES_PER_PACKET, BufferedConfig, BufferedSession, adts_frames};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    time::{Instant, sleep_until},
};

pub fn frame_duration() -> Duration {
    Duration::from_secs_f64(AAC_FRAMES_PER_PACKET as f64 / 44_100.0)
}

pub fn frame_deadline(start: Instant, index: u64) -> Instant {
    start + frame_duration() * index as u32
}

struct ControlTask(tokio::task::JoinHandle<()>);
impl Drop for ControlTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Experimental type 103 sender. Its control listener drains incoming data;
/// it does not implement a complete reverse control channel.
pub async fn send(
    bytes: &[u8],
    address: SocketAddr,
    control_port: u16,
    request_timeout: Duration,
) -> Result<u64> {
    let frames = adts_frames(bytes).map_err(|e| anyhow!("ADTS: {e:?}"))?;
    if frames.is_empty() {
        bail!("AAC stream contains no frames");
    }
    let stream = TcpStream::connect(address)
        .await
        .with_context(|| format!("connect {address}"))?;
    let local = stream.local_addr()?.ip();
    let listener = TcpListener::bind((local, control_port)).await?;
    let control_port = listener.local_addr()?.port();
    let _control = ControlTask(tokio::spawn(async move {
        let mut clients = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((mut socket, _)) = accepted else { break; };
                    clients.spawn(async move {
                        let mut sink = [0u8; 256];
                        while matches!(socket.read(&mut sink).await, Ok(n) if n > 0) {}
                    });
                }
                _ = clients.join_next(), if !clients.is_empty() => {}
            }
        }
    }));
    let mut shk = [0u8; 32];
    for byte in &mut shk {
        *byte = rand::random();
    }
    let stream_connection_id = rand::random::<u64>();
    let device_id = format!(
        "02:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>(),
        rand::random::<u8>()
    );
    let uri = format!("rtsp://{local}/{stream_connection_id:016X}");
    let mut config = BufferedConfig::aac_44100(address, control_port, shk, stream_connection_id);
    config.request_timeout = request_timeout;
    let mut session = BufferedSession::pair_transient(stream, uri, &device_id, config).await?;
    if let Some(size) = session.reply().audio_buffer_size {
        tracing::info!(
            bytes = size,
            "receiver audioBufferSize (not used for pacing)"
        );
    }
    let start = Instant::now();
    for (index, frame) in frames.iter().enumerate() {
        sleep_until(frame_deadline(start, index as u64)).await;
        session.send_frame(frame).await?;
    }
    session.finish().await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn malformed_input_is_rejected_before_opening_receiver_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let result = send(
            b"not-adts",
            listener.local_addr().unwrap(),
            0,
            Duration::from_secs(1),
        )
        .await;
        assert!(result.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(30), listener.accept())
                .await
                .is_err()
        );
    }
}
