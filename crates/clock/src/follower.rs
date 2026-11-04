//! Bounded Sync/Follow_Up correlation for ONE selected source and epoch.
//! Estimates one-way offset only: no delay measurement, rate servo or claim
//! of hardware clock lock. Maps must be labelled OneWay by the service.
use crate::{
    OffsetFilter,
    domain::{ClockMap, Domain, LocalClock, LocalInstant},
    role::Dataset,
};
use std::{collections::BTreeMap, net::IpAddr};

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub pairing_lease_ns: u64,
    pub mapping_lease_ns: u64,
    pub warmup_samples: u8,
    pub step_threshold_ns: u64,
}
impl Config {
    pub fn validate(self) -> Result<(), Error> {
        if self.pairing_lease_ns == 0
            || self.mapping_lease_ns == 0
            || !(2..=64).contains(&self.warmup_samples)
            || self.step_threshold_ns == 0
            || self.step_threshold_ns > i64::MAX as u64 / 4
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    WrongClock,
    TimeReversed,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Ignored,
    Sample,
    Step,
}
#[derive(Clone, Copy)]
struct Pending {
    ingress: LocalInstant,
    correction: i64,
}
pub struct Follower {
    peer: IpAddr,
    source: Dataset,
    domain: Domain,
    local: LocalClock,
    config: Config,
    pending: BTreeMap<u16, Pending>,
    last_seq: Option<u16>,
    last_seen: Option<u64>,
    filter: OffsetFilter,
    count: u32,
    updated: Option<u64>,
}
impl Follower {
    pub fn new(
        peer: IpAddr,
        source: Dataset,
        domain: Domain,
        local: LocalClock,
        config: Config,
    ) -> Result<Self, Error> {
        config.validate()?;
        if source.grandmaster != domain.id() || source.source_clock == 0 || source.source_port == 0
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            peer,
            source,
            domain,
            local,
            config,
            pending: BTreeMap::new(),
            last_seq: None,
            last_seen: None,
            filter: OffsetFilter::default(),
            count: 0,
            updated: None,
        })
    }
    fn clear(&mut self) {
        self.pending.clear();
        self.clear_measurement();
    }
    fn clear_measurement(&mut self) {
        self.filter.reset();
        self.count = 0;
        self.updated = None;
        self.last_seq = None;
    }
    pub fn receive(
        &mut self,
        peer: IpAddr,
        packet: &[u8],
        now: LocalInstant,
    ) -> Result<Event, Error> {
        if now.clock() != self.local {
            self.clear();
            return Err(Error::WrongClock);
        }
        if self.last_seen.is_some_and(|last| now.ns() < last) {
            self.clear();
            return Err(Error::TimeReversed);
        }
        self.last_seen = Some(now.ns());
        // An incomplete warmup must not retain a completed sequence forever
        // across a source restart. Published mapping epochs are separately
        // revoked by the engine before accepting a replacement sample.
        if self
            .updated
            .is_some_and(|at| now.ns() - at > self.config.mapping_lease_ns)
        {
            self.clear_measurement();
        }
        self.pending
            .retain(|_, p| now.ns() - p.ingress.ns() <= self.config.pairing_lease_ns);
        if peer != self.peer
            || !(44..=512).contains(&packet.len())
            || packet[0] >> 4 != 1
            || packet[1] & 15 != 2
            || packet[4] != 0
            || u16::from_be_bytes([packet[2], packet[3]]) as usize != packet.len()
            || u64::from_be_bytes(packet[20..28].try_into().unwrap()) != self.source.source_clock
            || u16::from_be_bytes([packet[28], packet[29]]) != self.source.source_port
        {
            return Ok(Event::Ignored);
        }
        let seq = u16::from_be_bytes([packet[30], packet[31]]);
        if self.last_seq.is_some_and(|last| {
            let delta = seq.wrapping_sub(last);
            delta == 0 || delta >= 0x8000
        }) {
            return Ok(Event::Ignored);
        }
        let correction = i64::from_be_bytes(packet[8..16].try_into().unwrap()) / 65536;
        match packet[0] & 15 {
            0 if u16::from_be_bytes([packet[6], packet[7]]) & 0x0200 != 0 => {
                if self.pending.contains_key(&seq) {
                    return Ok(Event::Ignored);
                }
                if self.pending.len() >= 8 {
                    let oldest = *self
                        .pending
                        .iter()
                        .min_by_key(|(_, p)| p.ingress.ns())
                        .unwrap()
                        .0;
                    self.pending.remove(&oldest);
                }
                self.pending.insert(
                    seq,
                    Pending {
                        ingress: now,
                        correction,
                    },
                );
                Ok(Event::Ignored)
            }
            0 => {
                let Some(origin) = timestamp(&packet[34..44]) else {
                    return Ok(Event::Ignored);
                };
                self.accept(seq, now, origin, i128::from(correction))
            }
            8 => {
                let Some(origin) = timestamp(&packet[34..44]) else {
                    return Ok(Event::Ignored);
                };
                let Some(sync) = self.pending.remove(&seq) else {
                    return Ok(Event::Ignored);
                };
                self.accept(
                    seq,
                    sync.ingress,
                    origin,
                    i128::from(correction) + i128::from(sync.correction),
                )
            }
            _ => Ok(Event::Ignored),
        }
    }
    fn accept(
        &mut self,
        seq: u16,
        ingress: LocalInstant,
        origin: u64,
        correction: i128,
    ) -> Result<Event, Error> {
        let offset = i128::from(origin) + correction - i128::from(ingress.ns());
        let Ok(offset) = i64::try_from(offset) else {
            return Ok(Event::Ignored);
        };
        if self.filter.offset().is_some_and(|old| {
            (i128::from(offset) - i128::from(old)).abs() > i128::from(self.config.step_threshold_ns)
        }) {
            self.clear();
            return Ok(Event::Step);
        }
        self.filter.update(offset);
        self.count = self.count.saturating_add(1);
        self.updated = Some(ingress.ns());
        self.last_seq = Some(seq);
        Ok(Event::Sample)
    }
    pub fn mapping(&self, now: LocalInstant) -> Option<ClockMap> {
        if now.clock() != self.local || self.count < u32::from(self.config.warmup_samples) {
            return None;
        }
        let updated = self.updated?;
        if now.ns().checked_sub(updated)? > self.config.mapping_lease_ns {
            return None;
        }
        crate::Timeline {
            clock_id: self.domain.id(),
            offset_ns: self.filter.offset()?,
            updated_ns: updated,
        }
        .clock_map(
            self.local,
            self.domain.epoch(),
            self.config.mapping_lease_ns,
        )
        .ok()
    }
}
fn timestamp(data: &[u8]) -> Option<u64> {
    let seconds = u64::from_be_bytes([0, 0, data[0], data[1], data[2], data[3], data[4], data[5]]);
    let nanos = u32::from_be_bytes(data[6..10].try_into().ok()?);
    if nanos >= 1_000_000_000 {
        return None;
    }
    seconds
        .checked_mul(1_000_000_000)?
        .checked_add(u64::from(nanos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptp_sender::Master;
    fn peer() -> IpAddr {
        "192.0.2.1".parse().unwrap()
    }
    fn local() -> LocalClock {
        LocalClock::new(1).unwrap()
    }
    fn config() -> Config {
        Config {
            pairing_lease_ns: 100_000_000,
            mapping_lease_ns: 2_000_000_000,
            warmup_samples: 3,
            step_threshold_ns: 10_000_000,
        }
    }
    fn fixture() -> (Master, Follower) {
        let mut master = Master::new(Domain::new(200, 1).unwrap()).unwrap();
        master.set_peers(&[peer()]).unwrap();
        let follower = Follower::new(
            peer(),
            master.dataset(),
            Domain::new(200, 7).unwrap(),
            local(),
            config(),
        )
        .unwrap();
        (master, follower)
    }
    fn pair(master: &mut Master, n: u64) -> (Vec<u8>, Vec<u8>) {
        let sync = master
            .poll(n * 1_000_000_000)
            .unwrap()
            .into_iter()
            .find(|d| d.completion.is_some())
            .unwrap();
        let follow = master
            .complete_send(
                sync.completion.as_ref().unwrap(),
                Domain::new(200, 1)
                    .unwrap()
                    .reading(10_000_000_000 + n * 1_000_000_000),
            )
            .unwrap();
        (sync.bytes, follow.bytes)
    }
    #[test]
    fn pairs_unique_sequences_and_publishes_only_after_warmup_then_expires() {
        let (mut m, mut f) = fixture();
        for n in 0..3 {
            let (s, u) = pair(&mut m, n);
            let at = n * 1_000_000_000 + 300;
            f.receive(peer(), &s, local().at(at)).unwrap();
            f.receive(peer(), &u, local().at(at + 10)).unwrap();
            assert_eq!(f.mapping(local().at(at + 10)).is_some(), n == 2);
            assert_eq!(
                f.receive(peer(), &u, local().at(at + 20)).unwrap(),
                Event::Ignored
            );
        }
        let now = local().at(2_000_000_400);
        let mapped = f.mapping(now).unwrap().to_network(now, now).unwrap();
        assert_eq!(mapped.ns(), 12_000_000_100);
        assert_eq!(mapped.domain().epoch(), 7);
        assert!(f.mapping(local().at(4_000_000_301)).is_none());
    }
    #[test]
    fn reordered_or_stale_followup_does_not_create_a_measurement() {
        let (mut m, mut f) = fixture();
        let (s, u) = pair(&mut m, 0);
        assert_eq!(
            f.receive(peer(), &u, local().at(1)).unwrap(),
            Event::Ignored
        );
        f.receive(peer(), &s, local().at(2)).unwrap();
        assert_eq!(
            f.receive(peer(), &u, local().at(100_000_003)).unwrap(),
            Event::Ignored
        );
        assert_eq!(f.count, 0);
    }
    #[test]
    fn duplicate_sync_keeps_first_ingress_and_step_revokes_map() {
        let (mut m, mut f) = fixture();
        for n in 0..3 {
            let (s, u) = pair(&mut m, n);
            let at = n * 1_000_000_000 + 300;
            f.receive(peer(), &s, local().at(at)).unwrap();
            f.receive(peer(), &s, local().at(at + 100)).unwrap();
            f.receive(peer(), &u, local().at(at + 200)).unwrap();
        }
        assert_eq!(f.filter.offset(), Some(9_999_999_700));
        let (s, u) = pair(&mut m, 4);
        f.receive(peer(), &s, local().at(4_000_000_300)).unwrap();
        assert_eq!(
            f.receive(peer(), &u, local().at(4_000_000_310)).unwrap(),
            Event::Sample
        );
        let (s, u) = pair(&mut m, 5);
        f.receive(peer(), &s, local().at(5_100_000_300)).unwrap();
        assert_eq!(
            f.receive(peer(), &u, local().at(5_100_000_310)).unwrap(),
            Event::Step
        );
        assert!(f.mapping(local().at(5_100_000_310)).is_none());
    }
    #[test]
    fn wrong_source_and_invalid_timestamp_cannot_refresh_mapping() {
        let (mut m, mut f) = fixture();
        let (s, mut u) = pair(&mut m, 0);
        f.receive(peer(), &s, local().at(0)).unwrap();
        u[40..44].copy_from_slice(&1_000_000_000u32.to_be_bytes());
        assert_eq!(
            f.receive(peer(), &u, local().at(1)).unwrap(),
            Event::Ignored
        );
        assert_eq!(
            f.receive("192.0.2.9".parse().unwrap(), &s, local().at(2))
                .unwrap(),
            Event::Ignored
        );
        assert_eq!(f.count, 0);
    }
    #[test]
    fn stale_incomplete_warmup_can_accept_restarted_sequence() {
        let (mut m, mut f) = fixture();
        let (s, u) = pair(&mut m, 10);
        f.receive(peer(), &s, local().at(10_000_000_000)).unwrap();
        f.receive(peer(), &u, local().at(10_000_000_010)).unwrap();
        assert_eq!(f.count, 1);
        let mut restarted = Master::new(Domain::new(200, 1).unwrap()).unwrap();
        restarted.set_peers(&[peer()]).unwrap();
        let (s, u) = pair(&mut restarted, 0);
        f.receive(peer(), &s, local().at(13_000_000_000)).unwrap();
        assert_eq!(
            f.receive(peer(), &u, local().at(13_000_000_010)).unwrap(),
            Event::Sample
        );
        assert_eq!(f.count, 1);
        assert!(f.mapping(local().at(13_000_000_010)).is_none());
    }
    #[test]
    fn unmatched_sync_flood_is_bounded_and_does_not_publish_time() {
        let (mut m, mut f) = fixture();
        for n in 0..100 {
            let (s, _) = pair(&mut m, n);
            f.receive(peer(), &s, local().at(n)).unwrap();
        }
        assert_eq!(f.pending.len(), 8);
        assert_eq!(f.count, 0);
        assert!(f.mapping(local().at(100)).is_none());
    }
}
