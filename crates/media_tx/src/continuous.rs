//! Source-side continuous scheduling policy ported from the Station sender.
//! No clock service, sockets, marker bytes, global generation or reconnects.
//! The caller retains input on Wait, consumes it on Frame/Discard/Skip, stamps
//! Frame once with the existing MediaClock/PTP producer, then broadcasts it.
use crate::{FRAMES_PER_PACKET, PACKET_BYTES, packet::frames_duration};
use anyhow::{Result, bail};
use std::time::{Duration, Instant};

const GAP_GRACE: Duration = Duration::from_millis(40);
const PAUSE_WINDOW: Duration = Duration::from_secs(600);

pub struct Input<'a> {
    pub generation: u64,
    pub pcm: &'a [u8],
}
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Wait(Instant),
    DiscardStale,
    /// Late input is discarded, not burst-sent or used to restart a session.
    Skip {
        frame_index: u64,
    },
    Frame {
        frame_index: u64,
        generation: u64,
        pcm: Box<[u8; PACKET_BYTES]>,
    },
    PauseExpired,
}
pub struct Timeline {
    epoch: Instant,
    frame_index: u64,
    generation: u64,
    max_late: Duration,
    paused_until: Option<Instant>,
}
impl Timeline {
    /// Use the SAME epoch as MediaClock. Station continuous tolerance is total
    /// lead minus150ms, with a100ms minimum, never a new per-target epoch.
    pub fn new(epoch: Instant, latency: Duration) -> Result<Self> {
        if latency < Duration::from_millis(250) || latency > Duration::from_secs(10) {
            bail!("invalid continuous playout lead");
        }
        Ok(Self {
            epoch,
            frame_index: 0,
            generation: 0,
            max_late: latency
                .saturating_sub(Duration::from_millis(150))
                .max(Duration::from_millis(100)),
            paused_until: None,
        })
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn epoch(&self) -> Instant {
        self.epoch
    }
    pub(super) fn apply_source_state(
        &mut self,
        generation: u64,
        paused: bool,
        now: Instant,
    ) -> Result<()> {
        if generation < self.generation {
            bail!("source generation regressed");
        }
        self.generation = generation;
        if paused {
            self.pause(now)?;
        } else {
            self.resume();
        }
        Ok(())
    }
    pub(super) fn output_deadline(&self) -> Duration {
        self.max_late + Duration::from_millis(250)
    }
    pub fn pause(&mut self, now: Instant) -> Result<()> {
        if self.paused_until.is_none() {
            self.paused_until = Some(
                now.checked_add(PAUSE_WINDOW)
                    .ok_or_else(|| anyhow::anyhow!("pause deadline overflow"))?,
            );
        }
        Ok(())
    }
    pub fn resume(&mut self) {
        self.paused_until = None;
    }
    /// Invalidate queued source data without jumping the shared media timeline.
    /// This alone does NOT flush remote/local queues or reset RTP/crypto state.
    pub fn invalidate_source(&mut self) -> Result<u64> {
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("source generation exhausted"))?;
        Ok(self.generation)
    }
    pub fn poll(&mut self, now: Instant, input: Option<Input<'_>>) -> Result<Step> {
        if self.paused_until.is_some_and(|deadline| now >= deadline) {
            return Ok(Step::PauseExpired);
        }
        if let Some(input) = &input {
            if input.pcm.len() != PACKET_BYTES {
                bail!("continuous input must be one complete PCM packet");
            }
            if input.generation < self.generation {
                return Ok(Step::DiscardStale);
            }
            if input.generation > self.generation {
                bail!("unannounced source generation");
            }
        }
        let due = self
            .epoch
            .checked_add(frames_duration(self.frame_index))
            .ok_or_else(|| anyhow::anyhow!("continuous media time overflow"))?;
        if now < due {
            return Ok(Step::Wait(due));
        }
        let frame_index = self.frame_index;
        let late = now.duration_since(due);
        if late > self.max_late {
            self.advance()?;
            return Ok(Step::Skip { frame_index });
        }
        let paused = self.paused_until.is_some();
        if !paused && input.is_none() && late < GAP_GRACE {
            return Ok(Step::Wait(
                due.checked_add(GAP_GRACE)
                    .ok_or_else(|| anyhow::anyhow!("gap deadline overflow"))?,
            ));
        }
        let mut pcm = Box::new([0; PACKET_BYTES]);
        if !paused {
            if let Some(input) = input {
                pcm.copy_from_slice(input.pcm);
            }
        }
        self.advance()?;
        Ok(Step::Frame {
            frame_index,
            generation: self.generation,
            pcm,
        })
    }
    fn advance(&mut self) -> Result<()> {
        self.frame_index = self
            .frame_index
            .checked_add(FRAMES_PER_PACKET as u64)
            .ok_or_else(|| anyhow::anyhow!("continuous frame index exhausted"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(step: Step) -> (u64, u64, Box<[u8; PACKET_BYTES]>) {
        match step {
            Step::Frame {
                frame_index,
                generation,
                pcm,
            } => (frame_index, generation, pcm),
            other => panic!("expected frame: {other:?}"),
        }
    }
    #[test]
    fn gap_fills_silence_after_grace_on_the_original_timeline() {
        let epoch = Instant::now();
        let mut timeline = Timeline::new(epoch, Duration::from_secs(2)).unwrap();
        assert_eq!(
            timeline.poll(epoch, None).unwrap(),
            Step::Wait(epoch + GAP_GRACE)
        );
        let (index, _, pcm) = frame(timeline.poll(epoch + GAP_GRACE, None).unwrap());
        assert_eq!(index, 0);
        assert!(pcm.iter().all(|byte| *byte == 0));
        let data = [9; PACKET_BYTES];
        let (index, _, pcm) = frame(
            timeline
                .poll(
                    epoch + GAP_GRACE,
                    Some(Input {
                        generation: 0,
                        pcm: &data,
                    }),
                )
                .unwrap(),
        );
        assert_eq!(index, 352);
        assert_eq!(*pcm, data);
    }
    #[test]
    fn pause_resume_and_source_changes_never_reset_media_time() {
        let epoch = Instant::now();
        let mut timeline = Timeline::new(epoch, Duration::from_secs(2)).unwrap();
        timeline.pause(epoch).unwrap();
        let data = [8; PACKET_BYTES];
        let (_, _, silent) = frame(
            timeline
                .poll(
                    epoch,
                    Some(Input {
                        generation: 0,
                        pcm: &data,
                    }),
                )
                .unwrap(),
        );
        assert!(silent.iter().all(|byte| *byte == 0));
        timeline.resume();
        assert_eq!(timeline.invalidate_source().unwrap(), 1);
        let due = epoch + frames_duration(352);
        assert_eq!(
            timeline
                .poll(
                    due,
                    Some(Input {
                        generation: 0,
                        pcm: &data
                    })
                )
                .unwrap(),
            Step::DiscardStale
        );
        assert!(
            timeline
                .poll(
                    due,
                    Some(Input {
                        generation: 2,
                        pcm: &data
                    })
                )
                .is_err()
        );
        let (index, generation, pcm) = frame(
            timeline
                .poll(
                    due,
                    Some(Input {
                        generation: 1,
                        pcm: &data,
                    }),
                )
                .unwrap(),
        );
        assert_eq!((index, generation), (352, 1));
        assert_eq!(*pcm, data);
    }
    #[test]
    fn lateness_skips_without_flooding_or_restarting() {
        let epoch = Instant::now();
        let mut timeline = Timeline::new(epoch, Duration::from_millis(250)).unwrap();
        assert_eq!(
            timeline
                .poll(epoch + Duration::from_millis(101), None)
                .unwrap(),
            Step::Skip { frame_index: 0 }
        );
        let (index, _, _) = frame(
            timeline
                .poll(epoch + Duration::from_millis(101), None)
                .unwrap(),
        );
        assert_eq!(index, 352);
    }
    #[test]
    fn duplicate_pause_does_not_extend_ten_minute_window() {
        let epoch = Instant::now();
        let mut timeline = Timeline::new(epoch, Duration::from_secs(2)).unwrap();
        timeline.pause(epoch).unwrap();
        timeline.pause(epoch + Duration::from_secs(599)).unwrap();
        assert_eq!(
            timeline
                .poll(epoch + Duration::from_secs(600), None)
                .unwrap(),
            Step::PauseExpired
        );
        timeline.resume();
        assert_ne!(
            timeline
                .poll(epoch + Duration::from_secs(600), None)
                .unwrap(),
            Step::PauseExpired
        );
    }
    #[test]
    fn invalid_input_does_not_advance_or_create_an_epoch() {
        let epoch = Instant::now();
        assert!(Timeline::new(epoch, Duration::from_millis(100)).is_err());
        let mut timeline = Timeline::new(epoch, Duration::from_secs(2)).unwrap();
        assert!(
            timeline
                .poll(
                    epoch,
                    Some(Input {
                        generation: 0,
                        pcm: &[0]
                    })
                )
                .is_err()
        );
        timeline.pause(epoch).unwrap();
        assert_eq!(frame(timeline.poll(epoch, None).unwrap()).0, 0);
    }
}
