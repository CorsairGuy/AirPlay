//! Offline clock-role policy, NOT complete IEEE 1588 BMCA.
//!
//! Compares the priority/quality vector for different grandmasters. Ambiguous
//! same-grandmaster paths hold Listening instead of guessing topology. Explicit
//! FollowOnly is available when a receiver must keep its own clock. Selection
//! never implies Sync lock, a valid ClockMap or audible receiver readiness.
use crate::domain::{Domain, LocalClock, LocalInstant};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
};

static EPOCH: AtomicU64 = AtomicU64::new(1);
fn next_epoch() -> Result<u64, Error> {
    let mut current = EPOCH.load(Ordering::Relaxed);
    loop {
        let next = current.checked_add(1).ok_or(Error::Exhausted)?;
        match EPOCH.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return Ok(current),
            Err(value) => current = value,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    WrongClock,
    TimeReversed,
    Exhausted,
    Faulted,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dataset {
    pub priority1: u8,
    pub clock_class: u8,
    pub accuracy: u8,
    pub variance: u16,
    pub priority2: u8,
    pub grandmaster: u64,
    pub steps_removed: u16,
    pub source_clock: u64,
    pub source_port: u16,
}
impl Dataset {
    /// Compatibility dataset emitted by the experimental sender. These fields
    /// describe a wire profile, not proof of a GPS-disciplined physical clock.
    pub fn airplay_profile(clock: u64) -> Result<Self, Error> {
        let data = Self {
            priority1: 128,
            clock_class: 6,
            accuracy: 0x21,
            variance: 0x436a,
            priority2: 128,
            grandmaster: clock,
            steps_removed: 0,
            source_clock: clock,
            source_port: 0x8005,
        };
        if !data.valid() {
            return Err(Error::Invalid);
        }
        Ok(data)
    }
    pub fn from_announce(packet: &[u8]) -> Option<Self> {
        Self::parse(packet).map(|(dataset, _)| dataset)
    }
    /// Ranking only for DIFFERENT grandmasters. Same-GM topology is handled
    /// separately and conservatively, not by an incomplete lexicographic BMCA.
    fn rank(self) -> (u8, u8, u8, u16, u8, u64) {
        (
            self.priority1,
            self.clock_class,
            self.accuracy,
            self.variance,
            self.priority2,
            self.grandmaster,
        )
    }
    fn valid(self) -> bool {
        self.grandmaster != 0
            && self.source_clock != 0
            && self.source_port != 0
            && self.steps_removed < 255
    }
    fn parse(packet: &[u8]) -> Option<(Self, u16)> {
        if !(64..=512).contains(&packet.len())
            || packet[0] != 0x1b
            || packet[1] & 15 != 2
            || packet[4] != 0
            || u16::from_be_bytes([packet[2], packet[3]]) as usize != packet.len()
        {
            return None;
        }
        let data = Self {
            priority1: packet[47],
            clock_class: packet[48],
            accuracy: packet[49],
            variance: u16::from_be_bytes([packet[50], packet[51]]),
            priority2: packet[52],
            grandmaster: u64::from_be_bytes(packet[53..61].try_into().ok()?),
            steps_removed: u16::from_be_bytes([packet[61], packet[62]]),
            source_clock: u64::from_be_bytes(packet[20..28].try_into().ok()?),
            source_port: u16::from_be_bytes([packet[28], packet[29]]),
        };
        data.valid()
            .then_some((data, u16::from_be_bytes([packet[30], packet[31]])))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Elect,
    FollowOnly(IpAddr),
}
pub struct Config {
    pub policy: Policy,
    pub settle_ns: u64,
    /// Explicit policy; do not assume all HomePods announce every second.
    pub announce_lease_ns: u64,
    pub min_announces: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Listening,
    Master,
    Follower(IpAddr),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub role: Role,
    pub domain: Option<Domain>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Listening,
    Local,
    Remote {
        peer: IpAddr,
        clock: u64,
        port: u16,
        gm: u64,
        incarnation: u64,
    },
}
#[derive(Clone, Copy)]
struct Candidate {
    dataset: Dataset,
    sequence: u16,
    last_ns: u64,
    count: u8,
    incarnation: u64,
}

/// One controller per selected host/group timeline; its Domain is shared by
/// RX, TX, mappings and media. Epochs are process-unique, not persisted on wire.
pub struct Controller {
    own: Dataset,
    config: Config,
    peers: BTreeSet<IpAddr>,
    foreign: BTreeMap<IpAddr, Candidate>,
    local_clock: LocalClock,
    start_ns: u64,
    last_ns: u64,
    incarnation: u64,
    key: Key,
    selection: Selection,
    faulted: bool,
}
impl Controller {
    pub fn new(
        own: Dataset,
        config: Config,
        peers: &[IpAddr],
        now: LocalInstant,
    ) -> Result<Self, Error> {
        let peers = validate_peers(peers)?;
        if !own.valid()
            || own.grandmaster != own.source_clock
            || own.steps_removed != 0
            || config.announce_lease_ns == 0
            || config.min_announces == 0
            || config.min_announces > 8
            || matches!(config.policy,Policy::FollowOnly(peer) if !peers.contains(&peer))
        {
            return Err(Error::Invalid);
        }
        Ok(Self {
            own,
            config,
            peers,
            foreign: BTreeMap::new(),
            local_clock: now.clock(),
            start_ns: now.ns(),
            last_ns: now.ns(),
            incarnation: 0,
            key: Key::Listening,
            selection: Selection {
                role: Role::Listening,
                domain: None,
            },
            faulted: false,
        })
    }
    fn check(&mut self, now: LocalInstant) -> Result<(), Error> {
        if self.faulted {
            return Err(Error::Faulted);
        }
        let error = if now.clock() != self.local_clock {
            Some(Error::WrongClock)
        } else if now.ns() < self.last_ns {
            Some(Error::TimeReversed)
        } else {
            None
        };
        if let Some(error) = error {
            self.fail();
            return Err(error);
        }
        self.last_ns = now.ns();
        Ok(())
    }
    fn fail(&mut self) {
        self.faulted = true;
        self.key = Key::Listening;
        self.selection = Selection {
            role: Role::Listening,
            domain: None,
        };
    }
    /// Duplicates/out-of-order packets do not renew a lease. After lease expiry
    /// a restarted source must qualify again, even if it reuses its ClockID.
    pub fn announce(
        &mut self,
        peer: IpAddr,
        packet: &[u8],
        now: LocalInstant,
    ) -> Result<bool, Error> {
        self.check(now)?;
        if !self.peers.contains(&peer) {
            return Ok(false);
        }
        let Some((dataset, sequence)) = Dataset::parse(packet) else {
            return Ok(false);
        };
        if dataset.source_clock == self.own.source_clock
            || dataset.grandmaster == self.own.grandmaster
        {
            return Ok(false);
        }
        let old = self.foreign.get(&peer).copied();
        let continuous = old.is_some_and(|c| {
            now.ns() - c.last_ns <= self.config.announce_lease_ns
                && c.dataset.source_clock == dataset.source_clock
                && c.dataset.source_port == dataset.source_port
                && c.dataset.grandmaster == dataset.grandmaster
        });
        let (count, incarnation) = if continuous {
            let c = old.unwrap();
            let delta = sequence.wrapping_sub(c.sequence);
            if delta == 0 || delta >= 0x8000 {
                return Ok(false);
            }
            (
                if c.dataset == dataset {
                    c.count.saturating_add(1)
                } else {
                    1
                },
                c.incarnation,
            )
        } else {
            let Some(next) = self.incarnation.checked_add(1) else {
                self.fail();
                return Err(Error::Exhausted);
            };
            self.incarnation = next;
            (1, next)
        };
        self.foreign.insert(
            peer,
            Candidate {
                dataset,
                sequence,
                last_ns: now.ns(),
                count,
                incarnation,
            },
        );
        Ok(true)
    }
    pub fn set_peers(&mut self, peers: &[IpAddr], now: LocalInstant) -> Result<Selection, Error> {
        self.check(now)?;
        let next = validate_peers(peers)?;
        self.foreign.retain(|ip, _| next.contains(ip));
        self.peers = next;
        self.tick(now)
    }
    pub fn tick(&mut self, now: LocalInstant) -> Result<Selection, Error> {
        self.check(now)?;
        self.foreign
            .retain(|_, c| now.ns() - c.last_ns <= self.config.announce_lease_ns);
        let key = if now.ns() - self.start_ns < self.config.settle_ns {
            Key::Listening
        } else {
            match self.config.policy {
                Policy::FollowOnly(peer) => self
                    .foreign
                    .get(&peer)
                    .filter(|c| c.count >= self.config.min_announces)
                    .map(|c| remote_key(peer, *c))
                    .unwrap_or(Key::Listening),
                Policy::Elect => self.elect(),
            }
        };
        self.select(key)
    }
    fn elect(&self) -> Key {
        let best = self
            .foreign
            .iter()
            .filter(|(_, c)| {
                c.dataset.grandmaster != self.own.grandmaster && c.dataset.rank() < self.own.rank()
            })
            .min_by_key(|(_, c)| c.dataset.rank());
        let Some((&peer, candidate)) = best else {
            return Key::Local;
        };
        // Wait for the BEST clock to qualify; a worse newcomer must not
        // interrupt a qualified, better master that we already follow.
        if candidate.count < self.config.min_announces {
            return Key::Listening;
        }
        if self.foreign.iter().any(|(&other, c)| {
            other != peer && c.dataset.grandmaster == candidate.dataset.grandmaster
        }) {
            return Key::Listening; // Full same-GM topology comparison is not implemented.
        }
        remote_key(peer, *candidate)
    }
    fn select(&mut self, key: Key) -> Result<Selection, Error> {
        if key == self.key {
            return Ok(self.selection);
        }
        let (role, clock) = match key {
            Key::Listening => (Role::Listening, None),
            Key::Local => (Role::Master, Some(self.own.grandmaster)),
            Key::Remote { peer, gm, .. } => (Role::Follower(peer), Some(gm)),
        };
        let domain = if let Some(id) = clock {
            let epoch = match next_epoch() {
                Ok(epoch) => epoch,
                Err(error) => {
                    self.fail();
                    return Err(error);
                }
            };
            Some(Domain::new(id, epoch).map_err(|_| Error::Invalid)?)
        } else {
            None
        };
        self.key = key;
        self.selection = Selection { role, domain };
        Ok(self.selection)
    }
    /// A clock step/lock loss reported by the sampler revokes the domain.
    /// Discard cached Announce evidence so the same source must qualify anew.
    pub fn invalidate(&mut self, now: LocalInstant) -> Result<Selection, Error> {
        self.check(now)?;
        self.foreign.clear();
        self.start_ns = now.ns();
        self.select(Key::Listening)
    }
    pub fn selection(&self) -> Selection {
        self.selection
    }
}
fn remote_key(peer: IpAddr, c: Candidate) -> Key {
    Key::Remote {
        peer,
        clock: c.dataset.source_clock,
        port: c.dataset.source_port,
        gm: c.dataset.grandmaster,
        incarnation: c.incarnation,
    }
}
fn validate_peers(peers: &[IpAddr]) -> Result<BTreeSet<IpAddr>, Error> {
    if peers.len() > 64 || peers.iter().any(|p| p.is_unspecified() || p.is_multicast()) {
        return Err(Error::Invalid);
    }
    Ok(peers.iter().copied().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer() -> IpAddr {
        "192.0.2.1".parse().unwrap()
    }
    fn local() -> LocalClock {
        LocalClock::new(1).unwrap()
    }
    fn dataset(clock: u64, priority: u8) -> Dataset {
        Dataset {
            priority1: priority,
            clock_class: 6,
            accuracy: 0x21,
            variance: 0x436a,
            priority2: 128,
            grandmaster: clock,
            steps_removed: 0,
            source_clock: clock,
            source_port: 1,
        }
    }
    fn packet(d: Dataset, seq: u16) -> Vec<u8> {
        let mut b = vec![0; 64];
        b[0] = 0x1b;
        b[1] = 2;
        b[2..4].copy_from_slice(&64u16.to_be_bytes());
        b[20..28].copy_from_slice(&d.source_clock.to_be_bytes());
        b[28..30].copy_from_slice(&d.source_port.to_be_bytes());
        b[30..32].copy_from_slice(&seq.to_be_bytes());
        b[47] = d.priority1;
        b[48] = d.clock_class;
        b[49] = d.accuracy;
        b[50..52].copy_from_slice(&d.variance.to_be_bytes());
        b[52] = d.priority2;
        b[53..61].copy_from_slice(&d.grandmaster.to_be_bytes());
        b[61..63].copy_from_slice(&d.steps_removed.to_be_bytes());
        b
    }
    fn controller(policy: Policy) -> Controller {
        Controller::new(
            dataset(10, 128),
            Config {
                policy,
                settle_ns: 10,
                announce_lease_ns: 100,
                min_announces: 2,
            },
            &[peer()],
            local().at(0),
        )
        .unwrap()
    }
    #[test]
    fn settles_and_qualifies_before_transmitting_or_following() {
        let mut c = controller(Policy::Elect);
        assert_eq!(c.tick(local().at(9)).unwrap().role, Role::Listening);
        c.announce(peer(), &packet(dataset(20, 100), 1), local().at(10))
            .unwrap();
        assert_eq!(c.tick(local().at(10)).unwrap().role, Role::Listening);
        c.announce(peer(), &packet(dataset(20, 100), 2), local().at(20))
            .unwrap();
        let selected = c.tick(local().at(20)).unwrap();
        assert_eq!(selected.role, Role::Follower(peer()));
        assert_eq!(selected.domain.unwrap().id(), 20);
    }
    #[test]
    fn replay_does_not_renew_lease_and_follow_only_never_self_promotes() {
        let mut c = controller(Policy::FollowOnly(peer()));
        for (n, t) in [(1, 10), (2, 20)] {
            c.announce(peer(), &packet(dataset(20, 200), n), local().at(t))
                .unwrap();
        }
        let first = c.tick(local().at(20)).unwrap().domain.unwrap();
        assert!(
            !c.announce(peer(), &packet(dataset(20, 200), 2), local().at(110))
                .unwrap()
        );
        assert_eq!(c.tick(local().at(121)).unwrap().role, Role::Listening);
        for (n, t) in [(1, 130), (2, 140)] {
            c.announce(peer(), &packet(dataset(20, 200), n), local().at(t))
                .unwrap();
        }
        let restarted = c.tick(local().at(140)).unwrap().domain.unwrap();
        assert_eq!(first.id(), restarted.id());
        assert!(restarted.epoch() > first.epoch());
    }
    #[test]
    fn local_wins_by_quality_then_yields_and_reacquires_with_new_epoch() {
        let mut c = controller(Policy::Elect);
        let first = c.tick(local().at(10)).unwrap();
        assert_eq!(first.role, Role::Master);
        for (n, t) in [(1, 20), (2, 30)] {
            c.announce(peer(), &packet(dataset(20, 100), n), local().at(t))
                .unwrap();
        }
        assert_eq!(c.tick(local().at(30)).unwrap().role, Role::Follower(peer()));
        let next = c.tick(local().at(131)).unwrap();
        assert_eq!(next.role, Role::Master);
        assert!(next.domain.unwrap().epoch() > first.domain.unwrap().epoch());
    }
    #[test]
    fn sequence_wrap_is_valid_but_old_sequences_are_not_new_evidence() {
        let mut c = controller(Policy::FollowOnly(peer()));
        assert!(
            c.announce(peer(), &packet(dataset(20, 100), 65535), local().at(10))
                .unwrap()
        );
        assert!(
            c.announce(peer(), &packet(dataset(20, 100), 0), local().at(20))
                .unwrap()
        );
        assert!(
            !c.announce(peer(), &packet(dataset(20, 100), 65534), local().at(30))
                .unwrap()
        );
        assert_eq!(c.tick(local().at(30)).unwrap().role, Role::Follower(peer()));
    }
    #[test]
    fn same_grandmaster_multiple_paths_are_not_misrepresented_as_complete_bmca() {
        let other = "192.0.2.2".parse().unwrap();
        let mut c = controller(Policy::Elect);
        c.set_peers(&[peer(), other], local().at(0)).unwrap();
        for n in 1..=2 {
            for ip in [peer(), other] {
                let mut d = dataset(20, 100);
                if ip == other {
                    d.source_clock = 21;
                    d.steps_removed = 1;
                }
                c.announce(ip, &packet(d, n), local().at(n as u64 * 10))
                    .unwrap();
            }
        }
        assert_eq!(c.tick(local().at(20)).unwrap().role, Role::Listening);
        c.set_peers(&[peer()], local().at(21)).unwrap();
        assert_eq!(c.selection().role, Role::Follower(peer()));
    }
    #[test]
    fn invalidation_and_wrong_local_clock_revoke_previous_domain() {
        let mut c = controller(Policy::Elect);
        let first = c.tick(local().at(10)).unwrap().domain.unwrap();
        c.invalidate(local().at(11)).unwrap();
        assert_eq!(c.selection().domain, None);
        let new = c.tick(local().at(21)).unwrap().domain.unwrap();
        assert!(new.epoch() > first.epoch());
        assert_eq!(
            c.tick(LocalClock::new(2).unwrap().at(22)),
            Err(Error::WrongClock)
        );
        assert_eq!(c.selection().domain, None);
        assert_eq!(c.tick(local().at(23)), Err(Error::Faulted));
    }
    #[test]
    fn malformed_unknown_and_self_announcements_are_ignored() {
        let mut c = controller(Policy::Elect);
        let mut b = packet(dataset(20, 100), 1);
        b[2] = 255;
        assert!(!c.announce(peer(), &b, local().at(1)).unwrap());
        assert!(
            !c.announce(
                "192.0.2.8".parse().unwrap(),
                &packet(dataset(20, 100), 1),
                local().at(2)
            )
            .unwrap()
        );
        assert!(
            !c.announce(peer(), &packet(dataset(10, 128), 1), local().at(3))
                .unwrap()
        );
        assert_eq!(c.tick(local().at(10)).unwrap().role, Role::Master);
    }
    #[test]
    fn following_our_own_grandmaster_through_a_peer_is_not_allowed() {
        let mut c = controller(Policy::FollowOnly(peer()));
        let mut d = dataset(20, 100);
        d.grandmaster = 10;
        assert!(!c.announce(peer(), &packet(d, 1), local().at(10)).unwrap());
        assert_eq!(c.tick(local().at(20)).unwrap().role, Role::Listening);
    }
    #[test]
    fn inferior_unqualified_newcomer_does_not_reset_selected_clock() {
        let other = "192.0.2.2".parse().unwrap();
        let mut c = controller(Policy::Elect);
        c.set_peers(&[peer(), other], local().at(0)).unwrap();
        for (n, t) in [(1, 10), (2, 20)] {
            c.announce(peer(), &packet(dataset(20, 10), n), local().at(t))
                .unwrap();
        }
        let chosen = c.tick(local().at(20)).unwrap();
        c.announce(other, &packet(dataset(30, 100), 1), local().at(21))
            .unwrap();
        assert_eq!(c.tick(local().at(21)).unwrap(), chosen);
    }
}
