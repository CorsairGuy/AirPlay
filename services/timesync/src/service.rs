//! Experimental unified unicast RX/TX loop. Explicit bind only; no CLI wiring.
//! Exactly one socket pair and one injected physical ClockSource serve all
//! selected roles. Snapshot mappings are local/one-way estimates, NOT lock or
//! acoustic readiness. No multicast or complete BMCA topology implementation.
use crate::peer_interests::{OwnerInterests, peer_table};
use crate::{
    engine::{Emission, Engine, Snapshot},
    sender::{ClockSource, Config as NetworkConfig, Peer, Ports, Source},
};
use clock::{follower, probe, role};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
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
    inbound: bool,
    deadline: Option<std::time::Instant>,
    completion: Arc<AtomicU8>,
    reply: oneshot::Sender<io::Result<()>>,
}

const PENDING: u8 = 0;
const ACCEPTED: u8 = 1;
const CANCELLED: u8 = 2;

// The receiver marks a queued update cancelled if its future is dropped,
// including after the worker has sent the reply but before it is observed.
struct CompletionGuard(Arc<AtomicU8>);
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        let _ = self
            .0
            .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
    }
}

fn dual_union(
    inbound: &OwnerInterests,
    outbound: &OwnerInterests,
) -> io::Result<BTreeMap<IpAddr, Ports>> {
    let mut all = inbound.union().clone();
    for (ip, ports) in outbound.union() {
        if all.insert(*ip, *ports).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "peer cannot be both timing source and receiver",
            ));
        }
    }
    if all.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many timing peers",
        ));
    }
    Ok(all)
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
    async fn master_answers_unicast_delay_and_grant() {
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.1".parse().unwrap(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let mut initial = config(peer, Policy::Elect);
        initial.network.peers.clear();
        let service = Service::bind(initial, Arc::new(TestClock)).await.unwrap();
        let mut status = service.subscribe();
        wait_for(&mut status, |s| {
            s.selection.role == Role::Master && s.mapping.is_some()
        })
        .await;
        service.set_peers(vec![peer]).await.unwrap();
        let addresses = service.addresses();
        let mut request = vec![0u8; 44];
        request[0] = 0x11;
        request[1] = 2;
        request[2..4].copy_from_slice(&44u16.to_be_bytes());
        request[20..28].copy_from_slice(&2u64.to_be_bytes());
        request[28..30].copy_from_slice(&1u16.to_be_bytes());
        request[30..32].copy_from_slice(&42u16.to_be_bytes());
        event.send_to(&request, addresses.0).await.unwrap();
        let mut signaling = request.clone();
        signaling.resize(54, 0);
        signaling[0] = 0x1c;
        signaling[2..4].copy_from_slice(&54u16.to_be_bytes());
        signaling[44..54].copy_from_slice(&[0, 4, 0, 6, 0xb0, 0, 0, 0, 0, 30]);
        general.send_to(&signaling, addresses.1).await.unwrap();
        let (delay, grant) = timeout(Duration::from_secs(2), async {
            let mut buffer = [0u8; 1024];
            let mut delay = false;
            let mut grant = false;
            while !delay || !grant {
                let (n, _) = general.recv_from(&mut buffer).await.unwrap();
                match buffer[0] & 15 {
                    9 if n == 54 => {
                        assert_eq!(&buffer[44..54], &request[20..30]);
                        delay = true;
                    }
                    12 if n >= 56 => {
                        assert_eq!(&buffer[44..50], &[0, 5, 0, 8, 0xb0, 0]);
                        grant = true;
                    }
                    _ => {}
                }
            }
            (delay, grant)
        })
        .await
        .unwrap();
        assert!(delay && grant);
        service.shutdown().await.unwrap();
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
        // Drive remote evidence until it is actually observed. A fixed burst of
        // datagrams across separate sockets can miss election/warmup on macOS.
        let foreign = timeout(Duration::from_secs(2), async {
            let mut n = 0;
            loop {
                emit(&mut remote, n, &event, &general, addresses).await;
                n += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
                let value = *status.borrow_and_update();
                if value.selection.role == Role::Follower(peer.ip) && value.mapping.is_some() {
                    break value;
                }
            }
        })
        .await
        .expect("fresh remote evidence must create a follower mapping");
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
    async fn dual_service_follows_a_source_and_serves_receivers_on_one_pair() {
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.1".parse().unwrap(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let source = Arc::new(crate::media::InstantSource::new(
            std::time::Instant::now(),
            LocalClock::new(1).unwrap(),
            Domain::new(100, 1).unwrap(),
            clock::now_ns() + 10_000_000_000,
        ));
        let service = Service::bind_dual(config(peer, Policy::FollowOnly(peer.ip)), source.clone())
            .await
            .unwrap();
        let receiver_event = UdpSocket::bind("127.0.0.3:0").await.unwrap();
        let receiver_general = UdpSocket::bind("127.0.0.3:0").await.unwrap();
        let receiver = Peer {
            ip: receiver_event.local_addr().unwrap().ip(),
            ports: Ports {
                event: receiver_event.local_addr().unwrap().port(),
                general: receiver_general.local_addr().unwrap().port(),
            },
        };
        service.set_group_peers(1, vec![receiver]).await.unwrap();
        service.set_group_peers(2, vec![receiver]).await.unwrap();
        service.set_group_peers(1, vec![]).await.unwrap();
        assert!(
            service.set_group_peers(2, vec![peer]).await.is_err(),
            "source cannot be a receiver"
        );
        assert!(
            service.set_session_peers(3, vec![receiver]).await.is_err(),
            "receiver cannot become source"
        );
        let addresses = service.addresses();
        let mut status = service.subscribe();
        let mut remote = Master::new(Domain::new(50, 1).unwrap()).unwrap();
        remote.set_peers(&[peer.ip]).unwrap();
        let mut buffer = [0u8; 1024];
        let followed = timeout(Duration::from_secs(2), async {
            let mut round = 0;
            loop {
                emit(&mut remote, round, &event, &general, addresses).await;
                round += 1;
                let value = *status.borrow_and_update();
                if value.selection.role == Role::Follower(peer.ip) && value.mapping.is_some() {
                    break value;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("inbound evidence must be followed");
        assert_eq!(followed.quality, Some(Quality::OneWay));
        assert_eq!(followed.selection.domain.unwrap().id(), 50);
        let outbound = followed
            .outbound
            .expect("outbound mapping survives following");
        assert_eq!(outbound.domain().id(), 100);
        assert_ne!(outbound.domain(), followed.selection.domain.unwrap());
        let timeline = Arc::new(
            crate::media::MasterTimeline::new_relay(
                source,
                service.subscribe(),
                Duration::from_millis(100),
            )
            .unwrap(),
        );
        let (_upstream, upstream) = watch::channel(false);
        let lease = crate::media::MasterLease::spawn(timeline, upstream);
        let mut stopped = lease.subscribe();
        assert!(!*stopped.borrow());
        // The same socket pair still serves receivers while we follow.
        while receiver_event.try_recv(&mut buffer).is_ok() {}
        let (size, _) = timeout(
            Duration::from_secs(1),
            receiver_event.recv_from(&mut buffer),
        )
        .await
        .expect("outbound Sync while following")
        .unwrap();
        assert!(size >= 44);
        assert_eq!(buffer[0] & 15, 0);
        assert_eq!(&buffer[20..28], &100u64.to_be_bytes());
        assert!(
            event.try_recv(&mut buffer).is_err(),
            "no outbound Sync to source"
        );
        service.set_peers(vec![]).await.unwrap();
        timeout(Duration::from_secs(1), stopped.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(
            *stopped.borrow(),
            "removing the inbound source stops relay media"
        );
        service.shutdown().await.unwrap();
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
        // A queued request may expire while the fixture emits warmup traffic.
        // Serve subsequent probes too; never assume the first datagram is live.
        let (stop, mut stopped) = oneshot::channel();
        let responding = tokio::spawn(async move {
            let mut buffer = [0u8; 1024];
            loop {
                let (size, addr) = tokio::select! {
                    _ = &mut stopped => break,
                    got = event.recv_from(&mut buffer) => got.unwrap(),
                };
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
            }
        });
        let measured = wait_for(&mut status, |s| s.link.is_some() && s.mapping.is_some()).await;
        let _ = stop.send(());
        responding.await.unwrap();
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
    use crate::peer_interests::peer_union;
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

    #[test]
    fn dual_peer_conflict_does_not_modify_the_committed_owners() {
        let bind = "192.0.2.1".parse().unwrap();
        let mut inbound = OwnerInterests::new(bind);
        let mut outbound = OwnerInterests::new(bind);
        let source = Peer {
            ip: "192.0.2.2".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        let receiver = Peer {
            ip: "192.0.2.3".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        inbound.set(1, vec![source], None).unwrap();
        outbound.set(2, vec![receiver], None).unwrap();
        let previous = dual_union(&inbound, &outbound).unwrap();
        let mut candidate = outbound.clone();
        candidate.set(2, vec![source], None).unwrap();
        assert!(dual_union(&inbound, &candidate).is_err());
        assert_eq!(dual_union(&inbound, &outbound).unwrap(), previous);
    }

    #[test]
    fn dual_expiry_keeps_the_other_role_and_shared_owners() {
        let bind = "192.0.2.1".parse().unwrap();
        let mut inbound = OwnerInterests::new(bind);
        let mut outbound = OwnerInterests::new(bind);
        let source = Peer {
            ip: "192.0.2.2".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        let receiver = Peer {
            ip: "192.0.2.3".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        let now = std::time::Instant::now();
        inbound.set(1, vec![source], Some(now)).unwrap();
        inbound.set(2, vec![source], None).unwrap();
        outbound.set(3, vec![receiver], Some(now)).unwrap();
        inbound.expire(now);
        outbound.expire(now);
        let remaining = dual_union(&inbound, &outbound).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining.get(&source.ip), Some(&source.ports));
        assert_eq!(inbound.owner_count(), 1);
        assert_eq!(outbound.owner_count(), 0);
    }

    #[tokio::test]
    async fn cancelled_after_apply_releases_the_owner_without_touching_others() {
        let source = Peer {
            ip: "127.0.0.2".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        let receiver = Peer {
            ip: "127.0.0.3".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        let service = Service::bind_dual(
            config(source, Policy::FollowOnly(source.ip)),
            Arc::new(TestClock),
        )
        .await
        .unwrap();
        let other = Peer {
            ip: "127.0.0.4".parse().unwrap(),
            ports: receiver.ports,
        };
        service.set_group_peers(9, vec![other]).await.unwrap();
        let (reply, wait) = oneshot::channel();
        let completion = Arc::new(AtomicU8::new(PENDING));
        let guard = CompletionGuard(completion.clone());
        assert!(
            service
                .peers
                .send(PeerRequest {
                    owner: 3,
                    peers: vec![receiver],
                    inbound: false,
                    deadline: None,
                    completion,
                    reply
                })
                .await
                .is_ok()
        );
        wait.await.unwrap().unwrap();
        assert!(service.set_session_peers(4, vec![receiver]).await.is_err());
        drop(guard);
        timeout(Duration::from_secs(2), async {
            loop {
                if service.set_session_peers(4, vec![receiver]).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled owner must be released");
        assert!(
            service.set_session_peers(5, vec![other]).await.is_err(),
            "another owner remains active"
        );
        service.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_older_update_cannot_remove_a_newer_update_for_same_owner() {
        let source = Peer {
            ip: "127.0.0.2".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        let old = Peer {
            ip: "127.0.0.3".parse().unwrap(),
            ports: source.ports,
        };
        let new = Peer {
            ip: "127.0.0.4".parse().unwrap(),
            ports: source.ports,
        };
        let service = Service::bind_dual(
            config(source, Policy::FollowOnly(source.ip)),
            Arc::new(TestClock),
        )
        .await
        .unwrap();
        let (reply, wait) = oneshot::channel();
        let completion = Arc::new(AtomicU8::new(PENDING));
        let guard = CompletionGuard(completion.clone());
        assert!(
            service
                .peers
                .send(PeerRequest {
                    owner: 3,
                    peers: vec![old],
                    inbound: false,
                    deadline: None,
                    completion,
                    reply
                })
                .await
                .is_ok()
        );
        wait.await.unwrap().unwrap();
        service.set_group_peers(3, vec![new]).await.unwrap();
        drop(guard);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            service.set_session_peers(4, vec![new]).await.is_err(),
            "newer update must remain"
        );
        service.set_session_peers(5, vec![old]).await.unwrap();
        service.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_public_update_future_does_not_leave_its_peer() {
        let source = Peer {
            ip: "127.0.0.2".parse().unwrap(),
            ports: Ports {
                event: 319,
                general: 320,
            },
        };
        let peer = Peer {
            ip: "127.0.0.3".parse().unwrap(),
            ports: source.ports,
        };
        let service = Service::bind_dual(
            config(source, Policy::FollowOnly(source.ip)),
            Arc::new(TestClock),
        )
        .await
        .unwrap();
        let mut update = Box::pin(service.set_group_peers(3, vec![peer]));
        tokio::select! {
            biased;
            result = &mut update => panic!("update completed before cancellation: {result:?}"),
            _ = std::future::ready(()) => {},
        }
        drop(update);
        timeout(Duration::from_secs(2), async {
            loop {
                if service.set_session_peers(4, vec![peer]).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled future must not leave a peer");
        service.shutdown().await.unwrap();
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

    #[tokio::test]
    async fn expired_owner_stops_receiving_without_stopping_other_owners() {
        let base_event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let base_general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let base = Peer {
            ip: base_event.local_addr().unwrap().ip(),
            ports: Ports {
                event: base_event.local_addr().unwrap().port(),
                general: base_general.local_addr().unwrap().port(),
            },
        };
        let event = UdpSocket::bind("127.0.0.3:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.3:0").await.unwrap();
        let leased = Peer {
            ip: event.local_addr().unwrap().ip(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let service = Service::bind(config(base, Policy::Elect), Arc::new(TestClock))
            .await
            .unwrap();
        let mut status = service.subscribe();
        wait_for(&mut status, |s| s.selection.role == Role::Master).await;
        service
            .set_group_peers_for(1, vec![leased], Some(Duration::from_millis(300)))
            .await
            .unwrap();
        let mut buffer = [0; 1024];
        timeout(Duration::from_secs(1), event.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(450)).await;
        while event.try_recv(&mut buffer).is_ok() {}
        assert!(
            timeout(Duration::from_millis(300), event.recv_from(&mut buffer))
                .await
                .is_err()
        );
        while base_event.try_recv(&mut buffer).is_ok() {}
        timeout(Duration::from_secs(1), base_event.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
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
        Self::bind_mode(config, clock, false).await
    }

    /// One socket pair and one physical source with an explicit co-resident
    /// outbound master. Receivers are served from our own domain while the
    /// selected role may follow an inbound source; domains never mix.
    pub async fn bind_dual(config: Config, clock: Arc<dyn ClockSource>) -> io::Result<Self> {
        Self::bind_mode(config, clock, true).await
    }

    async fn bind_mode(
        config: Config,
        clock: Arc<dyn ClockSource>,
        dual: bool,
    ) -> io::Result<Self> {
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
        peer_table(network.bind_ip, &network.peers)?;
        let mut interests = OwnerInterests::new(network.bind_ip);
        let mut output_interests = OwnerInterests::new(network.bind_ip);
        if !network.peers.is_empty() {
            interests.set(0, network.peers.clone(), None)?;
        }
        let mut peers = interests.union().clone();
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
        let mut engine = if dual {
            Engine::new_dual(
                network.domain,
                initial,
                &ips,
                config.election,
                config.follow,
            )?
        } else {
            Engine::new(
                network.domain,
                initial,
                &ips,
                config.election,
                config.follow,
            )?
        };
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
            let mut pending = BTreeMap::<(bool, u128), Arc<AtomicU8>>::new();
            loop {
                let batch = tokio::select! {
                    biased;
                    _=&mut stopped=>return Ok(()),
                    _=tick.tick()=>{
                        let mut changed = false;
                        pending.retain(|&(inbound, owner), completion| match completion.load(Ordering::Acquire) {
                            CANCELLED => {
                                let owners = if dual && !inbound { &mut output_interests } else { &mut interests };
                                changed |= owners.release(owner).is_some();
                                false
                            },
                            ACCEPTED => false,
                            _ => true,
                        });
                        // A silent owner expires exactly like an explicit release.
                        let input_change = interests.expire(std::time::Instant::now());
                        let output_change = output_interests.expire(std::time::Instant::now());
                        if changed || input_change.is_some() || output_change.is_some() {
                            let (local, own) = source.sample()?;
                            if dual {
                                engine.set_dual_peers(&interests.union().keys().copied().collect::<Vec<_>>(), &output_interests.union().keys().copied().collect::<Vec<_>>(), local, own)?;
                                peers = dual_union(&interests, &output_interests)?;
                            } else {
                                engine.set_peers(&interests.union().keys().copied().collect::<Vec<_>>(), local, own)?;
                                peers = interests.union().clone();
                            }
                            worker_status.send_replace(engine.snapshot());
                        }
                        let (local,own)=source.sample()?;
                        engine.tick(local,own)?
                    },
                    request=peer_requests.recv()=>{
                        let Some(request) = request else { return Ok(()); };
                        if request.completion.load(Ordering::Acquire) == CANCELLED || request.reply.is_closed() { continue; }
                        let owner_key = (dual && request.inbound, request.owner);
                        let completion = request.completion.clone();
                        let result = if dual {
                            let mut input = interests.clone();
                            let mut output = output_interests.clone();
                            let selected = if request.inbound { &mut input } else { &mut output };
                            match selected.set(request.owner, request.peers, request.deadline) {
                                Err(error) => Err(error),
                                Ok(_) => match dual_union(&input, &output) {
                                    Err(error) => Err(error),
                                    Ok(next) => {
                                        let (local, own) = source.sample()?;
                                        engine.set_dual_peers(&input.union().keys().copied().collect::<Vec<_>>(), &output.union().keys().copied().collect::<Vec<_>>(), local, own)?;
                                        peers = next;
                                        interests = input;
                                        output_interests = output;
                                        worker_status.send_replace(engine.snapshot());
                                        Ok(())
                                    }
                                }
                            }
                        } else { match interests.set(request.owner, request.peers, request.deadline) {
                            Err(error) => Err(error),
                            Ok(change) => {
                                if let Some(change) = change {
                                    let (local, own) = source.sample()?;
                                    engine.set_peers(
                                        &change.union().keys().copied().collect::<Vec<_>>(),
                                        local,
                                        own,
                                    )?;
                                    peers = change.union().clone();
                                    worker_status.send_replace(engine.snapshot());
                                }
                                Ok(())
                            },
                        }};
                        if result.is_ok() { pending.insert(owner_key, completion); }
                        let _ = request.reply.send(result);
                        continue;
                    },
                    received=event.recv_from(&mut event_buffer)=>{
                        let (size,addr)=match received {
                            Ok(packet) => packet,
                            // Windows can report an ICMP port-unreachable on a later
                            // recv from an otherwise usable, unconnected UDP socket.
                            Err(error) if cfg!(windows) && error.kind() == io::ErrorKind::ConnectionReset => continue,
                            Err(error) => return Err(error),
                        };let (local,own)=source.sample()?;
                        engine.receive(addr.ip(),&event_buffer[..size],local,own)?
                    },
                    received=general.recv_from(&mut general_buffer)=>{
                        let (size,addr)=match received {
                            Ok(packet) => packet,
                            Err(error) if cfg!(windows) && error.kind() == io::ErrorKind::ConnectionReset => continue,
                            Err(error) => return Err(error),
                        };let (local,own)=source.sample()?;
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
    /// Bounded queue: retry WouldBlock. A cancelled update releases its owner
    /// asynchronously; this is fail-closed, not restoration of prior peers.
    pub async fn set_peers(&self, peers: Vec<Peer>) -> io::Result<()> {
        self.update_peers(0, peers, None, true).await
    }
    /// Peers belonging to an incoming session, distinct from group receivers.
    pub async fn set_session_peers(&self, owner: u128, peers: Vec<Peer>) -> io::Result<()> {
        if owner == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reserved timing owner",
            ));
        }
        self.update_peers(owner, peers, None, true).await
    }
    /// Lease the owner's peers for at most `lease`. A consumer that stops
    /// refreshing releases its peers without an explicit call; other owners are
    /// untouched. The deadline is armed when the request is made, not when the
    /// worker happens to process it.
    pub async fn set_group_peers_for(
        &self,
        owner: u128,
        peers: Vec<Peer>,
        lease: Option<Duration>,
    ) -> io::Result<()> {
        if owner == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reserved timing owner",
            ));
        }
        let deadline = lease.and_then(|lease| std::time::Instant::now().checked_add(lease));
        if lease.is_some() && deadline.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unbounded timing-owner lease",
            ));
        }
        self.update_peers(owner, peers, deadline, false).await
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
        self.update_peers(owner, peers, None, false).await
    }
    async fn update_peers(
        &self,
        owner: u128,
        peers: Vec<Peer>,
        deadline: Option<std::time::Instant>,
        inbound: bool,
    ) -> io::Result<()> {
        peer_table(self.addresses.0.ip(), &peers)?;
        let (reply, result) = oneshot::channel();
        let completion = Arc::new(AtomicU8::new(PENDING));
        let guard = CompletionGuard(completion.clone());
        self.peers
            .try_send(PeerRequest {
                owner,
                peers,
                inbound,
                deadline,
                completion,
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
        let outcome = result
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::NotConnected, "clock service ended"))?;
        guard.0.store(ACCEPTED, Ordering::Release);
        outcome
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
