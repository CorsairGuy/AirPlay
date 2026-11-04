//! Experimental unified unicast RX/TX loop. Explicit bind only; no CLI wiring.
//! Exactly one socket pair and one injected physical ClockSource serve all
//! selected roles. Snapshot mappings are local/one-way estimates, NOT lock or
//! acoustic readiness. No multicast or complete BMCA topology implementation.
use crate::{
    engine::{Emission, Engine, Snapshot},
    sender::{ClockSource, Config as NetworkConfig, Ports, Source},
};
use clock::{follower, role};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    net::SocketAddr,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    sync::{oneshot, watch},
    task::JoinHandle,
    time::{MissedTickBehavior, interval, timeout},
};

pub struct Config {
    pub network: NetworkConfig,
    pub election: role::Config,
    pub follow: follower::Config,
}
pub struct Service {
    addresses: (SocketAddr, SocketAddr),
    status: watch::Sender<Snapshot>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<io::Result<()>>>,
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
    struct TestClock;
    impl ClockSource for TestClock {
        fn sample(&self) -> io::Result<(LocalInstant, Reading)> {
            let now = clock::now_ns();
            Ok((
                LocalClock::new(1).unwrap().at(now),
                Domain::new(100, 1).unwrap().reading(now + 10_000_000_000),
            ))
        }
    }
    fn config(peer: Peer, policy: Policy) -> Config {
        Config {
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
    async fn wait_for(
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
        let event = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.2".parse().unwrap(),
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
    async fn follow_only_without_evidence_never_creates_a_master() {
        let event = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.2".parse().unwrap(),
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
        let mut peers = BTreeMap::new();
        for peer in &network.peers {
            if peer.ports.event == 0
                || peer.ports.general == 0
                || peer.ip.is_ipv4() != network.bind_ip.is_ipv4()
                || peers.insert(peer.ip, peer.ports).is_some()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid/duplicate timing peer",
                ));
            }
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
        let event = UdpSocket::bind(SocketAddr::new(network.bind_ip, network.ports.event)).await?;
        let general =
            UdpSocket::bind(SocketAddr::new(network.bind_ip, network.ports.general)).await?;
        let addresses = (event.local_addr()?, general.local_addr()?);
        let (status, _) = watch::channel(Snapshot::inactive());
        let worker_status = status.clone();
        let (stop, mut stopped) = oneshot::channel();
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
        })
    }
    pub fn addresses(&self) -> (SocketAddr, SocketAddr) {
        self.addresses
    }
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.status.subscribe()
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
        if packet.completion.is_some() {
            let (local, own) = source.sample()?;
            if let Some(follow) = engine.complete(&emission, local, own)? {
                queue.push_front(follow);
            }
        }
    }
    Ok(())
}
