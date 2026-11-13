//! Shared, pure peer-interest planning for native and external clock owners.
//! No socket, daemon command or ownership mutation occurs here. A serialized
//! caller must apply the planned union before committing the returned owners.
use crate::sender::{Peer, Ports};
use std::{collections::BTreeMap, io, net::IpAddr};

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
