//! Timing of AAC frames in the buffered transport.
use crate::tx::AAC_FRAMES_PER_PACKET;
use std::time::Duration;
use tokio::time::Instant;

pub fn frame_duration() -> Duration {
    Duration::from_secs_f64(AAC_FRAMES_PER_PACKET as f64 / 44_100.0)
}

pub fn frame_deadline(start: Instant, index: u64) -> Instant {
    start + frame_duration() * index as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deadline_tracks_aac_frames() {
        let start = Instant::now();
        assert_eq!(frame_deadline(start, 0), start);
        let third = frame_deadline(start, 3) - start;
        assert!((third.as_secs_f64() - 3.0 * 1024.0 / 44_100.0).abs() < 1e-6);
    }
}
