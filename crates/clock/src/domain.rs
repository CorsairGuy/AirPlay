//! Explicit clock domains and affine conversion through ONE local clock.
//! No clocks are started here. The adapter owns sampling, epoch changes on
//! discontinuities and freshness limits; no measured drift is invented.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    WrongDomain,
    Stale,
    TimeReversed,
    Overflow,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

/// Epoch changes when a selected clock restarts or steps, even if its ID stays
/// the same. It is local control state, never sent as part of the wire ClockID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Domain {
    id: u64,
    epoch: u64,
}
impl Domain {
    pub fn new(id: u64, epoch: u64) -> Result<Self, Error> {
        if id == 0 || epoch == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self { id, epoch })
    }
    pub fn id(self) -> u64 {
        self.id
    }
    pub fn epoch(self) -> u64 {
        self.epoch
    }
    pub fn reading(self, ns: u64) -> Reading {
        Reading { domain: self, ns }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    domain: Domain,
    ns: u64,
}
impl Reading {
    pub fn domain(self) -> Domain {
        self.domain
    }
    pub fn ns(self) -> u64 {
        self.ns
    }
}

/// Identifies a specific local monotonic epoch. For example, process-relative
/// Instant and Linux CLOCK_MONOTONIC must have different IDs, not both zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalClock(u128);
impl LocalClock {
    pub fn new(id: u128) -> Result<Self, Error> {
        if id == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self(id))
    }
    pub fn at(self, ns: u64) -> LocalInstant {
        LocalInstant { clock: self, ns }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalInstant {
    clock: LocalClock,
    ns: u64,
}
impl LocalInstant {
    pub fn ns(self) -> u64 {
        self.ns
    }
    pub fn clock(self) -> LocalClock {
        self.clock
    }
}

/// Immutable sample of a mapping: remote/local rate = 1 + rate_ppb / 1e9.
/// All arithmetic uses i128 intermediates, including negative offsets.
#[derive(Clone, Copy, Debug)]
pub struct ClockMap {
    local: LocalInstant,
    remote: Reading,
    rate_ppb: i64,
    valid_for_ns: u64,
}
impl ClockMap {
    pub fn new(
        local: LocalInstant,
        remote: Reading,
        rate_ppb: i64,
        valid_for_ns: u64,
    ) -> Result<Self, Error> {
        if !(-1_000_000..=1_000_000).contains(&rate_ppb) || valid_for_ns == 0 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            local,
            remote,
            rate_ppb,
            valid_for_ns,
        })
    }
    pub fn domain(self) -> Domain {
        self.remote.domain
    }
    pub fn rate_ppb(self) -> i64 {
        self.rate_ppb
    }
    pub fn remaining_ns(&self, now: LocalInstant) -> Result<u64, Error> {
        self.fresh(now)?;
        Ok(self.valid_for_ns - (now.ns() - self.local.ns))
    }
    fn fresh(&self, now: LocalInstant) -> Result<(), Error> {
        if now.clock != self.local.clock {
            return Err(Error::WrongDomain);
        }
        let age = now
            .ns
            .checked_sub(self.local.ns)
            .ok_or(Error::TimeReversed)?;
        if age > self.valid_for_ns {
            return Err(Error::Stale);
        }
        Ok(())
    }
    pub fn to_network(&self, instant: LocalInstant, now: LocalInstant) -> Result<Reading, Error> {
        self.fresh(now)?;
        if instant.clock != self.local.clock {
            return Err(Error::WrongDomain);
        }
        let delta = i128::from(instant.ns) - i128::from(self.local.ns);
        let ns = i128::from(self.remote.ns)
            + delta * (1_000_000_000 + i128::from(self.rate_ppb)) / 1_000_000_000;
        Ok(self
            .remote
            .domain
            .reading(u64::try_from(ns).map_err(|_| Error::Overflow)?))
    }
    pub fn to_local(&self, reading: Reading, now: LocalInstant) -> Result<LocalInstant, Error> {
        self.fresh(now)?;
        if reading.domain != self.remote.domain {
            return Err(Error::WrongDomain);
        }
        let delta = i128::from(reading.ns) - i128::from(self.remote.ns);
        let ns = i128::from(self.local.ns)
            + delta * 1_000_000_000 / (1_000_000_000 + i128::from(self.rate_ppb));
        Ok(self
            .local
            .clock
            .at(u64::try_from(ns).map_err(|_| Error::Overflow)?))
    }
}

/// Preserve the same physical instant when bridging incoming/outgoing masters.
/// Both mappings must refer to the same local clock epoch and be fresh NOW.
pub fn translate(
    reading: Reading,
    from: &ClockMap,
    to: &ClockMap,
    now: LocalInstant,
) -> Result<Reading, Error> {
    let local = from.to_local(reading, now)?;
    to.to_network(local, now)
}

impl crate::Timeline {
    /// Adapt the receiver's measured offset using the SAME local clock epoch
    /// that timestamped its packets. An adapter supplies the selection epoch
    /// and freshness policy; this does not infer a restart from ClockID alone.
    pub fn clock_map(
        &self,
        local_clock: LocalClock,
        epoch: u64,
        valid_for_ns: u64,
    ) -> Result<ClockMap, Error> {
        let domain = Domain::new(self.clock_id, epoch)?;
        let remote = u64::try_from(i128::from(self.updated_ns) + i128::from(self.offset_ns))
            .map_err(|_| Error::Overflow)?;
        ClockMap::new(
            local_clock.at(self.updated_ns),
            domain.reading(remote),
            0,
            valid_for_ns,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local() -> LocalClock {
        LocalClock::new(1).unwrap()
    }
    fn domain(id: u64, epoch: u64) -> Domain {
        Domain::new(id, epoch).unwrap()
    }
    #[test]
    fn two_remote_masters_share_the_same_local_presentation_instant() {
        let l = local();
        let a = domain(11, 1);
        let b = domain(22, 1);
        let rx = ClockMap::new(
            l.at(5_000_000_000),
            a.reading(105_000_000_000),
            0,
            2_000_000_000,
        )
        .unwrap();
        let tx = ClockMap::new(
            l.at(5_000_000_000),
            b.reading(25_000_000_000),
            0,
            2_000_000_000,
        )
        .unwrap();
        let incoming = a.reading(106_000_000_000);
        let outgoing = translate(incoming, &rx, &tx, l.at(5_500_000_000)).unwrap();
        assert_eq!(outgoing, b.reading(26_000_000_000));
        assert_eq!(
            rx.to_local(incoming, l.at(5_500_000_000)),
            tx.to_local(outgoing, l.at(5_500_000_000))
        );
    }
    #[test]
    fn drift_is_applied_once_and_inverse_roundtrip_is_bounded() {
        let l = local();
        let d = domain(1, 1);
        for rate in [-999_999, -100_000, 0, 100_000, 999_999] {
            let map = ClockMap::new(
                l.at(10_000_000_000),
                d.reading(20_000_000_000),
                rate,
                10_000_000_000,
            )
            .unwrap();
            for time in [9_000_000_000, 10_000_000_000, 17_123_456_789] {
                let r = map.to_network(l.at(time), l.at(10_000_000_000)).unwrap();
                let back = map.to_local(r, l.at(10_000_000_000)).unwrap();
                assert!(back.ns.abs_diff(time) <= 1);
            }
        }
    }
    #[test]
    fn stale_maps_and_clock_restart_are_rejected() {
        let l = local();
        let old = domain(1, 1);
        let new = domain(1, 2);
        let map = ClockMap::new(l.at(100), new.reading(1000), 0, 50).unwrap();
        assert_eq!(
            map.to_local(old.reading(1000), l.at(100)),
            Err(Error::WrongDomain)
        );
        assert_eq!(map.to_network(l.at(200), l.at(151)), Err(Error::Stale));
        assert_eq!(
            map.to_network(l.at(100), l.at(99)),
            Err(Error::TimeReversed)
        );
        assert!(map.to_network(l.at(200), l.at(150)).is_ok());
    }
    #[test]
    fn different_local_epochs_cannot_be_bridged() {
        let a = local();
        let b = LocalClock::new(2).unwrap();
        let d = domain(7, 1);
        let from = ClockMap::new(a.at(100), d.reading(200), 0, 100).unwrap();
        let to = ClockMap::new(b.at(100), domain(8, 1).reading(300), 0, 100).unwrap();
        assert_eq!(
            translate(d.reading(220), &from, &to, a.at(100)),
            Err(Error::WrongDomain)
        );
    }
    #[test]
    fn negative_offsets_and_u64_limits_never_wrap() {
        let l = local();
        let d = domain(1, 1);
        let map = ClockMap::new(l.at(100), d.reading(0), 0, 10).unwrap();
        assert_eq!(map.to_network(l.at(99), l.at(100)), Err(Error::Overflow));
        let high = ClockMap::new(l.at(0), d.reading(u64::MAX), 0, 10).unwrap();
        assert_eq!(high.to_network(l.at(1), l.at(0)), Err(Error::Overflow));
        assert!(ClockMap::new(l.at(0), d.reading(0), i64::MIN, 1).is_err());
        assert!(Domain::new(0, 1).is_err());
        assert!(Domain::new(1, 0).is_err());
    }
    #[test]
    fn receiver_offset_uses_the_common_monotonic_clock_and_expires() {
        let l = local();
        let measured = crate::Timeline {
            clock_id: 11,
            offset_ns: -100,
            updated_ns: 1000,
        };
        let map = measured.clock_map(l, 7, 50).unwrap();
        assert_eq!(
            map.to_network(l.at(1020), l.at(1020)).unwrap(),
            domain(11, 7).reading(920)
        );
        assert_eq!(map.to_network(l.at(1051), l.at(1051)), Err(Error::Stale));
    }
}
