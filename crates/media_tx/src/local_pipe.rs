//! Optional timestamped PCM pipe adapter. The application supplies the renderer
//! process/device and an explicit mapping to its CLOCK_MONOTONIC nanoseconds.
use crate::{AudioPacket, LocalSink};
use anyhow::{Context, Result, bail};
use std::{sync::Arc, time::Instant};
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// MLP1: magic, signed little-endian monotonic ns, u32 LE frame count, S16LE stereo.
/// Compatible with the existing local_player; no process, route or device is opened.
pub struct LocalPcmPipe<W> {
    writer: W,
    reference: Instant,
    reference_ns: i64,
    gain: u8,
    audible: bool,
    record: Vec<u8>,
}
impl<W> LocalPcmPipe<W> {
    /// The reference pair must describe the same instant on the renderer's host.
    /// Rust Instant's private epoch must never be cast to a kernel timestamp.
    pub fn new(writer: W, reference: Instant, reference_ns: u64) -> Result<Self> {
        Ok(Self {
            writer,
            reference,
            reference_ns: reference_ns
                .try_into()
                .context("local clock outside signed nanoseconds")?,
            gain: 1,
            audible: true,
            record: Vec::with_capacity(crate::PACKET_BYTES + 16),
        })
    }
    /// Explicit compensation for an already attenuated source, local copy only.
    pub fn with_gain(mut self, gain: u8) -> Result<Self> {
        if !(1..=4).contains(&gain) {
            bail!("local compensation gain must be 1..4");
        }
        self.gain = gain;
        Ok(self)
    }
    /// Startup handoff gate; silences only this output, retaining its timing.
    pub fn set_audible(&mut self, audible: bool) {
        self.audible = audible;
    }
    fn timestamp(&self, at: Instant) -> Result<i64> {
        let delta = if at >= self.reference {
            at.duration_since(self.reference).as_nanos() as i128
        } else {
            -(self.reference.duration_since(at).as_nanos() as i128)
        };
        let ns = i128::from(self.reference_ns) + delta;
        if !(0..=i128::from(i64::MAX)).contains(&ns) {
            bail!("local presentation timestamp out of range");
        }
        Ok(ns as i64)
    }
}
impl<W: AsyncWrite + Unpin + Send + 'static> LocalSink for LocalPcmPipe<W> {
    async fn write(&mut self, packet: Arc<AudioPacket>, play_at: Instant) -> Result<()> {
        let timestamp = self.timestamp(play_at)?;
        self.record.clear();
        self.record.extend_from_slice(b"MLP1");
        self.record.extend_from_slice(&timestamp.to_le_bytes());
        self.record
            .extend_from_slice(&((packet.pcm().len() / 4) as u32).to_le_bytes());
        if !self.audible || !packet.source_is_current() {
            self.record.resize(16 + packet.pcm().len(), 0);
            return self
                .writer
                .write_all(&self.record)
                .await
                .context("local silent PCM pipe write");
        }
        for sample in packet.pcm().chunks_exact(2) {
            let value = (i32::from(i16::from_be_bytes([sample[0], sample[1]]))
                * i32::from(self.gain))
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            self.record.extend_from_slice(&value.to_le_bytes());
        }
        self.writer
            .write_all(&self.record)
            .await
            .context("local PCM pipe write")
    }
    async fn finish(&mut self) -> Result<()> {
        self.writer.flush().await.context("local PCM pipe flush")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    #[tokio::test]
    async fn startup_gate_silences_only_local_payload_and_keeps_timestamps() {
        let (writer, mut reader) = tokio::io::duplex(4096);
        let now = Instant::now();
        let packet = Arc::new(
            crate::MediaClock::at(now, 0)
                .packet(0, &[0x12; crate::PACKET_BYTES])
                .unwrap(),
        );
        let mut sink = LocalPcmPipe::new(writer, now, 1_000_000_000).unwrap();
        sink.set_audible(false);
        sink.write(packet.clone(), now).await.unwrap();
        sink.set_audible(true);
        sink.write(packet.clone(), now).await.unwrap();
        drop(sink);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        let size = 16 + crate::PACKET_BYTES;
        assert_eq!(bytes.len(), size * 2);
        assert_eq!(&bytes[..16], &bytes[size..size + 16]);
        assert!(bytes[16..size].iter().all(|b| *b == 0));
        assert!(bytes[size + 16..].iter().all(|b| *b == 0x12));
        assert!(packet.pcm().iter().all(|b| *b == 0x12));
    }
    #[tokio::test]
    async fn pipe_preserves_deadline_and_remote_pcm_with_explicit_local_gain() {
        let (writer, mut reader) = tokio::io::duplex(4096);
        let now = Instant::now();
        let mut pcm = vec![0; crate::PACKET_BYTES];
        pcm[..4].copy_from_slice(&[0x20, 0x00, 0xe0, 0x00]);
        let packet = Arc::new(crate::MediaClock::at(now, 0).packet(0, &pcm).unwrap());
        let mut sink = LocalPcmPipe::new(writer, now, 1_000_000_000)
            .unwrap()
            .with_gain(4)
            .unwrap();
        sink.write(packet.clone(), now + std::time::Duration::from_millis(500))
            .await
            .unwrap();
        sink.finish().await.unwrap();
        drop(sink);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(&bytes[..4], b"MLP1");
        assert_eq!(
            i64::from_le_bytes(bytes[4..12].try_into().unwrap()),
            1_500_000_000
        );
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 352);
        assert_eq!(&bytes[16..20], &[0xff, 0x7f, 0x00, 0x80]);
        assert_eq!(packet.pcm(), pcm);
    }
    #[test]
    fn clock_mapping_is_checked_in_both_directions() {
        let now = Instant::now();
        let sink = LocalPcmPipe::new(tokio::io::sink(), now, 100).unwrap();
        assert_eq!(
            sink.timestamp(now - std::time::Duration::from_nanos(50))
                .unwrap(),
            50
        );
        assert!(
            sink.timestamp(now - std::time::Duration::from_nanos(101))
                .is_err()
        );
        assert!(LocalPcmPipe::new(tokio::io::sink(), now, u64::MAX).is_err());
        assert!(sink.with_gain(5).is_err());
    }
}
