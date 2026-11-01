use std::time::{Duration, Instant};

use anyhow::{Result, bail};

pub const SAMPLE_RATE: u32 = 44_100;
pub const FRAMES_PER_PACKET: usize = 352;
pub const FRAME_BYTES: usize = 4;
pub const PACKET_BYTES: usize = FRAMES_PER_PACKET * FRAME_BYTES;

/// Shared fixed media epoch for all receiver connections and optional local
/// output. The NTP origin comes from the SAME monotonic-backed clock used by
/// the NTP responder, not from a new wall-clock read for every audio packet.
#[derive(Clone, Copy, Debug)]
pub struct MediaClock {
    monotonic: Instant,
    ntp: u64,
}

impl MediaClock {
    pub fn now() -> Self {
        Self::at(Instant::now(), timesync::ntp::now())
    }

    pub fn at(monotonic: Instant, ntp: u64) -> Self {
        Self { monotonic, ntp }
    }

    /// Interleaved S16BE stereo at44.1kHz, exactly352 frames. Frame indexes
    /// belong to the shared epoch, including frames skipped during reconnect.
    pub fn packet(&self, frame_index: u64, pcm: &[u8]) -> Result<AudioPacket> {
        if pcm.len() != PACKET_BYTES {
            bail!("realtime PCM packet must contain exactly {PACKET_BYTES} bytes");
        }
        let elapsed = frames_duration(frame_index);
        let produced = self
            .monotonic
            .checked_add(elapsed)
            .ok_or_else(|| anyhow::anyhow!("media timestamp exceeds monotonic clock range"))?;
        let offset = (((frame_index as u128) << 32) / SAMPLE_RATE as u128) as u64;
        Ok(AudioPacket {
            data: pcm.to_vec(),
            index: frame_index,
            produced,
            ntp: self.ntp.wrapping_add(offset),
        })
    }
}

/// Immutable already-framed input; source acquisition/decoding is the caller's
/// responsibility. No URLs, media keys or platform/process handles live here.
#[derive(Debug)]
pub struct AudioPacket {
    pub(crate) data: Vec<u8>,
    pub(crate) index: u64,
    pub(crate) produced: Instant,
    pub(crate) ntp: u64,
}

impl AudioPacket {
    pub fn pcm(&self) -> &[u8] {
        &self.data
    }
    pub fn frame_index(&self) -> u64 {
        self.index
    }
    pub fn produced_at(&self) -> Instant {
        self.produced
    }
    pub fn ntp_timestamp(&self) -> u64 {
        self.ntp
    }
    pub fn play_at(&self, latency: Duration) -> Option<Instant> {
        self.produced.checked_add(latency)
    }
}

pub(crate) fn frames_duration(frames: u64) -> Duration {
    Duration::new(
        frames / SAMPLE_RATE as u64,
        ((frames % SAMPLE_RATE as u64) * 1_000_000_000 / SAMPLE_RATE as u64) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_epoch_preserves_time_across_receiver_start_times() {
        let start = Instant::now();
        let clock = MediaClock::at(start, 100u64 << 32);
        let packet = clock.packet(44_100, &[0; PACKET_BYTES]).unwrap();
        assert_eq!(packet.produced_at(), start + Duration::from_secs(1));
        assert_eq!(packet.ntp_timestamp(), 101u64 << 32);
        assert_eq!(
            packet.play_at(Duration::from_millis(500)),
            Some(start + Duration::from_millis(1500))
        );
        assert_eq!(packet.frame_index(), 44_100);
        assert_eq!(packet.pcm().len(), PACKET_BYTES);
    }

    #[test]
    fn input_must_be_one_complete_pcm_packet() {
        assert!(MediaClock::now().packet(0, &[0; 4]).is_err());
        assert!(MediaClock::now().packet(0, &[0; PACKET_BYTES + 4]).is_err());
    }

    #[test]
    fn subsecond_frame_offsets_do_not_round_to_milliseconds() {
        let start = Instant::now();
        let clock = MediaClock::at(start, 0);
        let packet = clock.packet(352, &[0; PACKET_BYTES]).unwrap();
        assert_eq!(
            packet.produced_at().duration_since(start).as_nanos(),
            7_981_859
        );
        assert_eq!(packet.ntp_timestamp(), ((352u128 << 32) / 44_100) as u64);
    }
}
