//! One domain boundary, crossed once.
//!
//! A relayed frame arrives stamped in the upstream session's clock domain and
//! leaves stamped in the outbound group's domain. Both readings must come from
//! ONE producer instant; the deadline then carries the domain it belongs to, so
//! the same hop cannot be applied twice and a frame that is already outbound is
//! refused instead of being shifted again. This module performs no I/O, reads no
//! clock and makes no claim that either domain is synchronized.

use clock::domain::{Domain, Reading};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayError {
    /// The two readings belong to one domain: there is no boundary to cross.
    SameDomain,
    /// The deadline is not in the shift's source domain (a second application,
    /// or a frame from an unrelated session).
    WrongSource,
    /// The shifted deadline does not fit a nanosecond counter.
    OutOfRange,
}

/// Offset between two clock domains sampled at one instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shift {
    from: Domain,
    to: Domain,
    ns: i64,
}

impl Shift {
    /// `inbound` and `outbound` must be read at the same producer instant.
    /// Equal domains are rejected: nothing may be added to a frame that already
    /// lives in the outbound domain, which is exactly the double-shift case.
    pub fn between(inbound: Reading, outbound: Reading) -> Result<Self, RelayError> {
        if inbound.domain() == outbound.domain() {
            return Err(RelayError::SameDomain);
        }
        let ns = i128::from(outbound.ns()) - i128::from(inbound.ns());
        Ok(Self {
            from: inbound.domain(),
            to: outbound.domain(),
            ns: i64::try_from(ns).map_err(|_| RelayError::OutOfRange)?,
        })
    }

    pub fn from(&self) -> Domain {
        self.from
    }

    pub fn to(&self) -> Domain {
        self.to
    }

    pub fn ns(&self) -> i64 {
        self.ns
    }

    /// Translate one deadline. Checked: a wrapped counter is reported, never
    /// published.
    pub fn apply(&self, ns: u64) -> Result<u64, RelayError> {
        let shifted = i128::from(ns) + i128::from(self.ns);
        if !(0..=i128::from(u64::MAX)).contains(&shifted) {
            return Err(RelayError::OutOfRange);
        }
        Ok(shifted as u64)
    }

    /// Cross one boundary. `deadline` must still be in `from`, so re-applying
    /// this shift (or shifting a frame that already crossed it) fails.
    pub fn cross(&self, deadline: RelayedDeadline) -> Result<RelayedDeadline, RelayError> {
        if deadline.domain != self.from {
            return Err(RelayError::WrongSource);
        }
        Ok(RelayedDeadline {
            ns: self.apply(deadline.ns)?,
            domain: self.to,
        })
    }
}

/// A deadline together with the domain it is expressed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayedDeadline {
    ns: u64,
    domain: Domain,
}

impl RelayedDeadline {
    pub fn new(ns: u64, domain: Domain) -> Self {
        Self { ns, domain }
    }

    pub fn ns(&self) -> u64 {
        self.ns
    }

    pub fn domain(&self) -> Domain {
        self.domain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain(id: u64, epoch: u64) -> Domain {
        Domain::new(id, epoch).unwrap()
    }

    #[test]
    fn one_hop_moves_the_deadline_by_the_measured_offset() {
        let (a, b) = (domain(7, 1), domain(8, 1));
        let shift = Shift::between(a.reading(1_000), b.reading(1_500)).unwrap();
        assert_eq!(shift.from(), a);
        assert_eq!(shift.to(), b);
        assert_eq!(shift.ns(), 500);
        let out = shift
            .cross(RelayedDeadline::new(2_000, a))
            .expect("one crossing");
        assert_eq!(out.ns(), 2_500);
        assert_eq!(out.domain(), b);
    }

    #[test]
    fn chained_hops_equal_the_direct_shift_of_the_same_boundaries() {
        let (a, b, c) = (domain(7, 1), domain(8, 1), domain(9, 2));
        let ab = Shift::between(a.reading(1_000), b.reading(1_500)).unwrap();
        let bc = Shift::between(b.reading(1_500), c.reading(1_450)).unwrap();
        let direct = Shift::between(a.reading(1_000), c.reading(1_450)).unwrap();

        let start = RelayedDeadline::new(2_000, a);
        let mid = ab.cross(start).unwrap();
        let end = bc.cross(mid).unwrap();
        assert_eq!(end.ns(), 2_450);
        assert_eq!(end.ns(), direct.apply(start.ns()).unwrap());
        assert_eq!(end.domain(), c);
    }

    #[test]
    fn a_frame_already_in_the_outbound_domain_is_never_shifted_again() {
        let (a, b) = (domain(7, 1), domain(8, 1));
        let ab = Shift::between(a.reading(1_000), b.reading(1_500)).unwrap();
        let once = ab.cross(RelayedDeadline::new(2_000, a)).unwrap();
        assert_eq!(ab.cross(once), Err(RelayError::WrongSource));
        // A frame produced in the outbound domain has no boundary to cross.
        assert_eq!(
            Shift::between(b.reading(1_500), b.reading(1_700)),
            Err(RelayError::SameDomain)
        );
        // Same clock id, different epoch is a real boundary.
        let next = domain(7, 2);
        assert!(Shift::between(a.reading(1_000), next.reading(1_000)).is_ok());
    }

    #[test]
    fn unrelated_session_deadline_is_rejected_instead_of_mis_shifted() {
        let (a, b, other) = (domain(7, 1), domain(8, 1), domain(9, 1));
        let ab = Shift::between(a.reading(0), b.reading(10)).unwrap();
        assert_eq!(
            ab.cross(RelayedDeadline::new(5, other)),
            Err(RelayError::WrongSource)
        );
    }

    #[test]
    fn negative_and_wrapping_offsets_are_reported_not_published() {
        let (a, b) = (domain(7, 1), domain(8, 1));
        let back = Shift::between(a.reading(1_000), b.reading(400)).unwrap();
        assert_eq!(back.ns(), -600);
        assert_eq!(back.apply(100), Err(RelayError::OutOfRange));
        assert_eq!(back.apply(600), Ok(0));
        let ahead = Shift::between(a.reading(0), b.reading(1_000)).unwrap();
        assert_eq!(ahead.apply(u64::MAX - 1), Err(RelayError::OutOfRange));
        assert_eq!(ahead.apply(u64::MAX - 1_000), Ok(u64::MAX));
        // An offset that does not fit the signed nanosecond counter is refused
        // at construction, before any deadline is touched.
        assert_eq!(
            Shift::between(a.reading(0), b.reading(u64::MAX)).err(),
            Some(RelayError::OutOfRange)
        );
    }
}
