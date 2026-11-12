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
            ptp: None,
            source: None,
        })
    }
    /// Sample/map the shared producer instant once, before broadcasting the
    /// immutable frame. The provider must be nonblocking and role/freshness
    /// qualified; receivers must not replace this with a send-time clock read.
    pub fn packet_with_ptp(
        &self,
        frame_index: u64,
        pcm: &[u8],
        domain: clock::domain::Domain,
        read_at: impl FnOnce(Instant) -> Result<clock::domain::Reading>,
    ) -> Result<AudioPacket> {
        let mut packet = self.packet(frame_index, pcm)?;
        let reading = read_at(packet.produced)?;
        if reading.domain() != domain {
            bail!("producer PTP clock or epoch changed");
        }
        packet.ptp = Some(reading);
        Ok(packet)
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
    ptp: Option<clock::domain::Reading>,
    source: Option<tokio::sync::watch::Receiver<crate::continuous_source::State>>,
}

impl AudioPacket {
    pub(crate) fn guard_source(
        mut self,
        source: tokio::sync::watch::Receiver<crate::continuous_source::State>,
    ) -> Self {
        self.source = Some(source);
        self
    }
    /// A control revision invalidates queued frames even after pause/resume
    /// returns to the same state. Unmanaged legacy packets remain current.
    pub fn source_is_current(&self) -> bool {
        self.source
            .as_ref()
            .is_none_or(|source| matches!(source.has_changed(), Ok(false)))
    }
    pub(crate) fn same_source_revision(&self, other: &Self) -> bool {
        match (&self.source, &other.source) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.same_channel(b) && self.source_is_current() && other.source_is_current()
            }
            _ => false,
        }
    }
    pub(crate) fn silence(&self) -> Self {
        Self {
            data: vec![0; self.data.len()],
            index: self.index,
            produced: self.produced,
            ntp: self.ntp,
            ptp: self.ptp,
            source: self.source.clone(),
        }
    }
    pub fn ptp_reading(&self) -> Option<clock::domain::Reading> {
        self.ptp
    }
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
    fn source_revision_cannot_resurrect_after_pause_resume_or_owner_close() {
        use crate::continuous_source::State;
        let (control, mut source) = tokio::sync::watch::channel(State::default());
        source.borrow_and_update();
        let packet = MediaClock::now()
            .packet(0, &[8; PACKET_BYTES])
            .unwrap()
            .guard_source(source);
        assert!(packet.source_is_current());
        let mut current = control.subscribe();
        current.borrow_and_update();
        let peer = MediaClock::now()
            .packet(352, &[8; PACKET_BYTES])
            .unwrap()
            .guard_source(current);
        assert!(packet.same_source_revision(&peer));
        assert!(
            !packet.same_source_revision(&MediaClock::now().packet(0, &[8; PACKET_BYTES]).unwrap())
        );
        control.send_replace(State {
            generation: 0,
            paused: true,
        });
        control.send_replace(State::default());
        assert!(!packet.source_is_current());
        let mut current = control.subscribe();
        current.borrow_and_update();
        let new = MediaClock::now()
            .packet(704, &[8; PACKET_BYTES])
            .unwrap()
            .guard_source(current);
        assert!(!packet.same_source_revision(&new));
        assert_eq!(packet.pcm(), &[8; PACKET_BYTES]);
        let silence = packet.silence();
        assert_eq!(silence.frame_index(), packet.frame_index());
        assert_eq!(silence.produced_at(), packet.produced_at());
        assert!(silence.pcm().iter().all(|byte| *byte == 0));
        drop(control);
        assert!(!packet.source_is_current());
        assert!(
            MediaClock::now()
                .packet(0, &[8; PACKET_BYTES])
                .unwrap()
                .source_is_current()
        );
    }

    #[test]
    fn ptp_provider_is_called_once_at_the_producer_instant() {
        let start = Instant::now();
        let clock = MediaClock::at(start, 0);
        let domain = clock::domain::Domain::new(7, 1).unwrap();
        let calls = std::cell::Cell::new(0);
        let packet = clock
            .packet_with_ptp(44_100, &[0; PACKET_BYTES], domain, |instant| {
                calls.set(calls.get() + 1);
                assert_eq!(instant, start + Duration::from_secs(1));
                Ok(domain.reading(7_000_000_000))
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(packet.ptp_reading(), Some(domain.reading(7_000_000_000)));
        assert!(
            clock
                .packet(0, &[0; PACKET_BYTES])
                .unwrap()
                .ptp_reading()
                .is_none()
        );
    }
    #[test]
    fn ptp_frame_rejects_stale_domain_and_provider_failure() {
        let clock = MediaClock::now();
        let domain = clock::domain::Domain::new(7, 1).unwrap();
        assert!(
            clock
                .packet_with_ptp(0, &[0; PACKET_BYTES], domain, |_| Ok(
                    clock::domain::Domain::new(7, 2).unwrap().reading(1)
                ))
                .is_err()
        );
        assert!(
            clock
                .packet_with_ptp(0, &[0; PACKET_BYTES], domain, |_| anyhow::bail!(
                    "clock unqualified"
                ))
                .is_err()
        );
    }
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
