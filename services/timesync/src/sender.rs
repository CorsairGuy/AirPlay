//! Explicit experimental fixed-master UDP adapter. Never started by TimeSync.
//!
//! No clock is created here: the application supplies ONE sampled time source.
//! Automatic BMCA/follower operation, multicast and LAN deployment remain out
//! of scope. Bind fails if another service owns either address/port; no reuse.
use clock::{
    domain::{Domain, LocalClock, LocalInstant, Reading},
    ptp_sender::{Datagram, Master},
};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    sync::oneshot,
    task::JoinHandle,
    time::{MissedTickBehavior, interval, timeout},
};

/// Must sample a coherent local/selected-domain pair from the common provider.
/// Return Err for stale mappings; do not relabel timestamps after a clock step.
/// Must be nonblocking and bounded (cached/atomic state), without I/O or waiting
/// for clock lock: cancellation cannot preempt a blocked synchronous callback.
pub trait ClockSource: Send + Sync + 'static {
    fn sample(&self) -> io::Result<(LocalInstant, Reading)>;
}
#[derive(Clone, Copy, Debug)]
pub struct Ports {
    pub event: u16,
    pub general: u16,
}
#[derive(Clone, Copy, Debug)]
pub struct Peer {
    pub ip: IpAddr,
    pub ports: Ports,
}
pub struct Config {
    /// Identity of this explicitly selected fixed master, not a remote clock
    /// that we impersonate. Ownership/role policy belongs to the controller.
    pub domain: Domain,
    pub bind_ip: IpAddr,
    pub ports: Ports,
    pub peers: Vec<Peer>,
}
pub(super) struct Source {
    pub(super) inner: Arc<dyn ClockSource>,
    pub(super) domain: Domain,
    pub(super) local: LocalClock,
}
impl Source {
    pub(super) fn sample(&self) -> io::Result<(LocalInstant, Reading)> {
        let (local, remote) = self.inner.sample()?;
        if remote.domain() != self.domain || local.clock() != self.local {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PTP selected clock/epoch changed",
            ));
        }
        Ok((local, remote))
    }
}

pub struct MasterService {
    addresses: (SocketAddr, SocketAddr),
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<io::Result<()>>>,
}
impl MasterService {
    pub async fn bind(config: Config, source: Arc<dyn ClockSource>) -> io::Result<Self> {
        if config.bind_ip.is_unspecified()
            || config.bind_ip.is_multicast()
            || config.peers.len() > 64
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "specific bind IP and <=64 explicit peers required",
            ));
        }
        let mut peers = BTreeMap::new();
        for peer in &config.peers {
            if peer.ports.event == 0
                || peer.ports.general == 0
                || peer.ip.is_ipv4() != config.bind_ip.is_ipv4()
                || peers.insert(peer.ip, peer.ports).is_some()
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid/duplicate timing peer",
                ));
            }
        }
        let mut master = Master::new(config.domain).map_err(io::Error::other)?;
        master
            .set_peers(&config.peers.iter().map(|p| p.ip).collect::<Vec<_>>())
            .map_err(io::Error::other)?;
        let initial = source.sample()?;
        if initial.1.domain() != config.domain {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source and selected master differ",
            ));
        }
        let source = Source {
            inner: source,
            domain: config.domain,
            local: initial.0.clock(),
        };
        let event = UdpSocket::bind(SocketAddr::new(config.bind_ip, config.ports.event)).await?;
        let general =
            UdpSocket::bind(SocketAddr::new(config.bind_ip, config.ports.general)).await?;
        let addresses = (event.local_addr()?, general.local_addr()?);
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut tick = interval(Duration::from_millis(25));
            tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut event_buffer = [0u8; 1024];
            let mut general_buffer = [0u8; 1024];
            loop {
                let batch = tokio::select! {
                    biased;
                    _=&mut stopped=>return Ok(()),
                    _=tick.tick()=>{
                        let (local,_)=source.sample()?;master.poll(local.ns()).map_err(io::Error::other)?
                    },
                    received=event.recv_from(&mut event_buffer)=>{
                        let (size,addr)=received?;let (_,reading)=source.sample()?;
                        master.receive(addr.ip(),&event_buffer[..size],reading).into_iter().collect()
                    },
                    received=general.recv_from(&mut general_buffer)=>{
                        let (size,addr)=received?;let (_,reading)=source.sample()?;
                        master.receive(addr.ip(),&general_buffer[..size],reading).into_iter().collect()
                    },
                };
                dispatch(batch, &mut master, &event, &general, &peers, &source).await?;
            }
        });
        Ok(Self {
            addresses,
            stop: Some(stop),
            task: Some(task),
        })
    }
    pub fn addresses(&self) -> (SocketAddr, SocketAddr) {
        self.addresses
    }
    pub fn is_finished(&self) -> bool {
        self.task.as_ref().is_none_or(JoinHandle::is_finished)
    }
    pub async fn shutdown(mut self) -> io::Result<()> {
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
                    "PTP shutdown deadline",
                ))
            }
        }
    }
}
impl Drop for MasterService {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
async fn dispatch(
    batch: Vec<Datagram>,
    master: &mut Master,
    event: &UdpSocket,
    general: &UdpSocket,
    peers: &BTreeMap<IpAddr, Ports>,
    source: &Source,
) -> io::Result<()> {
    let mut queue: VecDeque<_> = batch.into();
    while let Some(packet) = queue.pop_front() {
        let peer = peers.get(&packet.peer).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "unconfigured PTP destination")
        })?;
        let (socket, port) = match packet.port {
            319 => (event, peer.event),
            320 => (general, peer.general),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unexpected PTP port",
                ));
            }
        };
        let sent = timeout(
            Duration::from_millis(100),
            socket.send_to(&packet.bytes, SocketAddr::new(packet.peer, port)),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "PTP send deadline"))??;
        if sent != packet.bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "short PTP datagram",
            ));
        }
        if let Some(token) = packet.completion {
            let (_, reading) = source.sample()?;
            let follow = master.complete_send(&token, reading).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "PTP send completion invalidated",
                )
            })?;
            queue.push_front(follow);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    struct TestClock {
        epoch: AtomicU64,
    }
    impl ClockSource for TestClock {
        fn sample(&self) -> io::Result<(LocalInstant, Reading)> {
            let now = clock::now_ns();
            Ok((
                LocalClock::new(1).unwrap().at(now),
                Domain::new(1, self.epoch.load(Ordering::SeqCst))
                    .unwrap()
                    .reading(now + 10_000_000_000),
            ))
        }
    }
    fn source() -> Arc<TestClock> {
        Arc::new(TestClock {
            epoch: AtomicU64::new(1),
        })
    }
    fn config(peer: Peer) -> Config {
        Config {
            domain: Domain::new(1, 1).unwrap(),
            bind_ip: "127.0.0.1".parse().unwrap(),
            ports: Ports {
                event: 0,
                general: 0,
            },
            peers: vec![peer],
        }
    }
    fn request(kind: u8, len: usize) -> Vec<u8> {
        let mut b = vec![0; len];
        b[0] = 0x10 | kind;
        b[1] = 2;
        b[2..4].copy_from_slice(&(len as u16).to_be_bytes());
        b[20..28].copy_from_slice(&2u64.to_be_bytes());
        b[28..30].copy_from_slice(&1u16.to_be_bytes());
        b[30..32].copy_from_slice(&42u16.to_be_bytes());
        b
    }
    async fn read_kind(socket: &UdpSocket, kind: u8) -> Vec<u8> {
        timeout(Duration::from_secs(2), async {
            let mut buffer = [0; 1024];
            loop {
                let (n, _) = socket.recv_from(&mut buffer).await.unwrap();
                if buffer[0] & 15 == kind {
                    break buffer[..n].to_vec();
                }
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn loopback_peer_delay_signaling_and_shutdown_release_both_ports() {
        let event = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.2".parse().unwrap(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let service = MasterService::bind(config(peer), source()).await.unwrap();
        let addresses = service.addresses();
        let pdelay = request(2, 54);
        event.send_to(&pdelay, addresses.0).await.unwrap();
        let response = read_kind(&event, 3).await;
        let follow = read_kind(&general, 0xa).await;
        assert_eq!(&response[44..54], &pdelay[20..30]);
        assert_eq!(&follow[30..32], &pdelay[30..32]);
        let mut signaling = request(0xc, 54);
        signaling[44..54].copy_from_slice(&[0, 4, 0, 6, 0xb0, 0, 0, 0, 0, 30]);
        general.send_to(&signaling, addresses.1).await.unwrap();
        let grant = read_kind(&general, 0xc).await;
        assert_eq!(&grant[44..50], &[0, 5, 0, 8, 0xb0, 0]);
        service.shutdown().await.unwrap();
        let _event_rebound = UdpSocket::bind(addresses.0).await.unwrap();
        let _general_rebound = UdpSocket::bind(addresses.1).await.unwrap();
    }
    #[tokio::test]
    async fn changing_clock_epoch_stops_service_instead_of_relabeling_time() {
        let event = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let general = UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let peer = Peer {
            ip: "127.0.0.2".parse().unwrap(),
            ports: Ports {
                event: event.local_addr().unwrap().port(),
                general: general.local_addr().unwrap().port(),
            },
        };
        let clock = source();
        let service = MasterService::bind(config(peer), clock.clone())
            .await
            .unwrap();
        clock.epoch.store(2, Ordering::SeqCst);
        timeout(Duration::from_secs(1), async {
            while !service.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(service.shutdown().await.is_err());
    }
    #[tokio::test]
    async fn occupied_port_is_an_error_and_never_reuses_another_service_socket() {
        let occupied = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut cfg = config(Peer {
            ip: "127.0.0.2".parse().unwrap(),
            ports: Ports {
                event: 12345,
                general: 12346,
            },
        });
        cfg.ports.general = occupied.local_addr().unwrap().port();
        assert!(MasterService::bind(cfg, source()).await.is_err());
    }
}
