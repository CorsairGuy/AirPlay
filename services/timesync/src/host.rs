//! Host adapter for the existing unified role engine. One explicit UDP owner.
use crate::{engine::Snapshot, sender, service};
use clock::{
    domain::{Domain, LocalClock, LocalInstant, Reading},
    follower, role,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    net::IpAddr,
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;

struct Physical {
    local: LocalClock,
    own: Domain,
}
impl sender::ClockSource for Physical {
    fn sample(&self) -> io::Result<(LocalInstant, Reading)> {
        let now = clock::now_ns();
        Ok((self.local.at(now), self.own.reading(now)))
    }
}
pub(crate) struct Host {
    _service: Arc<service::Service>,
    status: watch::Receiver<Snapshot>,
    local: LocalClock,
    dual_source: Option<Arc<crate::media::InstantSource>>,
    peers: watch::Sender<BTreeMap<u128, Vec<IpAddr>>>,
}
impl Host {
    pub fn service(&self) -> Arc<service::Service> {
        self._service.clone()
    }
    pub async fn bind(id: u64, ip: IpAddr) -> io::Result<Self> {
        Self::bind_mode(
            id,
            ip,
            false,
            sender::Ports {
                event: 319,
                general: 320,
            },
        )
        .await
    }

    /// Same socket pair with an explicit co-resident outbound master: this host
    /// can follow an inbound source and serve its own receivers at once.
    pub async fn bind_dual(id: u64, ip: IpAddr) -> io::Result<Self> {
        Self::bind_dual_with_ports(
            id,
            ip,
            sender::Ports {
                event: 319,
                general: 320,
            },
        )
        .await
    }

    pub async fn bind_dual_with_ports(
        id: u64,
        ip: IpAddr,
        ports: sender::Ports,
    ) -> io::Result<Self> {
        Self::bind_mode(id, ip, true, ports).await
    }

    async fn bind_mode(id: u64, ip: IpAddr, dual: bool, ports: sender::Ports) -> io::Result<Self> {
        let own = Domain::new(id, 1).map_err(io::Error::other)?;
        let local = LocalClock::new(u128::from(id)).map_err(io::Error::other)?;
        let dual_source = dual.then(|| {
            Arc::new(crate::media::InstantSource::new(
                std::time::Instant::now(),
                local,
                own,
                clock::now_ns(),
            ))
        });
        let clock: Arc<dyn sender::ClockSource> = match &dual_source {
            Some(source) => source.clone(),
            None => Arc::new(Physical { local, own }),
        };
        let config = service::Config {
            network: sender::Config {
                domain: own,
                bind_ip: ip,
                ports,
                peers: vec![],
            },
            election: role::Config {
                policy: role::Policy::Elect,
                settle_ns: 200_000_000,
                announce_lease_ns: 3_000_000_000,
                min_announces: 2,
            },
            follow: follower::Config {
                pairing_lease_ns: 1_000_000_000,
                mapping_lease_ns: 2_000_000_000,
                warmup_samples: 3,
                step_threshold_ns: 50_000_000,
            },
            probe: None,
        };
        let service = Arc::new(if dual {
            service::Service::bind_dual(config, clock).await?
        } else {
            service::Service::bind(config, clock).await?
        });
        let mut status = service.subscribe();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if status.borrow_and_update().mapping.is_some() {
                    return Ok::<_, io::Error>(());
                }
                status.changed().await.map_err(io::Error::other)?;
            }
        })
        .await
        .map_err(io::Error::other)??;
        let (peers, mut requests) = watch::channel(BTreeMap::<u128, Vec<IpAddr>>::new());
        let worker = service.clone();
        tokio::spawn(async move {
            let mut applied = BTreeMap::<u128, Vec<IpAddr>>::new();
            let mut retry = false;
            loop {
                tokio::select! {
                    changed = requests.changed() => if changed.is_err() { break; },
                    _ = tokio::time::sleep(Duration::from_millis(100)), if retry => {},
                }
                let desired = requests.borrow_and_update().clone();
                let owners = applied
                    .keys()
                    .chain(desired.keys())
                    .copied()
                    .collect::<BTreeSet<_>>();
                retry = false;
                for owner in owners {
                    let addresses = desired
                        .get(&owner)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|a| a.to_canonical())
                        .filter(|a| *a != ip && !a.is_loopback() && a.is_ipv4() == ip.is_ipv4())
                        .collect::<std::collections::BTreeSet<_>>();
                    let addresses: Vec<_> = addresses.into_iter().collect();
                    if applied.get(&owner).map(Vec::as_slice).unwrap_or_default() == addresses {
                        continue;
                    }
                    let peers: Vec<_> = addresses
                        .iter()
                        .copied()
                        .map(|ip| sender::Peer {
                            ip,
                            ports: sender::Ports {
                                event: 319,
                                general: 320,
                            },
                        })
                        .collect();
                    let result = if owner == 0 {
                        worker.set_peers(peers).await
                    } else {
                        worker.set_session_peers(owner, peers).await
                    };
                    match result {
                        Ok(()) if addresses.is_empty() => {
                            applied.remove(&owner);
                        }
                        Ok(()) => {
                            applied.insert(owner, addresses);
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotConnected => return,
                        Err(error) => {
                            tracing::warn!(%error,"PTP peer update failed; retrying");
                            retry = true;
                        }
                    }
                }
            }
        });
        tracing::info!(clock_id=id,%ip,"PTP: unified host ready");
        Ok(Self {
            _service: service,
            status,
            local,
            dual_source,
            peers,
        })
    }
    pub fn set_peers(&self, owner: u128, peers: Vec<IpAddr>) {
        self.peers.send_modify(|desired| {
            if peers.is_empty() {
                desired.remove(&owner);
            } else {
                desired.insert(owner, peers);
            }
        });
    }
    pub fn reading(&self) -> Option<Reading> {
        let snapshot = *self.status.borrow();
        let now = self.local_now()?;
        snapshot.mapping?.to_network(now, now).ok()
    }

    /// Clock we offer receivers, valid while following an inbound source.
    pub fn outbound_reading(&self) -> Option<Reading> {
        let snapshot = *self.status.borrow();
        let now = self.local_now()?;
        snapshot.outbound?.to_network(now, now).ok()
    }

    fn local_now(&self) -> Option<LocalInstant> {
        match &self.dual_source {
            Some(source) => source.local_at(std::time::Instant::now()).ok(),
            None => Some(self.local.at(clock::now_ns())),
        }
    }

    pub fn dual_source(&self) -> Option<Arc<crate::media::InstantSource>> {
        self.dual_source.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn retries_session_peer_after_outbound_conflict_clears() {
        let route = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        if route.connect("192.0.2.42:319").is_err() {
            return;
        }
        let local = route.local_addr().unwrap().ip();
        if local.is_loopback() || local.is_unspecified() {
            return;
        }
        let host = Host::bind_dual_with_ports(
            42,
            local,
            sender::Ports {
                event: 0,
                general: 0,
            },
        )
        .await
        .unwrap();
        let service = host.service();
        let address = "192.0.2.42".parse().unwrap();
        let peer = sender::Peer {
            ip: address,
            ports: sender::Ports {
                event: 319,
                general: 320,
            },
        };
        service.set_group_peers(1, vec![peer]).await.unwrap();
        host.set_peers(2, vec![address]);
        tokio::time::sleep(Duration::from_millis(250)).await;
        service.set_group_peers(1, vec![]).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if service.set_group_peers(3, vec![peer]).await.is_err() {
                    break;
                }
                service.set_group_peers(3, vec![]).await.unwrap();
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .expect("session peer must be retried after conflict");
    }
}
