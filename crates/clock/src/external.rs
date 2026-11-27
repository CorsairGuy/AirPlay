//! Qualified external-clock observations, without SHM/socket access or daemon control.
//! The owner supplies trusted daemon/role evidence and a MEASURED RAW/Instant
//! correlation. This does not attest that evidence or create a media lease.
use crate::domain::{Domain, Reading};
use std::time::{Duration, Instant};

/// A source-neutral observation of a clock domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub clock_id: u64,
    pub local_time: u64,
    pub offset: u64,
    pub clock_change_offset: Option<u64>,
    pub master_start_time: u64,
}
impl Snapshot {
    pub fn master_time(self, local_ns: u64) -> u64 {
        local_ns.wrapping_add(self.offset)
    }
}

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

/// Meaning of the daemon's start marker must be established for its exact patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartMarker {
    StableEpoch,
    RefreshedPublication,
}
/// Pinned selection, established independently by the daemon/control owner.
/// The codec alone cannot establish sender-master role or daemon instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub daemon_instance: u128,
    pub domain: Domain,
    /// Owner-attested lower bound of this selection epoch, not an automatically
    /// copied SHM marker. Patched sender-master refreshes that marker on keepalive.
    pub master_start_raw_ns: u64,
    pub start_marker: StartMarker,
    pub clock_change_offset: Option<u64>,
    pub sender_master_verified: bool,
}
/// What a provenancing provider claims about the supplied clock owner. It is a
/// statement about identity and epoch only — never about freshness, accuracy or
/// audible synchronization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Claim {
    pub daemon_instance: u128,
    pub clock_id: u64,
    pub master_start_raw_ns: u64,
    pub start_marker: StartMarker,
    pub clock_change_offset: Option<u64>,
}

/// One bounded attestation of the patched sender-master role.
///
/// Constructing it is the provider's explicit act; it holds for at most
/// [`Attestation::MAX_VALID_FOR`] after the observation and cannot be refreshed
/// in place — a new observation means a new `Attestation`. A `Selection` is
/// derived only for a published sample that still matches this identity, and
/// `sender_master_verified` is set from the attestation itself, so no caller can
/// assert it by hand.
pub struct Attestation {
    claim: Claim,
    observed: Instant,
    valid_for: Duration,
}

impl Attestation {
    /// Upper bound accepted for one claim; longer-lived evidence is stale by
    /// policy, not by a hardware property.
    pub const MAX_VALID_FOR: Duration = Duration::from_secs(5);

    pub fn new(claim: Claim, observed: Instant, valid_for: Duration) -> Result<Self, Error> {
        if claim.daemon_instance == 0
            || claim.clock_id == 0
            || claim.master_start_raw_ns == 0
            || valid_for.is_zero()
            || valid_for > Self::MAX_VALID_FOR
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            claim,
            observed,
            valid_for,
        })
    }

    pub fn claim(&self) -> Claim {
        self.claim
    }

    /// `false` after the bound or when time went backwards: the owner must
    /// obtain a fresh observation, not extend this one.
    pub fn is_fresh(&self, now: Instant) -> bool {
        now.checked_duration_since(self.observed)
            .is_some_and(|age| age <= self.valid_for)
    }

    /// Derive the pinned selection for this published sample. `epoch` is the
    /// caller's process-unique selection epoch, not something the daemon states.
    pub fn selection(
        &self,
        epoch: u64,
        sample: Snapshot,
        now: Instant,
    ) -> Result<Selection, Error> {
        if !self.is_fresh(now) {
            return Err(Error::Stale);
        }
        if sample.clock_id != self.claim.clock_id
            || sample.clock_change_offset != self.claim.clock_change_offset
        {
            return Err(Error::EpochChanged);
        }
        match self.claim.start_marker {
            StartMarker::StableEpoch
                if sample.master_start_time != self.claim.master_start_raw_ns =>
            {
                return Err(Error::EpochChanged);
            }
            // The patched master rewrites that marker on keepalive: newer is a
            // refreshed publication, older is a different epoch.
            StartMarker::RefreshedPublication
                if sample.master_start_time < self.claim.master_start_raw_ns =>
            {
                return Err(Error::EpochChanged);
            }
            _ => {}
        }
        Ok(Selection {
            daemon_instance: self.claim.daemon_instance,
            domain: Domain::new(self.claim.clock_id, epoch).map_err(|_| Error::Invalid)?,
            master_start_raw_ns: self.claim.master_start_raw_ns,
            start_marker: self.claim.start_marker,
            clock_change_offset: self.claim.clock_change_offset,
            sender_master_verified: true,
        })
    }
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
    last_start_marker: Option<u64>,
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
            last_start_marker: None,
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
            || (selection.start_marker == StartMarker::StableEpoch
                && sample.master_start_time != selection.master_start_raw_ns)
            || sample.clock_change_offset != selection.clock_change_offset
        {
            return Err(Error::EpochChanged);
        }
        if sample.local_time == 0 {
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
        if sample.master_start_time < selection.master_start_raw_ns
            || self
                .last_start_marker
                .is_some_and(|old| sample.master_start_time < old)
            || sample.master_start_time > sample.local_time
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
        self.last_start_marker = Some(sample.master_start_time);
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
            start_marker: StartMarker::StableEpoch,
            clock_change_offset: None,
            sender_master_verified: true,
        };
        let sample = Snapshot {
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
    fn attested_refresh_marker_is_not_a_new_epoch_on_keepalive() {
        let (s, p, c) = fixture();
        let s = Selection {
            start_marker: StartMarker::RefreshedPublication,
            ..s
        };
        let mut q = qualifier(s);
        q.observe(s, p, c, c.at, c.at).unwrap();
        let refreshed = Snapshot {
            master_start_time: p.local_time,
            ..p
        };
        assert!(q.observe(s, refreshed, c, c.at, c.at).is_ok());
        assert_eq!(q.observe(s, p, c, c.at, c.at), Err(Error::Reversed));
        let stable = Selection {
            start_marker: StartMarker::StableEpoch,
            ..s
        };
        assert_eq!(
            qualifier(stable).observe(stable, refreshed, c, c.at, c.at),
            Err(Error::EpochChanged)
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

#[cfg(test)]
mod attestation_tests {
    use super::*;

    fn claim() -> Claim {
        Claim {
            daemon_instance: 7,
            clock_id: 9,
            master_start_raw_ns: 1_000,
            start_marker: StartMarker::StableEpoch,
            clock_change_offset: Some(42),
        }
    }

    fn sample() -> Snapshot {
        Snapshot {
            clock_id: 9,
            local_time: 2_000_000_000,
            offset: 5,
            clock_change_offset: Some(42),
            master_start_time: 1_000,
        }
    }

    fn correlation(at: Instant) -> Correlation {
        Correlation {
            at,
            raw_ns: 2_000_000_000,
            rate_ppb: 0,
            rate_error_ppb: 0,
            uncertainty_ns: 0,
            valid_for: Duration::from_millis(10),
        }
    }

    #[test]
    fn fresh_attestation_pins_its_own_identity_and_epoch() {
        let at = Instant::now();
        let attestation = Attestation::new(claim(), at, Duration::from_secs(1)).unwrap();
        let selection = attestation.selection(3, sample(), at).unwrap();
        assert!(selection.sender_master_verified);
        assert_eq!(selection.daemon_instance, 7);
        assert_eq!(selection.domain.id(), 9);
        assert_eq!(selection.domain.epoch(), 3);
        assert_eq!(selection.master_start_raw_ns, 1_000);
        assert_eq!(selection.clock_change_offset, Some(42));
        let mut q =
            Qualifier::new(selection, Duration::from_secs(1), Duration::from_micros(1)).unwrap();
        assert!(
            q.observe(selection, sample(), correlation(at), at, at)
                .is_ok()
        );
    }

    #[test]
    fn expired_attestation_never_extends_itself() {
        let at = Instant::now();
        let attestation = Attestation::new(claim(), at, Duration::from_millis(500)).unwrap();
        assert!(attestation.is_fresh(at));
        assert!(!attestation.is_fresh(at + Duration::from_millis(501)));
        assert_eq!(
            attestation.selection(1, sample(), at + Duration::from_millis(501)),
            Err(Error::Stale)
        );
        assert!(
            !attestation.is_fresh(at - Duration::from_millis(1)),
            "backwards time is not freshness"
        );
    }

    #[test]
    fn unattestable_claims_and_unbounded_lifetimes_are_rejected() {
        let at = Instant::now();
        for bad in [
            Claim {
                daemon_instance: 0,
                ..claim()
            },
            Claim {
                clock_id: 0,
                ..claim()
            },
            Claim {
                master_start_raw_ns: 0,
                ..claim()
            },
        ] {
            assert!(Attestation::new(bad, at, Duration::from_secs(1)).is_err());
        }
        assert!(Attestation::new(claim(), at, Duration::ZERO).is_err());
        assert!(
            Attestation::new(
                claim(),
                at,
                Attestation::MAX_VALID_FOR + Duration::from_millis(1)
            )
            .is_err()
        );
    }

    #[test]
    fn another_clock_or_change_offset_is_not_the_attested_one() {
        let at = Instant::now();
        let attestation = Attestation::new(claim(), at, Duration::from_secs(1)).unwrap();
        assert_eq!(
            attestation.selection(
                1,
                Snapshot {
                    clock_id: 10,
                    ..sample()
                },
                at
            ),
            Err(Error::EpochChanged)
        );
        assert_eq!(
            attestation.selection(
                1,
                Snapshot {
                    clock_change_offset: None,
                    ..sample()
                },
                at
            ),
            Err(Error::EpochChanged)
        );
    }

    #[test]
    fn stable_epoch_requires_the_exact_marker_and_refreshed_allows_newer() {
        let at = Instant::now();
        let stable = Attestation::new(claim(), at, Duration::from_secs(1)).unwrap();
        assert_eq!(
            stable.selection(
                1,
                Snapshot {
                    master_start_time: 1_001,
                    ..sample()
                },
                at
            ),
            Err(Error::EpochChanged)
        );
        let refreshed = Attestation::new(
            Claim {
                start_marker: StartMarker::RefreshedPublication,
                ..claim()
            },
            at,
            Duration::from_secs(1),
        )
        .unwrap();
        let selection = refreshed
            .selection(
                1,
                Snapshot {
                    master_start_time: 1_500,
                    ..sample()
                },
                at,
            )
            .unwrap();
        assert_eq!(selection.start_marker, StartMarker::RefreshedPublication);
        assert_eq!(
            refreshed.selection(
                1,
                Snapshot {
                    master_start_time: 999,
                    ..sample()
                },
                at
            ),
            Err(Error::EpochChanged)
        );
    }

    #[test]
    fn a_later_different_clock_revokes_the_qualified_timeline() {
        let at = Instant::now();
        let attestation = Attestation::new(claim(), at, Duration::from_secs(2)).unwrap();
        let selection = attestation.selection(1, sample(), at).unwrap();
        let mut q =
            Qualifier::new(selection, Duration::from_secs(1), Duration::from_micros(1)).unwrap();
        q.observe(selection, sample(), correlation(at), at, at)
            .unwrap();
        let other = Snapshot {
            clock_id: 11,
            ..sample()
        };
        assert_eq!(
            q.observe(selection, other, correlation(at), at, at),
            Err(Error::EpochChanged)
        );
        assert_eq!(
            q.observe(selection, sample(), correlation(at), at, at),
            Err(Error::Revoked)
        );
    }
}
