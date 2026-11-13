//! Explicit Linux MONOTONIC_RAW / Rust Instant correlation acquisition.
//! No daemon, socket, sleep, servo or role qualification. Two observations are
//! required: a zero-rate mapping is never invented from one sample. Error bounds
//! are caller policy plus bracket/resolution measurements, not hardware accuracy.
use clock::external::Correlation;
use std::{
    io,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug)]
pub struct Bracket {
    at: Instant,
    raw_ns: u64,
    uncertainty_ns: u64,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

impl Bracket {
    /// Pure observation constructor, also useful for synthetic acquisition tests.
    /// Resolution is granularity, not accuracy: capture_uncertainty must bound
    /// additional platform/call uncertainty. A full bracket span is budgeted at
    /// its midpoint (conservative for the supported +/-1000ppm rate envelope).
    pub fn from_observation(
        before: Instant,
        after: Instant,
        raw_ns: u64,
        raw_resolution_ns: u64,
        instant_resolution_ns: u64,
        capture_uncertainty_ns: u64,
        max_bracket: Duration,
    ) -> io::Result<Self> {
        let span = after
            .checked_duration_since(before)
            .ok_or_else(|| invalid("reversed capture bracket"))?;
        if raw_ns == 0
            || raw_resolution_ns == 0
            || instant_resolution_ns == 0
            || max_bracket.is_zero()
            || max_bracket > Duration::from_secs(1)
            || span > max_bracket
        {
            return Err(invalid("invalid or over-budget clock bracket"));
        }
        let width = u64::try_from(span.as_nanos()).map_err(|_| invalid("bracket overflow"))?;
        let uncertainty_ns = width
            .checked_add(raw_resolution_ns)
            .and_then(|v| v.checked_add(instant_resolution_ns))
            .and_then(|v| v.checked_add(capture_uncertainty_ns))
            .ok_or_else(|| invalid("capture uncertainty overflow"))?;
        let at = before
            .checked_add(span / 2)
            .ok_or_else(|| invalid("capture instant overflow"))?;
        Ok(Self {
            at,
            raw_ns,
            uncertainty_ns,
        })
    }
    pub fn at(self) -> Instant {
        self.at
    }
    pub fn uncertainty_ns(self) -> u64 {
        self.uncertainty_ns
    }
}

/// Convert a checked timespec without signed-to-unsigned wrap or truncation.
#[cfg(target_os = "linux")]
fn timespec_ns(value: libc::timespec) -> io::Result<u64> {
    if value.tv_sec < 0 || value.tv_nsec < 0 || value.tv_nsec >= 1_000_000_000 {
        return Err(invalid("invalid clock timespec"));
    }
    let sec = u64::try_from(value.tv_sec).map_err(|_| invalid("clock seconds overflow"))?;
    let ns = u64::try_from(value.tv_nsec).map_err(|_| invalid("clock nanoseconds overflow"))?;
    sec.checked_mul(1_000_000_000)
        .and_then(|v| v.checked_add(ns))
        .ok_or_else(|| invalid("clock timestamp overflow"))
}

#[cfg(target_os = "linux")]
fn resolution(id: libc::clockid_t) -> io::Result<u64> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: writable timespec, checked return; no shared memory or lifetime alias.
    if unsafe { libc::clock_getres(id, &mut value) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let ns = timespec_ns(value)?;
    if ns == 0 {
        return Err(invalid("zero clock resolution"));
    }
    Ok(ns)
}

/// One bounded sampling attempt, no retry loop and no blocking wait. The caller
/// schedules a second capture separately. On Linux Rust Instant uses the local
/// monotonic clock; its resolution is included explicitly, never equated to RAW.
#[cfg(target_os = "linux")]
pub fn capture(max_bracket: Duration, capture_uncertainty_ns: u64) -> io::Result<Bracket> {
    let raw_resolution = resolution(libc::CLOCK_MONOTONIC_RAW)?;
    let instant_resolution = resolution(libc::CLOCK_MONOTONIC)?;
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let before = Instant::now();
    // SAFETY: writable timespec, fixed Linux clock ID; return checked below.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC_RAW, &mut value) };
    let error = if result != 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };
    let after = Instant::now();
    if let Some(error) = error {
        return Err(error);
    }
    Bracket::from_observation(
        before,
        after,
        timespec_ns(value)?,
        raw_resolution,
        instant_resolution,
        capture_uncertainty_ns,
        max_bracket,
    )
}

/// Two-bracket affine rate estimate. wander_ppb is an explicit owner-supplied
/// bound on departure from measured interval rate over the requested validity;
/// it must be based on platform observations/policy, not nominal oscillator specs.
/// Returned uncertainty grows via Correlation::raw_at. No claim of PTP lock.
pub fn estimate(
    first: Bracket,
    second: Bracket,
    wander_ppb: u64,
    valid_for: Duration,
    max_capture_uncertainty: Duration,
) -> io::Result<Correlation> {
    let elapsed = second
        .at
        .checked_duration_since(first.at)
        .ok_or_else(|| invalid("reversed bracket order"))?;
    if elapsed.is_zero()
        || elapsed > Duration::from_secs(10)
        || valid_for.is_zero()
        || valid_for > Duration::from_secs(10)
        || wander_ppb > 1_000_000
        || max_capture_uncertainty.is_zero()
    {
        return Err(invalid("invalid estimation interval or error policy"));
    }
    let cap = max_capture_uncertainty.as_nanos();
    if u128::from(first.uncertainty_ns) > cap || u128::from(second.uncertainty_ns) > cap {
        return Err(invalid("capture uncertainty exceeds policy"));
    }
    let raw_delta = second
        .raw_ns
        .checked_sub(first.raw_ns)
        .ok_or_else(|| invalid("RAW clock reversed"))?;
    if raw_delta == 0 {
        return Err(invalid("RAW clock did not advance"));
    }
    let dt = elapsed.as_nanos();
    let rate = (i128::from(raw_delta) - dt as i128) * 1_000_000_000 / dt as i128;
    if !(-1_000_000..=1_000_000).contains(&rate) {
        return Err(invalid("measured rate outside supported envelope"));
    }
    let errors = u128::from(first.uncertainty_ns) + u128::from(second.uncertainty_ns);
    // Ceil endpoint uncertainty / elapsed; +1 accounts for integer rate rounding.
    let rate_error = (errors * 1_000_000_000).div_ceil(dt) + u128::from(wander_ppb) + 1;
    if rate_error > 1_000_000 {
        return Err(invalid("rate estimate too uncertain"));
    }
    Ok(Correlation {
        at: second.at,
        raw_ns: second.raw_ns,
        rate_ppb: rate as i64,
        rate_error_ppb: rate_error as u64,
        uncertainty_ns: second.uncertainty_ns,
        valid_for,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bracket(at: Instant, raw: u64, width: u64) -> Bracket {
        Bracket::from_observation(
            at,
            at + Duration::from_nanos(width),
            raw,
            1,
            1,
            3,
            Duration::from_micros(10),
        )
        .unwrap()
    }
    #[test]
    fn positive_negative_rate_and_extrapolation_use_two_observations() {
        let at = Instant::now();
        for (step, expected) in [(1_000_100_000, 100_000), (999_900_000, -100_000)] {
            let first = bracket(at, 5_000_000_000, 20);
            let second = bracket(at + Duration::from_secs(1), 5_000_000_000 + step, 20);
            let c = estimate(
                first,
                second,
                100,
                Duration::from_secs(1),
                Duration::from_micros(1),
            )
            .unwrap();
            assert_eq!(c.rate_ppb, expected);
            assert_eq!(c.uncertainty_ns, 25);
            assert_eq!(c.rate_error_ppb, 151);
            let now = c.at + Duration::from_secs(1);
            assert_eq!(c.raw_at(now, now, 1000).unwrap(), 5_000_000_000 + 2 * step);
            assert!(c.raw_at(now, now, 100).is_err());
        }
    }
    #[test]
    fn rejects_reversal_stalls_short_uncertain_and_implausible_rate() {
        let at = Instant::now();
        let first = bracket(at, 5_000_000_000, 20);
        for (dt, raw) in [
            (0, 5_000_000_000),
            (1_000_000_000, 4_000_000_000),
            (1_000_000_000, 5_000_000_000),
            (1, 5_000_000_001),
            (1_000_000_000, 7_000_000_000),
        ] {
            let second = bracket(at + Duration::from_nanos(dt), raw, 20);
            assert!(
                estimate(
                    first,
                    second,
                    100,
                    Duration::from_secs(1),
                    Duration::from_micros(1)
                )
                .is_err()
            );
        }
        assert!(
            estimate(
                first,
                bracket(at + Duration::from_secs(1), 6_000_000_000, 20),
                0,
                Duration::from_secs(1),
                Duration::from_nanos(24)
            )
            .is_err()
        );
    }
    #[test]
    fn capture_bounds_and_policy_are_checked_without_clocks_or_sleep() {
        let at = Instant::now();
        assert!(
            Bracket::from_observation(
                at,
                at + Duration::from_millis(1),
                1,
                1,
                1,
                0,
                Duration::from_micros(10)
            )
            .is_err()
        );
        assert!(Bracket::from_observation(at, at, 1, 0, 1, 0, Duration::from_micros(10)).is_err());
        assert!(
            Bracket::from_observation(at, at, 1, 1, 1, u64::MAX, Duration::from_micros(10))
                .is_err()
        );
        let a = bracket(at, 5_000_000_000, 20);
        let b = bracket(at + Duration::from_secs(1), 6_000_000_000, 20);
        assert!(estimate(b, a, 0, Duration::from_secs(1), Duration::from_micros(1)).is_err());
        assert!(
            estimate(
                a,
                b,
                1_000_001,
                Duration::from_secs(1),
                Duration::from_micros(1)
            )
            .is_err()
        );
        assert!(estimate(a, b, 0, Duration::from_secs(11), Duration::from_micros(1)).is_err());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn timespec_validation_does_not_wrap_negative_or_overflow() {
        assert_eq!(
            timespec_ns(libc::timespec {
                tv_sec: 2,
                tv_nsec: 3
            })
            .unwrap(),
            2_000_000_003
        );
        assert!(
            timespec_ns(libc::timespec {
                tv_sec: -1,
                tv_nsec: 0
            })
            .is_err()
        );
        assert!(
            timespec_ns(libc::timespec {
                tv_sec: 0,
                tv_nsec: 1_000_000_000
            })
            .is_err()
        );
        if std::mem::size_of_val(
            &libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            }
            .tv_sec,
        ) == 8
        {
            assert!(
                timespec_ns(libc::timespec {
                    tv_sec: i64::MAX as _,
                    tv_nsec: 0
                })
                .is_err()
            );
        }
    }
}
