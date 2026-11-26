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
    peers: watch::Sender<BTreeMap<u128, Vec<IpAddr>>>,
}
impl Host {
    pub fn service(&self) -> Arc<service::Service> {
        self._service.clone()
    }
    pub async fn bind(id: u64, ip: IpAddr) -> io::Result<Self> {
        Self::bind_mode(id, ip, false).await
    }

    /// Same socket pair with an explicit co-resident outbound master: this host
    /// can follow an inbound source and serve its own receivers at once.
    pub async fn bind_dual(id: u64, ip: IpAddr) -> io::Result<Self> {
        Self::bind_mode(id, ip, true).await
    }

    async fn bind_mode(id: u64, ip: IpAddr, dual: bool) -> io::Result<Self> {
        let own = Domain::new(id, 1).map_err(io::Error::other)?;
        let local = LocalClock::new(u128::from(id)).map_err(io::Error::other)?;
        let clock = Arc::new(Physical { local, own });
        let config = service::Config {
            network: sender::Config {
                domain: own,
                bind_ip: ip,
                ports: sender::Ports {
                    event: 319,
                    general: 320,
                },
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
            let mut applied = BTreeSet::new();
            while requests.changed().await.is_ok() {
                let desired = requests.borrow_and_update().clone();
                let owners = applied
                    .union(&desired.keys().copied().collect())
                    .copied()
                    .collect::<Vec<_>>();
                for owner in owners {
                    let addresses = desired
                        .get(&owner)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|a| a.to_canonical())
                        .filter(|a| *a != ip && !a.is_loopback() && a.is_ipv4() == ip.is_ipv4())
                        .collect::<std::collections::BTreeSet<_>>();
                    let peers: Vec<_> = addresses
                        .into_iter()
                        .map(|ip| sender::Peer {
                            ip,
                            ports: sender::Ports {
                                event: 319,
                                general: 320,
                            },
                        })
                        .collect();
                    loop {
                        let result = if owner == 0 {
                            worker.set_peers(peers.clone()).await
                        } else {
                            worker.set_session_peers(owner, peers.clone()).await
                        };
                        match result {
                            Ok(()) => break,
                            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                                tokio::time::sleep(Duration::from_millis(10)).await
                            }
                            Err(error) => {
                                tracing::warn!(%error,"PTP peer update failed");
                                break;
                            }
                        }
                    }
                }
                applied = desired.keys().copied().collect();
            }
        });
        tracing::info!(clock_id=id,%ip,"PTP: unified host ready");
        Ok(Self {
            _service: service,
            status,
            local,
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
        let now = self.local.at(clock::now_ns());
        snapshot.mapping?.to_network(now, now).ok()
    }

    /// Clock we offer receivers, valid while following an inbound source.
    pub fn outbound_reading(&self) -> Option<Reading> {
        let snapshot = *self.status.borrow();
        let now = self.local.at(clock::now_ns());
        snapshot.outbound?.to_network(now, now).ok()
    }
}
