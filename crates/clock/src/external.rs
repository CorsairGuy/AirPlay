//! Qualified external-clock observations, without SHM/socket access or daemon control.
//! The owner supplies trusted daemon/role evidence and a MEASURED RAW/Instant
//! correlation. This does not attest that evidence or create a media lease.
use crate::{
    domain::{Domain, Reading},
    nqptp::Snapshot,
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Stale,
    Uncertain,
    EpochChanged,
    Revoked,
    Reversed,
    Overflow,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

/// Pinned selection, established independently by the daemon/control owner.
/// The codec alone cannot establish sender-master role or daemon instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub daemon_instance: u128,
    pub domain: Domain,
    pub master_start_raw_ns: u64,
    pub clock_change_offset: Option<u64>,
    pub sender_master_verified: bool,
}
/// Measured affine RAW = raw_ns + elapsed(Instant)*(1+rate_ppb/1e9).
/// rate_error_ppb bounds additional uncertainty growth, not an invented servo.
#[derive(Clone, Copy, Debug)]
pub struct Correlation {
    pub at: Instant,
    pub raw_ns: u64,
    pub rate_ppb: i64,
    pub rate_error_ppb: u64,
    pub uncertainty_ns: u64,
    pub valid_for: Duration,
}
impl Correlation {
    pub fn raw_at(&self, at: Instant, now: Instant, max_uncertainty_ns: u64) -> Result<u64, Error> {
        if self.raw_ns == 0
            || !(-1_000_000..=1_000_000).contains(&self.rate_ppb)
            || self.rate_error_ppb > 1_000_000
            || self.valid_for.is_zero()
            || self.valid_for > Duration::from_secs(10)
        {
            return Err(Error::Invalid);
        }
        let age = now.checked_duration_since(self.at).ok_or(Error::Reversed)?;
        if age > self.valid_for {
            return Err(Error::Stale);
        }
        let delta = if at >= self.at {
            at.duration_since(self.at).as_nanos() as i128
        } else {
            -(self.at.duration_since(at).as_nanos() as i128)
        };
        let span = age.as_nanos().max(delta.unsigned_abs());
        if span > self.valid_for.as_nanos() {
            return Err(Error::Stale);
        }
        let uncertainty = u128::from(self.uncertainty_ns)
            + (span * u128::from(self.rate_error_ppb)).div_ceil(1_000_000_000);
        if uncertainty > u128::from(max_uncertainty_ns) {
            return Err(Error::Uncertain);
        }
        u64::try_from(
            i128::from(self.raw_ns)
                + delta * (1_000_000_000 + i128::from(self.rate_ppb)) / 1_000_000_000,
        )
        .map_err(|_| Error::Overflow)
    }
}
/// One-way qualification owner. Any bad observation revokes it; a caller must
/// establish a NEW selection/lease after loss, never revive this object.
pub struct Qualifier {
    selection: Selection,
    max_age_ns: u64,
    max_uncertainty_ns: u64,
    last_raw: Option<u64>,
    last_published: Option<u64>,
    last_offset: Option<u64>,
    last_correlation: Option<Correlation>,
    revoked: bool,
}
impl Qualifier {
    pub fn new(
        selection: Selection,
        max_age: Duration,
        max_uncertainty: Duration,
    ) -> Result<Self, Error> {
        let max_age_ns = u64::try_from(max_age.as_nanos()).map_err(|_| Error::Invalid)?;
        let max_uncertainty_ns =
            u64::try_from(max_uncertainty.as_nanos()).map_err(|_| Error::Invalid)?;
        if selection.daemon_instance == 0
            || selection.master_start_raw_ns == 0
            || !selection.sender_master_verified
            || max_age_ns == 0
            || max_uncertainty_ns == 0
            || max_age > Duration::from_secs(10)
            || max_uncertainty > Duration::from_millis(10)
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            selection,
            max_age_ns,
            max_uncertainty_ns,
            last_raw: None,
            last_published: None,
            last_offset: None,
            last_correlation: None,
            revoked: false,
        })
    }
    pub fn revoke(&mut self) {
        self.revoked = true;
    }
    pub fn observe(
        &mut self,
        selection: Selection,
        sample: Snapshot,
        correlation: Correlation,
        produced: Instant,
        now: Instant,
    ) -> Result<Reading, Error> {
        if self.revoked {
            return Err(Error::Revoked);
        }
        let result = self.check(selection, sample, correlation, produced, now);
        if result.is_err() {
            self.revoke();
        }
        result
    }
    fn check(
        &mut self,
        selection: Selection,
        sample: Snapshot,
        correlation: Correlation,
        produced: Instant,
        now: Instant,
    ) -> Result<Reading, Error> {
        if selection != self.selection
            || sample.clock_id != selection.domain.id()
            || sample.master_start_time != selection.master_start_raw_ns
            || sample.clock_change_offset != selection.clock_change_offset
        {
            return Err(Error::EpochChanged);
        }
        if !matches!(sample.version, 10 | 11)
            || (sample.version == 10 && sample.clock_change_offset.is_some())
            || (sample.version == 11 && sample.clock_change_offset.is_none())
            || sample.local_time == 0
        {
            return Err(Error::Invalid);
        }
        let raw_now = correlation.raw_at(now, now, self.max_uncertainty_ns)?;
        let raw_produced = correlation.raw_at(produced, now, self.max_uncertainty_ns)?;
        if let Some(previous) = self.last_correlation {
            let old_raw_now = previous.raw_at(now, now, self.max_uncertainty_ns)?;
            if old_raw_now.abs_diff(raw_now) > self.max_uncertainty_ns.saturating_mul(2) {
                return Err(Error::EpochChanged);
            }
        }
        if raw_produced < selection.master_start_raw_ns {
            return Err(Error::Reversed);
        }

        let age = raw_now
            .checked_sub(sample.local_time)
            .ok_or(Error::Reversed)?;
        if age > self.max_age_ns {
            return Err(Error::Stale);
        }
        if sample.master_start_time > sample.local_time
            || self.last_raw.is_some_and(|old| raw_now < old)
            || self
                .last_published
                .is_some_and(|old| sample.local_time < old)
        {
            return Err(Error::Reversed);
        }
        // For an attested sender-master, a discontinuity must start a new
        // selection. Bound offset steps by the caller's correlation budget;
        // this is a conservative policy, not a claim of servo lock.
        if self.last_offset.is_some_and(|old| {
            sample.offset.wrapping_sub(old) as i64 != 0
                && (sample.offset.wrapping_sub(old) as i64).unsigned_abs() > self.max_uncertainty_ns
        }) {
            return Err(Error::EpochChanged);
        }
        self.last_correlation = Some(correlation);
        self.last_offset = Some(sample.offset);
        self.last_raw = Some(raw_now);
        self.last_published = Some(sample.local_time);
        // Preserve the external daemon's modular signed-offset semantics.
        Ok(selection.domain.reading(sample.master_time(raw_produced)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Selection, Snapshot, Correlation) {
        let selection = Selection {
            daemon_instance: 7,
            domain: Domain::new(9, 1).unwrap(),
            master_start_raw_ns: 100,
            clock_change_offset: None,
            sender_master_verified: true,
        };
        let sample = Snapshot {
            version: 10,
            clock_id: 9,
            local_time: 1_000_000,
            offset: (-100i64) as u64,
            clock_change_offset: None,
            master_start_time: 100,
        };
        let correlation = Correlation {
            at: Instant::now(),
            raw_ns: 1_000_100,
            rate_ppb: 1000,
            rate_error_ppb: 10,
            uncertainty_ns: 10,
            valid_for: Duration::from_secs(1),
        };
        (selection, sample, correlation)
    }
    fn qualifier(s: Selection) -> Qualifier {
        Qualifier::new(s, Duration::from_millis(10), Duration::from_micros(1)).unwrap()
    }
    #[test]
    fn measured_rate_and_modular_negative_offset_preserve_instant() {
        let (s, p, c) = fixture();
        let mut q = qualifier(s);
        let at = c.at + Duration::from_millis(1);
        assert_eq!(q.observe(s, p, c, at, at).unwrap().ns(), 2_000_001);
    }
    #[test]
    fn stale_observation_revokes_permanently() {
        let (s, p, c) = fixture();
        let mut q = qualifier(s);
        assert_eq!(
            q.observe(s, p, c, c.at, c.at + Duration::from_millis(11)),
            Err(Error::Stale)
        );
        assert_eq!(q.observe(s, p, c, c.at, c.at), Err(Error::Revoked));
    }
    #[test]
    fn changed_daemon_epoch_role_and_abi_revoke() {
        let (s, p, c) = fixture();
        for changed in [
            Selection {
                daemon_instance: 8,
                ..s
            },
            Selection {
                sender_master_verified: false,
                ..s
            },
            Selection {
                domain: Domain::new(9, 2).unwrap(),
                ..s
            },
        ] {
            assert_eq!(
                qualifier(s).observe(changed, p, c, c.at, c.at),
                Err(Error::EpochChanged)
            );
        }
        let p11 = Snapshot {
            version: 11,
            clock_change_offset: Some(5),
            ..p
        };
        let s11 = Selection {
            clock_change_offset: Some(5),
            ..s
        };
        assert!(qualifier(s11).observe(s11, p11, c, c.at, c.at).is_ok());
        assert_eq!(
            qualifier(s).observe(s, p11, c, c.at, c.at),
            Err(Error::EpochChanged)
        );
    }
    #[test]
    fn uncertainty_growth_and_reversed_samples_fail_closed() {
        let (s, p, c) = fixture();
        let bad = Correlation {
            rate_error_ppb: 1_000_000,
            ..c
        };
        let now = c.at + Duration::from_millis(2);
        assert_eq!(
            qualifier(s).observe(s, p, bad, now, now),
            Err(Error::Uncertain)
        );
        let mut q = qualifier(s);
        q.observe(s, p, c, c.at, c.at).unwrap();
        assert_eq!(
            q.observe(
                s,
                Snapshot {
                    local_time: p.local_time - 1,
                    ..p
                },
                c,
                c.at,
                c.at
            ),
            Err(Error::Reversed)
        );
    }
    #[test]
    fn same_id_offset_step_requires_new_selection() {
        let (s, p, c) = fixture();
        let mut q = qualifier(s);
        q.observe(s, p, c, c.at, c.at).unwrap();
        let changed = Snapshot {
            offset: p.offset.wrapping_add(2000),
            ..p
        };
        assert_eq!(
            q.observe(s, changed, c, c.at, c.at),
            Err(Error::EpochChanged)
        );
        assert_eq!(q.observe(s, p, c, c.at, c.at), Err(Error::Revoked));
    }
    #[test]
    fn same_epoch_correlation_step_is_not_a_new_valid_mapping() {
        let (s, p, c) = fixture();
        let mut q = qualifier(s);
        q.observe(s, p, c, c.at, c.at).unwrap();
        assert_eq!(
            q.observe(
                s,
                p,
                Correlation {
                    raw_ns: c.raw_ns + 100_000,
                    ..c
                },
                c.at,
                c.at
            ),
            Err(Error::EpochChanged)
        );
        assert_eq!(q.observe(s, p, c, c.at, c.at), Err(Error::Revoked));
    }
    #[test]
    fn produced_frame_cannot_precede_selected_master_epoch() {
        let (s, p, c) = fixture();
        let at = c.at - Duration::from_millis(1);
        assert_eq!(
            qualifier(s).observe(s, p, c, at, c.at),
            Err(Error::Reversed)
        );
    }
    #[test]
    fn absent_attestation_and_unbounded_correlation_are_rejected() {
        let (s, p, c) = fixture();
        assert!(
            Qualifier::new(
                Selection {
                    sender_master_verified: false,
                    ..s
                },
                Duration::from_secs(1),
                Duration::from_micros(1)
            )
            .is_err()
        );
        assert_eq!(
            qualifier(s).observe(
                s,
                p,
                Correlation {
                    valid_for: Duration::from_secs(11),
                    ..c
                },
                c.at,
                c.at
            ),
            Err(Error::Invalid)
        );
    }
}
