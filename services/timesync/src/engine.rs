//! One role controller and one selected mapping for RX/TX. No I/O here.
use clock::{
    domain::{ClockMap, Domain, LocalClock, LocalInstant, Reading},
    follower::{self, Follower},
    ptp_sender::{Datagram, Master},
    role::{self, Controller, Role, Selection},
};
use std::{io, net::IpAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Local,
    OneWay,
}
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub selection: Selection,
    pub mapping: Option<ClockMap>,
    pub quality: Option<Quality>,
}
impl Snapshot {
    pub fn inactive() -> Self {
        Self {
            selection: Selection {
                role: Role::Listening,
                domain: None,
            },
            mapping: None,
            quality: None,
        }
    }
}
#[derive(Clone, Copy, Debug)]
enum Origin {
    Master,
    Link,
}
#[derive(Clone, Debug)]
pub struct Emission {
    pub datagram: Datagram,
    origin: Origin,
}
pub struct Engine {
    own: Domain,
    local: LocalClock,
    peers: Vec<IpAddr>,
    controller: Controller,
    follow_config: follower::Config,
    master: Option<Master>,
    link: Master,
    follower: Option<Follower>,
    snapshot: Snapshot,
    had_mapping: bool,
    faulted: bool,
    physical_sample: Option<(u64, u64)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clock::{domain::LocalClock, ptp_sender::Master};
    fn local() -> LocalClock {
        LocalClock::new(1).unwrap()
    }
    fn own(ns: u64) -> Reading {
        Domain::new(100, 1).unwrap().reading(ns + 10_000_000_000)
    }
    fn peer() -> IpAddr {
        "192.0.2.1".parse().unwrap()
    }
    fn engine() -> Engine {
        Engine::new(
            Domain::new(100, 1).unwrap(),
            local().at(0),
            &[peer()],
            role::Config {
                policy: role::Policy::Elect,
                settle_ns: 10,
                announce_lease_ns: 10_000_000_000,
                min_announces: 2,
            },
            follower::Config {
                pairing_lease_ns: 100_000_000,
                mapping_lease_ns: 2_000_000_000,
                warmup_samples: 2,
                step_threshold_ns: 50_000_000,
            },
        )
        .unwrap()
    }
    fn remote() -> Master {
        let mut m = Master::new(Domain::new(50, 1).unwrap()).unwrap();
        m.set_peers(&[peer()]).unwrap();
        m
    }
    fn deliver(e: &mut Engine, m: &mut Master, n: u64, at: u64) {
        for d in m.poll(n * 1_000_000_000).unwrap() {
            e.receive(peer(), &d.bytes, local().at(at), own(at))
                .unwrap();
            if let Some(token) = d.completion {
                let f = m
                    .complete_send(
                        &token,
                        Domain::new(50, 1).unwrap().reading(at + 20_000_000_000),
                    )
                    .unwrap();
                e.receive(peer(), &f.bytes, local().at(at + 10), own(at + 10))
                    .unwrap();
            }
        }
    }
    #[test]
    fn one_engine_yields_revokes_pending_master_and_reacquires_a_new_epoch() {
        let mut e = engine();
        let mut m = remote();
        let batch = e.tick(local().at(10), own(10)).unwrap();
        let old = batch
            .into_iter()
            .find(|d| d.datagram.completion.is_some())
            .unwrap();
        let first = e.snapshot().selection.domain.unwrap();
        deliver(&mut e, &mut m, 0, 1_000_000_000);
        deliver(&mut e, &mut m, 1, 2_000_000_000);
        deliver(&mut e, &mut m, 2, 3_000_000_000);
        let s = e.snapshot();
        assert_eq!(s.selection.role, Role::Follower(peer()));
        assert_eq!(s.quality, Some(Quality::OneWay));
        assert!(s.mapping.is_some());
        assert_ne!(s.selection.domain, Some(first));
        assert!(
            e.complete(&old, local().at(3_000_000_100), own(3_000_000_100))
                .unwrap()
                .is_none()
        );
        assert!(
            e.tick(local().at(3_000_000_101), own(3_000_000_101))
                .unwrap()
                .is_empty()
        );
        e.tick(local().at(5_000_000_000), own(5_000_000_000))
            .unwrap(); // Exact lease boundary.
        e.tick(local().at(5_000_000_001), own(5_000_000_001))
            .unwrap();
        assert_eq!(e.snapshot().selection.role, Role::Follower(peer()));
        assert!(e.snapshot().mapping.is_none());
        assert_ne!(e.snapshot().selection.domain, s.selection.domain);
        assert!(
            e.tick(local().at(6_000_000_000), own(6_000_000_000))
                .unwrap()
                .is_empty()
        );
        e.tick(local().at(13_000_000_001), own(13_000_000_001))
            .unwrap();
        assert_eq!(e.snapshot().selection.role, Role::Master);
        assert!(e.snapshot().selection.domain.unwrap().epoch() > first.epoch());
    }
    #[test]
    fn changing_physical_epoch_revokes_snapshot_and_engine_cannot_resume_old_domain() {
        let mut e = engine();
        e.tick(local().at(10), own(10)).unwrap();
        assert!(
            e.tick(local().at(11), Domain::new(100, 2).unwrap().reading(0))
                .is_err()
        );
        assert!(e.snapshot().mapping.is_none());
        assert!(e.tick(local().at(12), own(12)).is_err());
    }
    #[test]
    fn unreported_physical_step_is_also_a_fault_without_relabeling_old_time() {
        let mut e = engine();
        e.tick(local().at(10), own(10)).unwrap();
        assert!(
            e.tick(
                local().at(11),
                Domain::new(100, 1).unwrap().reading(20_000_000_000)
            )
            .is_err()
        );
        assert!(e.snapshot().mapping.is_none());
        assert!(e.tick(local().at(12), own(12)).is_err());
    }
}
impl Engine {
    pub fn new(
        own: Domain,
        now: LocalInstant,
        peers: &[IpAddr],
        election: role::Config,
        follow: follower::Config,
    ) -> io::Result<Self> {
        // Validate follower policy before opening network sockets or selecting a role.
        let mut link = Master::new(own).map_err(io::Error::other)?;
        link.set_peers(peers).map_err(io::Error::other)?;
        let dataset = link.dataset();
        follow.validate().map_err(io::Error::other)?;
        let controller =
            Controller::new(dataset, election, peers, now).map_err(io::Error::other)?;
        Ok(Self {
            own,
            local: now.clock(),
            peers: peers.to_vec(),
            controller,
            follow_config: follow,
            master: None,
            link,
            follower: None,
            snapshot: Snapshot::inactive(),
            had_mapping: false,
            faulted: false,
            physical_sample: None,
        })
    }
    fn fault(&mut self) {
        self.faulted = true;
        self.master = None;
        self.follower = None;
        self.snapshot = Snapshot::inactive();
    }
    fn validate(&mut self, now: LocalInstant, own: Reading) -> io::Result<()> {
        if self.faulted || now.clock() != self.local || own.domain() != self.own {
            self.fault();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared physical clock changed or engine faulted",
            ));
        }
        if let Some((old_local, old_own)) = self.physical_sample {
            let local_delta = i128::from(now.ns()) - i128::from(old_local);
            let own_delta = i128::from(own.ns()) - i128::from(old_own);
            if local_delta < 0
                || own_delta < 0
                || (own_delta - local_delta).abs()
                    > i128::from(self.follow_config.step_threshold_ns)
            {
                self.fault();
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "physical clock discontinuity",
                ));
            }
        }
        self.physical_sample = Some((now.ns(), own.ns()));
        Ok(())
    }
    fn choose(&mut self, now: LocalInstant) -> io::Result<()> {
        let next = match self.controller.tick(now) {
            Ok(s) => s,
            Err(e) => {
                self.fault();
                return Err(io::Error::other(e));
            }
        };
        self.transition(next)
    }
    fn transition(&mut self, next: Selection) -> io::Result<()> {
        if self.snapshot.selection == next {
            return Ok(());
        }
        self.master = None;
        self.follower = None;
        self.had_mapping = false;
        self.snapshot = Snapshot {
            selection: next,
            mapping: None,
            quality: None,
        };
        match next.role {
            Role::Master => {
                let mut master = Master::new(next.domain.unwrap()).map_err(io::Error::other)?;
                master.set_peers(&self.peers).map_err(io::Error::other)?;
                self.master = Some(master);
            }
            Role::Follower(peer) => {
                let dataset = self.controller.selected_dataset().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "selected source absent")
                })?;
                self.follower = Some(
                    Follower::new(
                        peer,
                        dataset,
                        next.domain.unwrap(),
                        self.local,
                        self.follow_config,
                    )
                    .map_err(io::Error::other)?,
                );
            }
            Role::Listening => {}
        }
        Ok(())
    }
    fn invalidate(&mut self, now: LocalInstant) -> io::Result<()> {
        let next = self
            .controller
            .renew_mapping_epoch(now)
            .map_err(io::Error::other)?;
        self.transition(next)
    }
    fn refresh(&mut self, now: LocalInstant, own: Reading) -> io::Result<()> {
        match self.snapshot.selection.role {
            Role::Master => {
                // Same live physical clock, newly selected authority epoch. Do
                // not relabel foreign samples or a changed physical epoch.
                let domain = self.snapshot.selection.domain.unwrap();
                self.snapshot.mapping = Some(
                    ClockMap::new(
                        now,
                        domain.reading(own.ns()),
                        0,
                        self.follow_config.mapping_lease_ns,
                    )
                    .map_err(io::Error::other)?,
                );
                self.snapshot.quality = Some(Quality::Local);
            }
            Role::Follower(_) => {
                let map = self.follower.as_ref().and_then(|f| f.mapping(now));
                if self.had_mapping && map.is_none() {
                    self.invalidate(now)?;
                    return Ok(());
                }
                self.had_mapping |= map.is_some();
                self.snapshot.mapping = map;
                self.snapshot.quality = map.map(|_| Quality::OneWay);
            }
            Role::Listening => {
                self.snapshot.mapping = None;
                self.snapshot.quality = None;
            }
        }
        Ok(())
    }
    pub fn tick(&mut self, now: LocalInstant, own: Reading) -> io::Result<Vec<Emission>> {
        self.validate(now, own)?;
        self.choose(now)?;
        self.refresh(now, own)?;
        Ok(if let Some(master) = self.master.as_mut() {
            master
                .poll(now.ns())
                .map_err(io::Error::other)?
                .into_iter()
                .map(|datagram| Emission {
                    datagram,
                    origin: Origin::Master,
                })
                .collect()
        } else {
            Vec::new()
        })
    }
    pub fn receive(
        &mut self,
        peer: IpAddr,
        packet: &[u8],
        now: LocalInstant,
        own: Reading,
    ) -> io::Result<Vec<Emission>> {
        self.validate(now, own)?;
        let kind = packet.first().map(|b| b & 15);
        if kind == Some(0xb) {
            if let Err(error) = self.controller.announce(peer, packet, now) {
                self.fault();
                return Err(io::Error::other(error));
            }
        }
        self.choose(now)?;
        self.refresh(now, own)?;
        if let Some(follower) = self.follower.as_mut() {
            match follower
                .receive(peer, packet, now)
                .map_err(io::Error::other)?
            {
                follower::Event::Step => {
                    self.invalidate(now)?;
                    return Ok(Vec::new());
                }
                _ => {}
            }
            self.refresh(now, own)?;
        }
        if kind == Some(2) {
            return Ok(self
                .link
                .receive_peer_delay(peer, packet, own)
                .into_iter()
                .map(|datagram| Emission {
                    datagram,
                    origin: Origin::Link,
                })
                .collect());
        }
        if matches!(kind, Some(1 | 0xc)) {
            if let Some(master) = self.master.as_mut() {
                let reading = self.snapshot.selection.domain.unwrap().reading(own.ns());
                return Ok(master
                    .receive(peer, packet, reading)
                    .into_iter()
                    .map(|datagram| Emission {
                        datagram,
                        origin: Origin::Master,
                    })
                    .collect());
            }
        }
        Ok(Vec::new())
    }
    pub fn complete(
        &mut self,
        emission: &Emission,
        now: LocalInstant,
        own: Reading,
    ) -> io::Result<Option<Emission>> {
        self.validate(now, own)?;
        self.choose(now)?;
        self.refresh(now, own)?;
        let Some(token) = emission.datagram.completion.as_ref() else {
            return Ok(None);
        };
        let packet = match emission.origin {
            Origin::Master => self.master.as_mut().and_then(|m| {
                m.complete_send(
                    token,
                    self.snapshot.selection.domain.unwrap().reading(own.ns()),
                )
            }),
            Origin::Link => self.link.complete_send(token, own),
        };
        Ok(packet.map(|datagram| Emission {
            datagram,
            origin: emission.origin,
        }))
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
    }
}
