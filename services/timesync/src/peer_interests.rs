//! Shared, pure peer-interest planning for native and external clock owners.
//! No socket, daemon command or ownership mutation occurs here. A serialized
//! caller must apply the planned union before committing the returned owners.
use crate::sender::{Peer, Ports};
use std::{collections::BTreeMap, io, net::IpAddr, time::Instant};

/// Prepare the union before mutating engine or ownership. One group's removal
/// must not withdraw another group's shared peer; port conflicts fail closed.
pub type PeerUnion = (BTreeMap<u128, Vec<Peer>>, BTreeMap<IpAddr, Ports>);
pub fn peer_union(
    bind: IpAddr,
    owners: &BTreeMap<u128, Vec<Peer>>,
    owner: u128,
    peers: Vec<Peer>,
) -> io::Result<PeerUnion> {
    peer_table(bind, &peers)?;
    let mut next = owners.clone();
    if peers.is_empty() {
        next.remove(&owner);
    } else {
        next.insert(owner, peers);
    }
    if next.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many timing consumers",
        ));
    }
    let mut union: BTreeMap<IpAddr, Ports> = BTreeMap::new();
    for peer in next.values().flatten() {
        if let Some(old) = union.insert(peer.ip, peer.ports) {
            if old.event != peer.ports.event || old.general != peer.ports.general {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "timing peer port conflict",
                ));
            }
        }
    }
    if union.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many aggregate timing peers",
        ));
    }
    Ok((next, union))
}
/// One committed aggregate view of every owner's interests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Aggregate {
    union: BTreeMap<IpAddr, Ports>,
    generation: u64,
}

impl Aggregate {
    pub fn union(&self) -> &BTreeMap<IpAddr, Ports> {
        &self.union
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Sorted peer list for the engine/sender; never a per-owner view.
    pub fn peers(&self) -> Vec<Peer> {
        self.union
            .iter()
            .map(|(ip, ports)| Peer {
                ip: *ip,
                ports: *ports,
            })
            .collect()
    }
}

/// Ownership of the shared timing peer set, with one optional deadline per
/// owner. A silent owner expires exactly like an explicit release, so a crashed
/// consumer cannot pin the aggregate forever. Every update is planned and
/// validated before anything is committed, and callers only ever see complete
/// [`Aggregate`] snapshots — never a partially applied owner change.
#[derive(Clone)]
pub struct OwnerInterests {
    bind: IpAddr,
    owners: BTreeMap<u128, Owner>,
    union: BTreeMap<IpAddr, Ports>,
    generation: u64,
}

#[derive(Clone)]
struct Owner {
    peers: Vec<Peer>,
    deadline: Option<Instant>,
}

impl OwnerInterests {
    pub fn new(bind: IpAddr) -> Self {
        Self {
            bind,
            owners: BTreeMap::new(),
            union: BTreeMap::new(),
            generation: 0,
        }
    }

    pub fn union(&self) -> &BTreeMap<IpAddr, Ports> {
        &self.union
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn owner_count(&self) -> usize {
        self.owners.len()
    }

    /// Replace one owner's peers and lease. An empty list releases the owner.
    /// A conflict, an invalid peer or an unsupported address changes nothing.
    /// `Ok(None)` means the aggregate union did not change, so no engine update
    /// is needed even though a lease may have been extended.
    pub fn set(
        &mut self,
        owner: u128,
        peers: Vec<Peer>,
        deadline: Option<Instant>,
    ) -> io::Result<Option<Aggregate>> {
        // Owner zero is the service's own base peer list, not a group token.
        let current: BTreeMap<u128, Vec<Peer>> = self
            .owners
            .iter()
            .map(|(id, value)| (*id, value.peers.clone()))
            .collect();
        let (planned, union) = peer_union(self.bind, &current, owner, peers)?;
        let mut next = BTreeMap::new();
        for (id, peers) in planned {
            let deadline = if id == owner {
                deadline
            } else {
                self.owners[&id].deadline
            };
            next.insert(id, Owner { peers, deadline });
        }
        self.owners = next;
        Ok(self.commit(union))
    }

    /// Release one owner without touching anybody else's peers.
    pub fn release(&mut self, owner: u128) -> Option<Aggregate> {
        self.set(owner, Vec::new(), None).ok().flatten()
    }

    /// Drop owners whose deadline has passed (inclusive). A deadline only
    /// ever shortens; extending it requires a new [`Self::set`].
    pub fn expire(&mut self, now: Instant) -> Option<Aggregate> {
        let expired: Vec<u128> = self
            .owners
            .iter()
            .filter(|(_, owner)| owner.deadline.is_some_and(|deadline| deadline <= now))
            .map(|(id, _)| *id)
            .collect();
        if expired.is_empty() {
            return None;
        }
        for id in expired {
            self.owners.remove(&id);
        }
        let mut union = BTreeMap::new();
        for peer in self.owners.values().flat_map(|owner| &owner.peers) {
            union.insert(peer.ip, peer.ports);
        }
        self.commit(union)
    }

    fn commit(&mut self, union: BTreeMap<IpAddr, Ports>) -> Option<Aggregate> {
        if union == self.union {
            return None;
        }
        self.union = union;
        self.generation = self.generation.wrapping_add(1);
        Some(Aggregate {
            union: self.union.clone(),
            generation: self.generation,
        })
    }
}

pub fn peer_table(bind: IpAddr, peers: &[Peer]) -> io::Result<BTreeMap<IpAddr, Ports>> {
    let mut table = BTreeMap::new();
    if peers.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many timing peers",
        ));
    }
    for peer in peers {
        if peer.ports.event == 0
            || peer.ports.general == 0
            || peer.ip.is_unspecified()
            || peer.ip.is_multicast()
            || matches!(peer.ip, IpAddr::V4(ip) if ip.is_broadcast())
            || peer.ip.is_ipv4() != bind.is_ipv4()
            || table.insert(peer.ip, peer.ports).is_some()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid/duplicate timing peer",
            ));
        }
    }
    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn bind() -> IpAddr {
        "192.0.2.1".parse().unwrap()
    }

    fn peer(last: u8) -> Peer {
        Peer {
            ip: format!("192.0.2.{last}").parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        }
    }

    #[test]
    fn removing_one_owner_keeps_a_shared_peer_for_the_other() {
        let mut owners = OwnerInterests::new(bind());
        let aggregate = owners.set(1, vec![peer(10), peer(11)], None).unwrap();
        assert_eq!(aggregate.unwrap().union().len(), 2);
        assert!(owners.set(2, vec![peer(11)], None).unwrap().is_none());
        assert_eq!(owners.owner_count(), 2);

        let after = owners.release(1).expect("shared peer remains");
        assert_eq!(after.union().len(), 1);
        assert!(after.union().contains_key(&peer(11).ip));
        assert!(owners.release(1).is_none(), "second release is a no-op");
        assert_eq!(owners.owner_count(), 1);
        assert!(owners.release(2).unwrap().union().is_empty());
        assert_eq!(owners.owner_count(), 0);
    }

    #[test]
    fn rejected_updates_leave_owners_and_union_untouched() {
        let mut owners = OwnerInterests::new(bind());
        owners.set(1, vec![peer(10)], None).unwrap();
        let generation = owners.generation();
        let before = owners.union().clone();

        let conflict = Peer {
            ports: Ports {
                event: 319,
                general: 321,
            },
            ..peer(10)
        };
        assert!(owners.set(2, vec![conflict], None).is_err());
        let multicast = Peer {
            ip: "224.0.0.1".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        assert!(owners.set(2, vec![multicast], None).is_err());
        assert_eq!(owners.generation(), generation);
        assert_eq!(owners.union(), &before);
        assert_eq!(owners.owner_count(), 1);
    }

    #[test]
    fn silent_owner_expires_like_an_explicit_release() {
        let now = Instant::now();
        let mut owners = OwnerInterests::new(bind());
        owners.set(1, vec![peer(10)], None).unwrap();
        owners
            .set(2, vec![peer(11)], Some(now + Duration::from_secs(1)))
            .unwrap();

        assert!(owners.expire(now).is_none(), "lease is still live");
        let after = owners
            .expire(now + Duration::from_secs(1))
            .expect("inclusive deadline");
        assert_eq!(after.union().len(), 1);
        assert!(after.union().contains_key(&peer(10).ip));
        assert_eq!(owners.owner_count(), 1);
        assert!(
            owners.expire(now + Duration::from_secs(5)).is_none(),
            "an owner without a deadline is not expired by time"
        );
    }

    #[test]
    fn keepalive_extends_a_lease_without_a_new_aggregate_generation() {
        let now = Instant::now();
        let mut owners = OwnerInterests::new(bind());
        owners
            .set(1, vec![peer(10)], Some(now + Duration::from_secs(1)))
            .unwrap();
        let generation = owners.generation();
        assert!(
            owners
                .set(1, vec![peer(10)], Some(now + Duration::from_secs(10)))
                .unwrap()
                .is_none(),
            "same union needs no engine update"
        );
        assert_eq!(owners.generation(), generation);
        assert!(owners.expire(now + Duration::from_secs(2)).is_none());
        assert!(owners.expire(now + Duration::from_secs(10)).is_some());
    }

    #[test]
    fn aggregate_peers_are_sorted_and_complete() {
        let mut owners = OwnerInterests::new(bind());
        let aggregate = owners
            .set(1, vec![peer(12), peer(10)], None)
            .unwrap()
            .unwrap();
        let peers = aggregate.peers();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].ip, peer(10).ip);
        assert_eq!(peers[1].ip, peer(12).ip);
        assert_eq!(peers[0].ports.event, 319);
        assert_eq!(owners.generation(), 1);
    }
}
