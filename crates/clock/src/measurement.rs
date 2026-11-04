//! Four-timestamp peer-link estimates. Diagnostic only, not a selected GM map.
//!
//! Rate is REMOTE ns / LOCAL ns. Convert remote turnaround by DIVIDING by
//! that ratio exactly once. gPTP's correction is FollowUp - Response (not the
//! ordinary 1588 sum); checked against linuxptp 227bbd47 port_peer_delay.
//! Delay assumes a symmetric path. Asymmetry cannot be solved from four stamps.
use crate::domain::{ClockMap, Domain, LocalClock, LocalInstant, Reading};
use std::collections::VecDeque;

const SCALE: i128 = 1_000_000_000;
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub max_roundtrip_ns: u64,
    pub rate_window_ns: u64,
    pub valid_for_ns: u64,
    /// Explicit capture uncertainty, zero only for exact simulation stamps.
    pub timestamp_error_ns: u64,
}
impl Config {
    pub fn validate(self) -> Result<(), Error> {
        if self.max_roundtrip_ns == 0
            || self.rate_window_ns < 1_000_000
            || self.valid_for_ns == 0
            || self.timestamp_error_ns > self.max_roundtrip_ns
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    WrongDomain,
    TimeOrder,
    Overflow,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
#[derive(Clone, Copy, Debug)]
pub struct Exchange {
    pub sent: LocalInstant,
    pub peer_received: Reading,
    pub peer_sent: Reading,
    pub received: LocalInstant,
    pub response_correction_ns: i64,
    pub followup_correction_ns: i64,
}
#[derive(Clone, Copy, Debug)]
pub struct Estimate {
    reference_local: LocalInstant,
    reference_remote: Reading,
    valid_for_ns: u64,
    pub neighbor: Domain,
    pub measured_at: LocalInstant,
    pub mean_path_ns: u64,
    /// Offset uncertainty includes the unidentifiable directional asymmetry.
    pub offset_uncertainty_ns: u64,
    pub neighbor_rate_ppb: Option<i64>,
    pub rate_uncertainty_ppb: Option<u64>,
}
/// Application limits, not a declaration of hardware/acoustic lock.
#[derive(Clone, Copy, Debug)]
pub struct MappingPolicy {
    pub max_age_ns: u64,
    pub lease_ns: u64,
    pub max_uncertainty_ns: u64,
    pub max_rate_uncertainty_ppb: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct MeasuredMap {
    pub mapping: ClockMap,
    /// Bound over the entire granted lease, under stable-rate/capture assumptions.
    pub uncertainty_ns: u64,
}
impl Estimate {
    /// Only a direct root neighbor can supply the selected group's scale.
    /// Never relabel a boundary clock as its upstream grandmaster.
    pub fn mapping_for(
        self,
        selected: Domain,
        now: LocalInstant,
        policy: MappingPolicy,
    ) -> Option<MeasuredMap> {
        if selected != self.neighbor
            || now.clock() != self.reference_local.clock()
            || policy.max_age_ns == 0
            || policy.lease_ns == 0
        {
            return None;
        }
        let age = now.ns().checked_sub(self.measured_at.ns())?;
        let remaining = policy.max_age_ns.min(self.valid_for_ns).checked_sub(age)?;
        let lease = policy.lease_ns.min(remaining);
        if lease == 0 {
            return None;
        }
        let rate = self.neighbor_rate_ppb?;
        let rate_error = self.rate_uncertainty_ppb?;
        if rate_error > policy.max_rate_uncertainty_ppb {
            return None;
        }
        let horizon = now
            .ns()
            .checked_sub(self.reference_local.ns())?
            .checked_add(lease)?;
        // Ceiling avoids understating even a fractional nanosecond of drift.
        let drift = (u128::from(horizon) * u128::from(rate_error) + 999_999_999) / 1_000_000_000;
        let uncertainty = self
            .offset_uncertainty_ns
            .checked_add(u64::try_from(drift).ok()?)?;
        if uncertainty > policy.max_uncertainty_ns {
            return None;
        }
        let source =
            ClockMap::new(self.reference_local, self.reference_remote, rate, u64::MAX).ok()?;
        let remote_now = source.to_network(now, now).ok()?;
        Some(MeasuredMap {
            mapping: ClockMap::new(now, remote_now, rate, lease).ok()?,
            uncertainty_ns: uncertainty,
        })
    }
}
#[derive(Clone, Copy)]
struct Point {
    local: u64,
    remote: i128,
    roundtrip: u64,
}
pub struct Estimator {
    neighbor: Domain,
    local: LocalClock,
    config: Config,
    points: VecDeque<Point>,
    latest: Option<Estimate>,
}
impl Estimator {
    pub fn new(neighbor: Domain, local: LocalClock, config: Config) -> Result<Self, Error> {
        config.validate()?;
        Ok(Self {
            neighbor,
            local,
            config,
            points: VecDeque::new(),
            latest: None,
        })
    }
    pub fn observe(&mut self, e: Exchange) -> Result<Option<Estimate>, Error> {
        if e.sent.clock() != self.local
            || e.received.clock() != self.local
            || e.peer_received.domain() != self.neighbor
            || e.peer_sent.domain() != self.neighbor
        {
            return Err(Error::WrongDomain);
        }
        if e.received.ns() < e.sent.ns()
            || e.peer_sent.ns() < e.peer_received.ns()
            || self
                .latest
                .is_some_and(|old| e.sent.ns() <= old.measured_at.ns())
        {
            return Err(Error::TimeOrder);
        }
        let elapsed = e.received.ns() - e.sent.ns();
        if elapsed > self.config.max_roundtrip_ns {
            return Ok(None);
        }
        let corrected_sent = i128::from(e.peer_sent.ns()) + i128::from(e.followup_correction_ns)
            - i128::from(e.response_correction_ns);
        let remote_turn = corrected_sent - i128::from(e.peer_received.ns());
        if remote_turn < 0 {
            return Ok(None);
        }
        let point = Point {
            local: e.sent.ns() + elapsed / 2,
            remote: (i128::from(e.peer_received.ns()) + corrected_sent) / 2,
            roundtrip: elapsed,
        };
        let mut slopes = Vec::new();
        for old in &self.points {
            let Some(span) = point.local.checked_sub(old.local) else {
                continue;
            };
            if span < self.config.rate_window_ns {
                continue;
            }
            let remote_span = point.remote - old.remote;
            let ppb = (remote_span - i128::from(span)) * SCALE / i128::from(span);
            if !(-1_000_000..=1_000_000).contains(&ppb) {
                continue;
            }
            let uncertainty = (i128::from(old.roundtrip)
                + i128::from(elapsed)
                + 4 * i128::from(self.config.timestamp_error_ns))
                * SCALE
                / i128::from(span)
                + 1;
            slopes.push((
                ppb as i64,
                u64::try_from(uncertainty).map_err(|_| Error::Overflow)?,
            ));
        }
        slopes.sort_unstable_by_key(|s| s.0);
        let rate = (slopes.len() >= 3).then(|| slopes[slopes.len() / 2]);
        let ppb = rate.map(|r| r.0).unwrap_or(0);
        let turn_local = remote_turn * SCALE / (SCALE + i128::from(ppb));
        let network = i128::from(elapsed) - turn_local;
        let capture = 4 * i128::from(self.config.timestamp_error_ns);
        // Software send/receive stamps may overlap by the explicit capture
        // budget. Larger negative network time is invalid, never wrapped.
        if network < -capture {
            return Ok(None);
        }
        let delay = u64::try_from(network.max(0) / 2).map_err(|_| Error::Overflow)?;
        // Two rates within the supported +/-1,000,000 ppb range can differ
        // by 2,000,000 ppb. Before warmup the assumed rate is exactly zero.
        let rate_error = rate.map(|r| r.1.min(2_000_000)).unwrap_or(1_000_000);
        let uncertainty = i128::from(delay)
            + capture
            + turn_local * i128::from(rate_error) / SCALE
            + (-network).max(0);
        let result = Estimate {
            reference_local: self.local.at(point.local),
            reference_remote: self
                .neighbor
                .reading(u64::try_from(point.remote).map_err(|_| Error::Overflow)?),
            valid_for_ns: self.config.valid_for_ns,
            neighbor: self.neighbor,
            measured_at: e.received,
            mean_path_ns: delay,
            offset_uncertainty_ns: u64::try_from(uncertainty).map_err(|_| Error::Overflow)?,
            neighbor_rate_ppb: rate.map(|r| r.0),
            rate_uncertainty_ppb: rate.map(|r| r.1),
        };
        if self.points.len() == 9 {
            self.points.pop_front();
        }
        self.points.push_back(point);
        self.latest = Some(result);
        Ok(Some(result))
    }
    pub fn latest(&self, now: LocalInstant) -> Option<Estimate> {
        let value = self.latest?;
        if now.clock() != self.local
            || now.ns().checked_sub(value.measured_at.ns())? > self.config.valid_for_ns
        {
            return None;
        }
        Some(value)
    }
    pub fn reset(&mut self) {
        self.points.clear();
        self.latest = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn local() -> LocalClock {
        LocalClock::new(1).unwrap()
    }
    fn domain() -> Domain {
        Domain::new(200, 7).unwrap()
    }
    fn config() -> Config {
        Config {
            max_roundtrip_ns: 100_000_000,
            rate_window_ns: 1_000_000_000,
            valid_for_ns: 3_000_000_000,
            timestamp_error_ns: 0,
        }
    }
    fn remote(local: i128, ppb: i64) -> Reading {
        domain().reading((10_000_000_000i128 + local * (SCALE + i128::from(ppb)) / SCALE) as u64)
    }
    fn exchange(at: u64, forward: u64, back: u64, work: u64, ppb: i64) -> Exchange {
        Exchange {
            sent: local().at(at),
            peer_received: remote(i128::from(at + forward), ppb),
            peer_sent: remote(i128::from(at + forward + work), ppb),
            received: local().at(at + forward + work + back),
            response_correction_ns: 0,
            followup_correction_ns: 0,
        }
    }
    #[test]
    fn measured_map_tracks_rate_and_preserves_a_common_presentation_instant() {
        for ppb in [-200_000, 300_000] {
            let mut e = Estimator::new(domain(), local(), config()).unwrap();
            for n in 0..12 {
                e.observe(exchange(
                    1_000_000_000 + n * 1_000_000_000,
                    2_000_000,
                    2_000_000,
                    10_000_000,
                    ppb,
                ))
                .unwrap();
            }
            let estimate = e.latest.unwrap();
            let now = estimate.measured_at;
            let policy = MappingPolicy {
                max_age_ns: 1_000_000_000,
                lease_ns: 500_000_000,
                max_uncertainty_ns: 100_000_000,
                max_rate_uncertainty_ppb: 100_000_000,
            };
            let map = estimate.mapping_for(domain(), now, policy).unwrap();
            let target = local().at(now.ns() + 250_000_000);
            let network = map.mapping.to_network(target, now).unwrap();
            assert!(
                (i128::from(network.ns()) - i128::from(remote(i128::from(target.ns()), ppb).ns()))
                    .abs()
                    <= 2
            );
            assert!(
                (i128::from(map.mapping.to_local(network, now).unwrap().ns())
                    - i128::from(target.ns()))
                .abs()
                    <= 2
            );
            assert!(
                map.mapping
                    .to_network(target, local().at(now.ns() + 500_000_001))
                    .is_err()
            );
            assert!(
                estimate
                    .mapping_for(Domain::new(50, 7).unwrap(), now, policy)
                    .is_none()
            );
            assert!(
                estimate
                    .mapping_for(Domain::new(200, 8).unwrap(), now, policy)
                    .is_none()
            );
            assert!(
                estimate
                    .mapping_for(domain(), local().at(now.ns() + 1_000_000_000), policy)
                    .is_none()
            );
            assert!(
                estimate
                    .mapping_for(
                        domain(),
                        now,
                        MappingPolicy {
                            max_uncertainty_ns: 1,
                            ..policy
                        }
                    )
                    .is_none()
            );
            assert!(
                estimate
                    .mapping_for(
                        domain(),
                        now,
                        MappingPolicy {
                            max_rate_uncertainty_ppb: 0,
                            ..policy
                        }
                    )
                    .is_none()
            );
        }
    }
    #[test]
    fn unqualified_measurement_and_wrong_local_clock_do_not_produce_maps() {
        let mut e = Estimator::new(domain(), local(), config()).unwrap();
        let estimate = e
            .observe(exchange(1_000_000_000, 1000, 1000, 1000, 0))
            .unwrap()
            .unwrap();
        let policy = MappingPolicy {
            max_age_ns: 1_000_000_000,
            lease_ns: 100_000_000,
            max_uncertainty_ns: u64::MAX,
            max_rate_uncertainty_ppb: u64::MAX,
        };
        assert!(
            estimate
                .mapping_for(domain(), estimate.measured_at, policy)
                .is_none()
        );
        assert!(
            estimate
                .mapping_for(
                    domain(),
                    LocalClock::new(2).unwrap().at(estimate.measured_at.ns()),
                    policy
                )
                .is_none()
        );
    }
    #[test]
    fn symmetric_delay_and_fast_slow_clock_rates_are_recovered_without_double_scaling() {
        for ppb in [-200_000, 0, 300_000] {
            let mut e = Estimator::new(domain(), local(), config()).unwrap();
            let mut last = None;
            for n in 0..12 {
                last = e
                    .observe(exchange(
                        1_000_000_000 + n * 1_000_000_000,
                        2_000_000,
                        2_000_000,
                        10_000_000,
                        ppb,
                    ))
                    .unwrap();
            }
            let value = last.unwrap();
            assert!((value.mean_path_ns as i64 - 2_000_000).abs() <= 1);
            assert!((value.neighbor_rate_ppb.unwrap() - ppb).abs() <= 1);
            assert!(value.rate_uncertainty_ppb.is_some());
            assert_eq!(e.points.len(), 9);
        }
    }
    #[test]
    fn asymmetric_path_is_not_claimed_to_have_an_exact_offset() {
        let mut e = Estimator::new(domain(), local(), config()).unwrap();
        let value = e
            .observe(exchange(1_000_000_000, 1_000_000, 5_000_000, 1_000_000, 0))
            .unwrap()
            .unwrap();
        assert_eq!(value.mean_path_ns, 3_000_000);
        assert!(value.offset_uncertainty_ns >= 2_000_000);
        assert!(value.neighbor_rate_ppb.is_none());
    }
    #[test]
    fn gptp_correction_difference_and_epoch_mismatch_are_checked() {
        let mut e = Estimator::new(domain(), local(), config()).unwrap();
        let mut x = exchange(1_000_000_000, 1000, 1000, 1000, 0);
        x.response_correction_ns = 400;
        x.followup_correction_ns = 1000;
        assert_eq!(e.observe(x).unwrap().unwrap().mean_path_ns, 700);
        x.sent = local().at(2_000_000_000);
        x.received = local().at(2_000_003_000);
        x.peer_sent = Domain::new(200, 8).unwrap().reading(1);
        assert_eq!(e.observe(x).unwrap_err(), Error::WrongDomain);
    }
    #[test]
    fn delayed_duplicate_negative_and_expired_samples_do_not_become_fresh_estimates() {
        let mut e = Estimator::new(domain(), local(), config()).unwrap();
        let x = exchange(1_000_000_000, 1000, 1000, 1000, 0);
        let value = e.observe(x).unwrap().unwrap();
        assert!(e.observe(x).is_err());
        assert!(
            e.observe(exchange(2_000_000_000, 200_000_000, 1000, 1000, 0))
                .unwrap()
                .is_none()
        );
        let mut bad = exchange(3_000_000_000, 1000, 1000, 1000, 0);
        bad.peer_sent = domain().reading(bad.peer_sent.ns() + 1_000_000);
        assert!(e.observe(bad).unwrap().is_none());
        assert!(
            e.latest(local().at(value.measured_at.ns() + 3_000_000_001))
                .is_none()
        );
    }
}
