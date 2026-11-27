//! Pacing and framing of caller-provided raw PCM.
use crate::tx::{AudioPacket, FRAMES_PER_PACKET, MediaClock, PACKET_BYTES};
use anyhow::{Context, Result, anyhow, bail};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::broadcast;

/// Input is S16BE, stereo, 44.1 kHz. A final complete stereo frame is padded
/// with silence to the packet boundary; a truncated sample/frame is an error.
pub async fn produce<R: AsyncRead + Unpin>(
    mut input: R,
    output: broadcast::Sender<Arc<AudioPacket>>,
) -> Result<u64> {
    let clock = MediaClock::now();
    let mut frame_index = 0_u64;
    loop {
        let mut pcm = [0_u8; PACKET_BYTES];
        let mut used = 0;
        while used < pcm.len() {
            let count = input
                .read(&mut pcm[used..])
                .await
                .context("чтение PCM потока")?;
            if count == 0 {
                break;
            }
            used += count;
        }
        if used == 0 {
            return Ok(frame_index);
        }
        if used % 4 != 0 {
            bail!("PCM поток закончился посреди S16BE стереокадра");
        }
        let packet = Arc::new(clock.packet(frame_index, &pcm)?);
        tokio::time::sleep_until(packet.produced_at().into()).await;
        output
            .send(packet)
            .map_err(|_| anyhow!("нет активных получателей PCM"))?;
        frame_index += FRAMES_PER_PACKET as u64;
        if used < PACKET_BYTES {
            return Ok(frame_index);
        }
    }
}
