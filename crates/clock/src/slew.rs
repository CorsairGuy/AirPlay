//! Explicit, bounded application of clock-map updates. No I/O or PCM servo.
//! Preserve NOW at each update; converge by rate, never by a phase step.
//! Already queued media deadlines must remain frozen by the renderer.
use crate::domain::{ClockMap, Error, LocalInstant};

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub horizon_ns: u64,
    pub max_slew_ppb: i64,
    pub max_phase_ns: u64,
    pub max_lease_ns: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct Update {
    pub mapping: ClockMap,
    /// Target minus output at the update instant. Not acoustic error/lock.
    pub phase_error_ns: i64,
}
pub struct Slew {
    config: Config,
    current: Option<ClockMap>,
    last: Option<LocalInstant>,
}
impl Slew {
    pub fn new(config: Config) -> Result<Self, Error> {
        if config.horizon_ns == 0
            || config.max_lease_ns == 0
            || config.max_lease_ns > config.horizon_ns
            || !(1..=1_000_000).contains(&config.max_slew_ppb)
            || config.max_phase_ns == 0
            || config.max_phase_ns > i64::MAX as u64
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            config,
            current: None,
            last: None,
        })
    }
    pub fn clear(&mut self) {
        self.current = None;
        self.last = None;
    }
    /// Explicit initial arm (including after a discontinuity). Never auto-rearm.
    pub fn start(&mut self, target: ClockMap, now: LocalInstant) -> Result<Update, Error> {
        if self.current.is_some() {
            return Err(Error::Invalid);
        }
        let remote = target.to_network(now, now)?;
        let lease = target.remaining_ns(now)?.min(self.config.max_lease_ns);
        let mapping = ClockMap::new(now, remote, target.rate_ppb(), lease)?;
        self.current = Some(mapping);
        self.last = Some(now);
        Ok(Update {
            mapping,
            phase_error_ns: 0,
        })
    }
    pub fn update(&mut self, target: ClockMap, now: LocalInstant) -> Result<Update, Error> {
        let result = self.apply(target, now);
        // An expired source, changed epoch or large step requires explicit
        // re-arming, never a silent switch back to an old map.
        if result.is_err() {
            self.clear();
        }
        result
    }
    fn apply(&mut self, target: ClockMap, now: LocalInstant) -> Result<Update, Error> {
        let old = self.current.ok_or(Error::Invalid)?;
        let last = self.last.ok_or(Error::Invalid)?;
        if now.clock() != last.clock() {
            return Err(Error::WrongDomain);
        }
        if now.ns() < last.ns() {
            return Err(Error::TimeReversed);
        }
        if target.domain() != old.domain() {
            return Err(Error::WrongDomain);
        }
        let previous = old.to_network(now, now)?;
        let wanted = target.to_network(now, now)?;
        let phase = i128::from(wanted.ns()) - i128::from(previous.ns());
        if phase.abs() > i128::from(self.config.max_phase_ns) {
            return Err(Error::Invalid);
        }
        let correction = (phase * 1_000_000_000 / i128::from(self.config.horizon_ns)).clamp(
            -i128::from(self.config.max_slew_ppb),
            i128::from(self.config.max_slew_ppb),
        );
        let rate = (i128::from(target.rate_ppb()) + correction).clamp(-1_000_000, 1_000_000) as i64;
        let lease = target.remaining_ns(now)?.min(self.config.max_lease_ns);
        let mapping = ClockMap::new(now, previous, rate, lease)?;
        self.current = Some(mapping);
        self.last = Some(now);
        Ok(Update {
            mapping,
            phase_error_ns: phase as i64,
        })
    }
    pub fn mapping(&mut self, now: LocalInstant) -> Option<ClockMap> {
        let map = self.current?;
        let last = self.last?;
        if now.clock() != last.clock()
            || now.ns() < last.ns()
            || map.remaining_ns(now).map_or(true, |n| n == 0)
        {
            self.clear();
            return None;
        }
        self.last = Some(now);
        Some(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Domain, LocalClock};
    fn local() -> LocalClock {
        LocalClock::new(1).unwrap()
    }
    fn domain() -> Domain {
        Domain::new(100, 1).unwrap()
    }
    fn config() -> Config {
        Config {
            horizon_ns: 1_000_000_000,
            max_slew_ppb: 100_000,
            max_phase_ns: 10_000_000,
            max_lease_ns: 1_000_000_000,
        }
    }
    fn map(at: u64, offset: i64, rate: i64) -> ClockMap {
        ClockMap::new(
            local().at(at),
            domain().reading((i128::from(at) + 10_000_000_000 + i128::from(offset)) as u64),
            rate,
            2_000_000_000,
        )
        .unwrap()
    }
    #[test]
    fn positive_and_negative_corrections_are_continuous_bounded_and_converge() {
        for offset in [-500_000, 500_000] {
            let mut s = Slew::new(config()).unwrap();
            s.start(map(0, 0, 0), local().at(0)).unwrap();
            let mut previous = 500_001i64;
            for n in 1..20 {
                let at = n * 500_000_000;
                let now = local().at(at);
                let before = s.mapping(now).unwrap().to_network(now, now).unwrap();
                let update = s.update(map(at, offset, 0), now).unwrap();
                assert_eq!(update.mapping.to_network(now, now).unwrap(), before);
                assert!(update.mapping.rate_ppb().abs() <= 100_000);
                assert!(update.phase_error_ns.abs() <= previous);
                previous = update.phase_error_ns.abs();
                let later = local().at(at + 1000);
                assert!(update.mapping.to_network(later, now).unwrap().ns() > before.ns());
            }
            assert!(previous < 1000);
        }
    }
    #[test]
    fn changed_epoch_large_step_expiry_and_backward_time_revoke_until_explicit_start() {
        for mode in 0..5 {
            let mut s = Slew::new(config()).unwrap();
            s.start(map(0, 0, 0), local().at(0)).unwrap();
            let mut target = map(100, 0, 0);
            let mut now = local().at(100);
            match mode {
                0 => {
                    target = ClockMap::new(
                        now,
                        Domain::new(100, 2).unwrap().reading(10_000_000_100),
                        0,
                        1000,
                    )
                    .unwrap()
                }
                1 => target = map(100, 20_000_000, 0),
                2 => now = local().at(2_000_000_001),
                3 => {
                    s.mapping(local().at(200));
                }
                _ => now = LocalClock::new(2).unwrap().at(100),
            }
            assert!(s.update(target, now).is_err());
            assert!(s.mapping(local().at(201)).is_none());
            assert!(s.update(map(202, 0, 0), local().at(202)).is_err());
            assert!(s.start(map(203, 0, 0), local().at(203)).is_ok());
        }
    }
    #[test]
    fn target_rate_extremes_and_short_source_lease_are_respected() {
        let mut s = Slew::new(config()).unwrap();
        s.start(map(0, 0, 0), local().at(0)).unwrap();
        let now = local().at(1);
        let target = ClockMap::new(now, domain().reading(10_000_001_001), 999_999, 1000).unwrap();
        let update = s.update(target, now).unwrap();
        assert_eq!(update.mapping.rate_ppb(), 1_000_000);
        assert_eq!(update.mapping.remaining_ns(now).unwrap(), 1000);
        assert!(s.mapping(local().at(1001)).is_none());
    }
}
