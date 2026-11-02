//! Hardware frame-counter rate measurement, independent of any audio backend.
//!
//! Inspired by Telik's AudioMasterClock (Apache-2.0, AirBridge provenance).
//! This is an independent implementation: fixed memory, disjoint one-second
//! measurement windows and a median of five windows. No pacing or resampling
//! is changed here. Feed presented frames, never queued/submitted frames.

/// Estimates the physical sample rate from a cumulative frame counter and
/// timestamps in one monotonic nanosecond domain. Recreate on device changes.
/// Counter wrap must be extended by the backend before calling `observe`.
#[derive(Debug, Clone)]
pub struct AudioRateEstimator {
    nominal: u32,
    start: Option<(u64, u64)>,
    last: Option<(u64, u64)>,
    rates: [f64; 5],
    count: usize,
    next: usize,
    measured_at: Option<u64>,
}

impl AudioRateEstimator {
    pub fn new(nominal: u32) -> Self {
        assert!(nominal > 0, "sample rate must be positive");
        Self {
            nominal,
            start: None,
            last: None,
            rates: [0.0; 5],
            count: 0,
            next: 0,
            measured_at: None,
        }
    }

    /// Discard measurements after a seek, device restart or clock discontinuity.
    pub fn reset(&mut self) {
        self.start = None;
        self.last = None;
        self.count = 0;
        self.next = 0;
        self.measured_at = None;
    }

    pub fn observe(&mut self, frames: u64, ns: u64) {
        if let Some((previous_frames, previous_ns)) = self.last {
            if ns <= previous_ns || frames < previous_frames || ns - previous_ns > 2_000_000_000 {
                self.reset();
            } else if frames == previous_frames {
                // A paused/stalled DAC is not a slower running DAC. Keep the
                // last advancing sample so frequent stalled reports expire it.
                if ns - previous_ns > 500_000_000 {
                    self.reset();
                }
                return;
            }
        }
        self.last = Some((frames, ns));
        let Some((first_frames, first_ns)) = self.start else {
            self.start = Some((frames, ns));
            return;
        };
        let elapsed = ns - first_ns;
        if elapsed < 1_000_000_000 {
            return;
        }
        let rate = (frames - first_frames) as f64 * 1e9 / elapsed as f64;
        self.start = Some((frames, ns));
        // Broad sanity bound, not an oscillator correction limit. Invalid
        // windows never become evidence for changing a playback clock.
        if (rate / self.nominal as f64 - 1.0).abs() > 0.015 {
            return;
        }
        self.rates[self.next] = rate;
        self.next = (self.next + 1) % self.rates.len();
        self.count = (self.count + 1).min(self.rates.len());
        self.measured_at = Some(ns);
    }

    /// Requires three valid windows. Stale/future-domain queries return None;
    /// callers should then use the nominal rate, not retain an old correction.
    pub fn measured_rate(&self, now_ns: u64) -> Option<f64> {
        let at = self.measured_at?;
        if self.count < 3 || now_ns < at || now_ns - at > 2_000_000_000 {
            return None;
        }
        let mut sorted = self.rates;
        sorted[..self.count].sort_by(f64::total_cmp);
        Some(sorted[self.count / 2])
    }

    pub fn rate_or_nominal(&self, now_ns: u64) -> f64 {
        self.measured_rate(now_ns).unwrap_or(self.nominal as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_positive_and_negative_drift_with_large_epochs() {
        for actual in [47_976_u64, 48_024] {
            let mut clock = AudioRateEstimator::new(48_000);
            for second in 0..=6 {
                clock.observe(
                    u64::MAX - 1_000_000 + actual * second,
                    9_000_000_000_000 + second * 1_000_000_000,
                );
            }
            assert_eq!(clock.measured_rate(9_006_000_000_000), Some(actual as f64));
            assert_eq!(clock.rate_or_nominal(9_009_000_000_001), 48_000.0);
        }
    }

    #[test]
    fn median_rejects_single_plausible_bad_window() {
        let mut clock = AudioRateEstimator::new(48_000);
        let mut frames = 0;
        clock.observe(frames, 0);
        for second in 1..=5 {
            frames += if second == 3 { 48_480 } else { 48_024 };
            clock.observe(frames, second * 1_000_000_000);
        }
        assert_eq!(clock.measured_rate(5_000_000_000), Some(48_024.0));
        clock.observe(frames + 96_000, 6_000_000_000);
        assert_eq!(clock.measured_rate(6_000_000_000), Some(48_024.0));
    }

    #[test]
    fn stall_reset_backward_time_and_missing_samples_require_new_warmup() {
        for fault in 0..4 {
            let mut clock = AudioRateEstimator::new(48_000);
            for second in 0..=3 {
                clock.observe(second * 48_000, second * 1_000_000_000);
            }
            assert!(clock.measured_rate(3_000_000_000).is_some());
            match fault {
                0 => clock.observe(144_000, 3_600_000_000),
                1 => clock.observe(0, 4_000_000_000),
                2 => clock.observe(192_000, 2_000_000_000),
                _ => clock.observe(288_000, 6_000_000_000),
            }
            assert_eq!(clock.rate_or_nominal(6_000_000_000), 48_000.0);
            assert!(clock.measured_rate(0).is_none());
        }
    }
}
