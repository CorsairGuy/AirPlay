//! Experimental unified unicast RX/TX loop. Explicit bind only; no CLI wiring.
//! Exactly one socket pair and one injected physical ClockSource serve all
//! selected roles. Snapshot mappings are local/one-way estimates, NOT lock or
//! acoustic readiness. No multicast or complete BMCA topology implementation.
use crate::{
    engine::{Emission, Engine, Snapshot},
    sender::{ClockSource, Config as NetworkConfig, Peer, Ports, Source},
};
use clock::{follower, probe, role};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::{MissedTickBehavior, interval, timeout},
};

pub struct Config {
    pub network: NetworkConfig,
    pub election: role::Config,
    pub follow: follower::Config,
    pub probe: Option<probe::Config>,
}
pub struct Service {
    addresses: (SocketAddr, SocketAddr),
    status: watch::Sender<Snapshot>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<io::Result<()>>>,
    peers: mpsc::Sender<PeerRequest>,
}
struct PeerRequest {
    owner: u128,
    peers: Vec<Peer>,
    reply: oneshot::Sender<io::Result<()>>,
}
/// Prepare the union before mutating engine or ownership. One group's removal
/// must not withdraw another group's shared peer; port conflicts fail closed.
type PeerUnion = (BTreeMap<u128, Vec<Peer>>, BTreeMap<IpAddr, Ports>);
fn peer_union(
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
fn peer_table(bind: IpAddr, peers: &[Peer]) -> io::Result<BTreeMap<IpAddr, Ports>> {
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
    use crate::{engine::Quality, sender::Peer};
    use clock::{
        domain::{Domain, LocalClock, LocalInstant, Reading},
        ptp_sender::Master,
        role::{Policy, Role},
    };
    pub(super) struct TestClock;
    impl ClockSource for TestClock {
        fn sample(&self) -> io::Result<(LocalInstant, Reading)> {
            let now = clock::now_ns();
            Ok((
                LocalClock::new(1).unwrap().at(now),
                Domain::new(100, 1).unwrap().reading(now + 10_000_000_000),
            ))
        }
    }
    pub(super) fn config(peer: Peer, policy: Policy) -> Config {
        Config {
            probe: None,
            network: NetworkConfig {
                domain: Domain::new(100, 1).unwrap(),
                bind_ip: "127.0.0.1".parse().unwrap(),
                ports: Ports {
                    event: 0,
                    general: 0,
                },
                peers: vec![peer],
            },
            election: role::Config {
                policy,
                settle_ns: 10_000_000,
                announce_lease_ns: 200_000_000,
                min_announces: 2,
            },
            follow: follower::Config {
                pairing_lease_ns: 100_000_000,
                mapping_lease_ns: 150_000_000,
                warmup_samples: 2,
                step_threshold_ns: 50_000_000,
            },
        }
    }
    pub(super) async fn wait_for(
        rx: &mut watch::Receiver<Snapshot>,
        test: impl Fn(Snapshot) -> bool,
    ) -> Snapshot {
        timeout(Duration::from_secs(2), async {
            loop {
                let value = *rx.borrow_and_update();
                if test(value) {
                    break value;
                }
                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }
    async fn emit(
        m: &mut Master,
        n: u64,
        event: &UdpSocket,
        general: &UdpSocket,
        addresses: (SocketAddr, SocketAddr),
    ) {
        for d in m.poll(n * 1_000_000_000).unwrap() {
            let (socket, target) = if d.port == 319 {
                (event, addresses.0)
            } else {
                (general, addresses.1)
            };
            socket.send_to(&d.bytes, target).await.unwrap();
            if let Some(token) = d.completion {
                let f = m
                    .complete_send(
                        &token,
                        Domain::new(50, 1)
                            .unwrap()
                            .reading(clock::now_ns() + 20_000_000_000),
                    )
                    .unwrap();
                general.send_to(&f.bytes, addresses.1).await.unwrap();
            }
        }
    }
    #[tokio::test]
    async fn loopback_master_follower_master_uses_same_sockets_and_revokes_old_map() {
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.1".parse().unwrap(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let service = Service::bind(config(peer, Policy::Elect), Arc::new(TestClock))
            .await
            .unwrap();
        let addresses = service.addresses();
        let mut status = service.subscribe();
        let local = wait_for(&mut status, |s| {
            s.selection.role == Role::Master && s.mapping.is_some()
        })
        .await;
        let mut remote = Master::new(Domain::new(50, 1).unwrap()).unwrap();
        remote.set_peers(&["127.0.0.1".parse().unwrap()]).unwrap();
        for n in 0..4 {
            emit(&mut remote, n, &event, &general, addresses).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let foreign = wait_for(&mut status, |s| {
            s.selection.role == Role::Follower(peer.ip) && s.mapping.is_some()
        })
        .await;
        assert_eq!(foreign.quality, Some(Quality::OneWay));
        assert_eq!(foreign.selection.domain.unwrap().id(), 50);
        assert_ne!(foreign.selection.domain, local.selection.domain);
        let back = wait_for(&mut status, |s| {
            s.selection.role == Role::Master && s.selection.domain != local.selection.domain
        })
        .await;
        assert_eq!(back.quality, Some(Quality::Local));
        assert_eq!(service.addresses(), addresses);
        service.shutdown().await.unwrap();
        assert!(status.borrow().mapping.is_none());
        let _event = UdpSocket::bind(addresses.0).await.unwrap();
        let _general = UdpSocket::bind(addresses.1).await.unwrap();
    }
    #[tokio::test]
    async fn opt_in_probe_completes_over_loopback_without_changing_mapping_quality() {
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.1".parse().unwrap(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let mut cfg = config(peer, Policy::FollowOnly(peer.ip));
        cfg.election.announce_lease_ns = 2_000_000_000;
        cfg.follow.mapping_lease_ns = 1_000_000_000;
        cfg.probe = Some(probe::Config {
            interval_ns: 125_000_000,
            timeout_ns: 100_000_000,
            measurement: clock::measurement::Config {
                max_roundtrip_ns: 100_000_000,
                rate_window_ns: 1_000_000_000,
                valid_for_ns: 1_000_000_000,
                timestamp_error_ns: 1_000_000,
            },
        });
        let service = Service::bind(cfg, Arc::new(TestClock)).await.unwrap();
        let addresses = service.addresses();
        let mut status = service.subscribe();
        let domain = Domain::new(50, 1).unwrap();
        let mut remote = Master::new(domain).unwrap();
        remote.set_peers(&["127.0.0.1".parse().unwrap()]).unwrap();
        for n in 0..4 {
            emit(&mut remote, n, &event, &general, addresses).await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut buffer = [0u8; 1024];
        let (size, addr) = timeout(Duration::from_secs(1), event.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(buffer[0] & 15, 2);
        let response = remote
            .receive(
                addr.ip(),
                &buffer[..size],
                domain.reading(clock::now_ns() + 20_000_000_000),
            )
            .unwrap();
        event.send_to(&response.bytes, addresses.0).await.unwrap();
        let follow = remote
            .complete_send(
                response.completion.as_ref().unwrap(),
                domain.reading(clock::now_ns() + 20_000_000_000),
            )
            .unwrap();
        general.send_to(&follow.bytes, addresses.1).await.unwrap();
        let measured = wait_for(&mut status, |s| s.link.is_some() && s.mapping.is_some()).await;
        assert_eq!(measured.link.unwrap().neighbor.id(), 50);
        assert_eq!(measured.quality, Some(Quality::OneWay));
        service.shutdown().await.unwrap();
        assert!(status.borrow().link.is_none());
    }
    #[tokio::test]
    async fn follow_only_without_evidence_never_creates_a_master() {
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.1".parse().unwrap(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let service = Service::bind(
            config(peer, Policy::FollowOnly(peer.ip)),
            Arc::new(TestClock),
        )
        .await
        .unwrap();
        let status = service.subscribe();
        tokio::time::sleep(Duration::from_millis(75)).await;
        assert_eq!(status.borrow().selection.role, Role::Listening);
        assert!(status.borrow().mapping.is_none());
        let mut buffer = [0; 1024];
        assert_eq!(
            event.try_recv(&mut buffer).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(
            general.try_recv(&mut buffer).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        service.shutdown().await.unwrap();
    }
}
#[cfg(test)]
mod peer_update_tests {
    use super::tests::{TestClock, config, wait_for};
    use super::*;
    use clock::role::{Policy, Role};
    #[test]
    fn shared_peers_survive_one_owner_removal_and_conflicts_are_atomic() {
        let bind = "127.0.0.1".parse().unwrap();
        let peer = Peer {
            ip: "127.0.0.1".parse().unwrap(),
            ports: Ports {
                event: 10001,
                general: 10002,
            },
        };
        let (owners, _) = peer_union(bind, &BTreeMap::new(), 1, vec![peer]).unwrap();
        let (owners, union) = peer_union(bind, &owners, 2, vec![peer]).unwrap();
        assert_eq!(union.len(), 1);
        let (owners, union) = peer_union(bind, &owners, 1, vec![]).unwrap();
        assert_eq!(owners.len(), 1);
        assert_eq!(union.len(), 1);
        let conflict = Peer {
            ports: Ports {
                event: 10003,
                ..peer.ports
            },
            ..peer
        };
        assert!(peer_union(bind, &owners, 3, vec![conflict]).is_err());
        assert_eq!(owners.len(), 1);
        let (owners, union) = peer_union(bind, &owners, 2, vec![]).unwrap();
        assert!(owners.is_empty());
        assert!(union.is_empty());
    }
    #[tokio::test]
    async fn live_peer_changes_reuse_sockets_source_and_master_epoch() {
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = Peer {
            ip: event.local_addr().unwrap().ip(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let service = Service::bind(config(peer, Policy::Elect), Arc::new(TestClock))
            .await
            .unwrap();
        let addresses = service.addresses();
        let mut status = service.subscribe();
        let selected = wait_for(&mut status, |s| s.selection.role == Role::Master)
            .await
            .selection;
        let next_event = UdpSocket::bind("127.0.0.3:0").await.unwrap();
        let next_general = UdpSocket::bind("127.0.0.3:0").await.unwrap();
        let next = Peer {
            ip: next_event.local_addr().unwrap().ip(),
            ports: Ports {
                event: next_event.local_addr().unwrap().port(),
                general: next_general.local_addr().unwrap().port(),
            },
        };
        service.set_group_peers(1, vec![next]).await.unwrap();
        assert_eq!(service.addresses(), addresses);
        assert_eq!(status.borrow().selection, selected);
        let mut buffer = [0; 1024];
        let (_, source) = timeout(Duration::from_secs(1), next_event.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source, addresses.0);
        assert!(
            service
                .set_peers(vec![Peer {
                    ip: "224.0.0.1".parse().unwrap(),
                    ..next
                }])
                .await
                .is_err()
        );
        // Releasing the base receiver interest must keep the outgoing group.
        service.set_peers(vec![]).await.unwrap();
        timeout(Duration::from_secs(1), next_event.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert!(service.set_group_peers(0, vec![next]).await.is_err());
        service.set_group_peers(1, vec![]).await.unwrap();
        while next_event.try_recv(&mut buffer).is_ok() {}
        assert!(
            timeout(
                Duration::from_millis(100),
                next_event.recv_from(&mut buffer)
            )
            .await
            .is_err()
        );
        assert_eq!(service.addresses(), addresses);
        assert_eq!(status.borrow().selection, selected);
        assert!(status.borrow().mapping.is_some());
        service.shutdown().await.unwrap();
    }
}
struct StatusGuard(watch::Sender<Snapshot>);
impl Drop for StatusGuard {
    fn drop(&mut self) {
        self.0.send_replace(Snapshot::inactive());
    }
}
impl Service {
    pub async fn bind(config: Config, clock: Arc<dyn ClockSource>) -> io::Result<Self> {
        let network = config.network;
        if network.bind_ip.is_unspecified()
            || network.bind_ip.is_multicast()
            || network.peers.len() > 64
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "specific bind IP and <=64 peers required",
            ));
        }
        let mut peers = peer_table(network.bind_ip, &network.peers)?;
        let mut owners = BTreeMap::new();
        if !network.peers.is_empty() {
            owners.insert(0, network.peers.clone());
        }
        let (initial, reading) = clock.sample()?;
        if reading.domain() != network.domain {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "own physical clock differs",
            ));
        }
        let source = Source {
            inner: clock,
            domain: network.domain,
            local: initial.clock(),
        };
        let ips = network.peers.iter().map(|peer| peer.ip).collect::<Vec<_>>();
        let mut engine = Engine::new(
            network.domain,
            initial,
            &ips,
            config.election,
            config.follow,
        )?;
        if let Some(probe) = config.probe {
            engine.enable_probe(probe)?;
        }
        let event = UdpSocket::bind(SocketAddr::new(network.bind_ip, network.ports.event)).await?;
        let general =
            UdpSocket::bind(SocketAddr::new(network.bind_ip, network.ports.general)).await?;
        let addresses = (event.local_addr()?, general.local_addr()?);
        let (status, _) = watch::channel(Snapshot::inactive());
        let worker_status = status.clone();
        let (stop, mut stopped) = oneshot::channel();
        let (peer_control, mut peer_requests) = mpsc::channel::<PeerRequest>(8);
        let task = tokio::spawn(async move {
            let _guard = StatusGuard(worker_status.clone());
            let mut tick = interval(Duration::from_millis(25));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut event_buffer = [0u8; 1024];
            let mut general_buffer = [0u8; 1024];
            loop {
                let batch = tokio::select! {
                    biased;
                    _=&mut stopped=>return Ok(()),
                    _=tick.tick()=>{let (local,own)=source.sample()?;engine.tick(local,own)?},
                    request=peer_requests.recv()=>{
                        let Some(request) = request else { return Ok(()); };
                        // A cancelled caller must reconcile state: once queued,
                        // an update may still apply. Ack means applied locally,
                        // never receiver adoption or measured synchronization.
                        let result = match peer_union(network.bind_ip, &owners, request.owner, request.peers) {
                            Err(error) => Err(error),
                            Ok((next_owners, next)) => {
                                let (local, own) = source.sample()?;
                                engine.set_peers(&next.keys().copied().collect::<Vec<_>>(), local, own)?;
                                peers = next;
                                owners = next_owners;
                                worker_status.send_replace(engine.snapshot());
                                Ok(())
                            },
                        };
                        let _ = request.reply.send(result);
                        continue;
                    },
                    received=event.recv_from(&mut event_buffer)=>{
                        let (size,addr)=received?;let (local,own)=source.sample()?;
                        engine.receive(addr.ip(),&event_buffer[..size],local,own)?
                    },
                    received=general.recv_from(&mut general_buffer)=>{
                        let (size,addr)=received?;let (local,own)=source.sample()?;
                        engine.receive(addr.ip(),&general_buffer[..size],local,own)?
                    },
                };
                dispatch(batch, &mut engine, &event, &general, &peers, &source).await?;
                worker_status.send_replace(engine.snapshot());
            }
        });
        Ok(Self {
            addresses,
            status,
            stop: Some(stop),
            task: Some(task),
            peers: peer_control,
        })
    }
    pub fn addresses(&self) -> (SocketAddr, SocketAddr) {
        self.addresses
    }
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.status.subscribe()
    }
    /// Replace base receiver/service peers (owner zero). Group-owned peers are
    /// retained independently. Sockets/source remain alive, including no peers.
    /// Bounded queue: retry WouldBlock; cancellation after enqueue is not rollback.
    pub async fn set_peers(&self, peers: Vec<Peer>) -> io::Result<()> {
        self.update_peers(0, peers).await
    }
    /// Replace one consumer's peers in the common service's union. Owner is an
    /// application lifecycle token, not proof of native AirPlay group adoption.
    /// An empty list releases only this owner; explicit release is required.
    pub async fn set_group_peers(&self, owner: u128, peers: Vec<Peer>) -> io::Result<()> {
        if owner == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reserved timing owner",
            ));
        }
        self.update_peers(owner, peers).await
    }
    async fn update_peers(&self, owner: u128, peers: Vec<Peer>) -> io::Result<()> {
        peer_table(self.addresses.0.ip(), &peers)?;
        let (reply, result) = oneshot::channel();
        self.peers
            .try_send(PeerRequest {
                owner,
                peers,
                reply,
            })
            .map_err(|error| {
                io::Error::new(
                    match error {
                        mpsc::error::TrySendError::Full(_) => io::ErrorKind::WouldBlock,
                        mpsc::error::TrySendError::Closed(_) => io::ErrorKind::NotConnected,
                    },
                    "timing peer update unavailable",
                )
            })?;
        result
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::NotConnected, "clock service ended"))?
    }
    pub async fn shutdown(mut self) -> io::Result<()> {
        self.status.send_replace(Snapshot::inactive());
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let Some(mut task) = self.task.take() else {
            return Ok(());
        };
        match timeout(Duration::from_secs(1), &mut task).await {
            Ok(result) => result.map_err(io::Error::other)?,
            Err(_) => {
                task.abort();
                let _ = task.await;
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "unified clock shutdown deadline",
                ))
            }
        }
    }
}
impl Drop for Service {
    fn drop(&mut self) {
        self.status.send_replace(Snapshot::inactive());
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
async fn dispatch(
    batch: Vec<Emission>,
    engine: &mut Engine,
    event: &UdpSocket,
    general: &UdpSocket,
    peers: &BTreeMap<std::net::IpAddr, Ports>,
    source: &Source,
) -> io::Result<()> {
    let mut queue: VecDeque<_> = batch.into();
    while let Some(emission) = queue.pop_front() {
        let packet = &emission.datagram;
        let peer = peers
            .get(&packet.peer)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unconfigured clock peer"))?;
        let (socket, port) = match packet.port {
            319 => (event, peer.event),
            320 => (general, peer.general),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid clock port",
                ));
            }
        };
        let sent = timeout(
            Duration::from_millis(100),
            socket.send_to(&packet.bytes, SocketAddr::new(packet.peer, port)),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "clock send deadline"))??;
        if sent != packet.bytes.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short clock send"));
        }
        if emission.needs_completion() {
            let (local, own) = source.sample()?;
            if let Some(follow) = engine.complete(&emission, local, own)? {
                queue.push_front(follow);
            }
        }
    }
    Ok(())
}
