//! Служба единых часов (аналог TimeSync у Apple): слушает PTP на портах
//! 319/320 и отдаёт плеерам смещение часов отправителя.
//!
//! Порты ниже 1024 требуют root или `CAP_NET_BIND_SERVICE`. Без них служба
//! работает вхолостую, и плеер играет по своим часам, без мультирума.

pub mod engine;
pub mod external_media;
mod host;
pub mod media;
pub mod ntp;
pub mod peer_interests;
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod raw_correlation;
pub mod sender;
pub mod service;

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clock::ptp::{EVENT_PORT, GENERAL_PORT};
use clock::{Action, Ptp, Timeline, now_ns};
use tokio::net::UdpSocket;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct TimeSync {
    ptp: Arc<Mutex<Ptp>>,
    general: Arc<Vec<Arc<UdpSocket>>>,
    listening: bool,
    host: Option<Arc<host::Host>>,
    interests: Arc<Mutex<BTreeMap<u128, Vec<IpAddr>>>>,
}

impl TimeSync {
    /// Служба без сокетов: для тестов и когда PTP не нужен.
    pub fn idle(self_id: u64) -> Self {
        Self {
            ptp: Arc::new(Mutex::new(Ptp::new(self_id))),
            general: Arc::new(Vec::new()),
            listening: false,
            host: None,
            interests: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// Открывает порты PTP и запускает приём. Вызывать внутри рантайма tokio.
    pub fn start(self_id: u64) -> Self {
        let mut this = Self::idle(self_id);
        let mut sockets = Vec::new();
        let mut general = Vec::new();
        // Сначала IPv6: на Linux такой сокет принимает и IPv4, и тогда привязка
        // 0.0.0.0 ниже просто не нужна. На Windows сокеты раздельные.
        for ip in [
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        ] {
            for port in [EVENT_PORT, GENERAL_PORT] {
                match bind(SocketAddr::new(ip, port)) {
                    Ok(s) => {
                        let s = Arc::new(s);
                        if port == GENERAL_PORT {
                            general.push(s.clone());
                        }
                        sockets.push(s);
                    }
                    Err(e) => debug!(%ip, port, "PTP: {e}"),
                }
            }
        }
        if sockets.is_empty() {
            warn!(
                "PTP: порты 319/320 недоступны (нужен root или CAP_NET_BIND_SERVICE), \
                 играем без синхронизации с отправителем"
            );
            return this;
        }
        info!(sockets = sockets.len(), "PTP: слушаю 319/320");
        this.general = Arc::new(general);
        this.listening = true;
        for s in sockets {
            let this = this.clone();
            tokio::spawn(async move { this.receive(s).await });
        }
        this
    }

    pub fn is_listening(&self) -> bool {
        self.listening
    }

    /// Explicit interface and one socket pair for combined RX/TX clock roles.
    pub async fn start_host(self_id: u64, ip: IpAddr) -> std::io::Result<Self> {
        let host = host::Host::bind(self_id, ip).await?;
        let mut this = Self::idle(self_id);
        this.host = Some(Arc::new(host));
        this.listening = true;
        Ok(this)
    }

    pub fn selected_reading(&self) -> Option<clock::domain::Reading> {
        self.host.as_ref()?.reading()
    }

    pub fn set_session_peers(&self, owner: u128, peers: Vec<IpAddr>) {
        if let Some(host) = &self.host {
            host.set_peers(owner, peers);
        } else {
            self.set_listener_peers(owner, peers);
        }
    }
    pub fn release_session_peers(&self, owner: u128) {
        if let Some(host) = &self.host {
            host.set_peers(owner, vec![]);
        } else {
            self.set_listener_peers(owner, vec![]);
        }
    }

    pub fn set_peers(&self, peers: Vec<IpAddr>) {
        if let Some(host) = &self.host {
            host.set_peers(0, peers);
            return;
        }
        self.set_listener_peers(0, peers);
    }

    fn set_listener_peers(&self, owner: u128, peers: Vec<IpAddr>) {
        let mut interests = self.interests.lock().unwrap_or_else(|e| e.into_inner());
        if peers.is_empty() {
            interests.remove(&owner);
        } else {
            interests.insert(owner, peers);
        }
        let peers: Vec<_> = interests
            .values()
            .flatten()
            .map(|ip| ip.to_canonical())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        debug!(?peers, "PTP: участники");
        self.lock().set_peers(peers);
    }

    pub fn timeline(&self, clock_id: u64) -> Option<Timeline> {
        if self.uses_selected_reading() {
            return self.latest().filter(|t| t.clock_id == clock_id);
        }
        self.lock().timeline(clock_id)
    }

    pub fn latest(&self) -> Option<Timeline> {
        if self.uses_selected_reading() {
            let reading = self.selected_reading()?;
            let now = clock::now_ns();
            return Some(Timeline {
                clock_id: reading.domain().id(),
                offset_ns: i64::try_from(i128::from(reading.ns()) - i128::from(now)).ok()?,
                updated_ns: now,
            });
        }
        self.lock().latest()
    }

    fn uses_selected_reading(&self) -> bool {
        self.host.is_some()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ptp> {
        self.ptp.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn receive(self, socket: Arc<UdpSocket>) {
        let mut buf = [0u8; 256];
        let mut observed_timeline = None;
        loop {
            let (n, from) = match socket.recv_from(&mut buf).await {
                Ok(r) => r,
                Err(e) => {
                    debug!("PTP: приём: {e}");
                    continue;
                }
            };
            let local = now_ns();
            let actions = self
                .lock()
                .on_packet(from.ip().to_canonical(), &buf[..n], local);
            if let Some(timeline) = self.latest()
                && observed_timeline != Some(timeline.clock_id)
            {
                observed_timeline = Some(timeline.clock_id);
                info!(
                    clock_id = timeline.clock_id,
                    offset_ns = timeline.offset_ns,
                    "PTP: sender timeline observed"
                );
            }
            for Action::Send { to, data, delay_ms } in actions {
                let this = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    this.send_general(to, &data).await;
                });
            }
        }
    }

    async fn send_general(&self, to: IpAddr, data: &[u8]) {
        let same_family = self
            .general
            .iter()
            .find(|s| s.local_addr().is_ok_and(|a| a.is_ipv4() == to.is_ipv4()));
        let (socket, target) = match (same_family, to) {
            (Some(s), _) => (s, SocketAddr::new(to, GENERAL_PORT)),
            // Двухстековый IPv6-сокет шлёт на IPv4 через адрес вида ::ffff:a.b.c.d.
            (None, IpAddr::V4(v4)) => match self.general.first() {
                Some(s) => (
                    s,
                    SocketAddr::new(IpAddr::V6(v4.to_ipv6_mapped()), GENERAL_PORT),
                ),
                None => return,
            },
            (None, IpAddr::V6(_)) => return,
        };
        if let Err(e) = socket.send_to(data, target).await {
            debug!(%target, "PTP: отправка: {e}");
        }
    }
}

fn bind(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let s = std::net::UdpSocket::bind(addr)?;
    s.set_nonblocking(true)?;
    UdpSocket::from_std(s)
}

impl clock::TimeSource for TimeSync {
    fn offset(&self, timeline: u64) -> Option<i64> {
        self.timeline(timeline).map(|t| t.offset_ns)
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[test]
    fn removing_one_session_keeps_the_other_peers_active() {
        let time = TimeSync::idle(1);
        let first: IpAddr = "192.0.2.1".parse().unwrap();
        let second: IpAddr = "192.0.2.2".parse().unwrap();
        time.set_session_peers(11, vec![first]);
        time.set_session_peers(22, vec![second]);
        time.release_session_peers(11);
        let mut announce = [0u8; 64];
        announce[0] = 0x0b;
        announce[1] = 2;
        announce[20..28].copy_from_slice(&2u64.to_be_bytes());
        announce[53..61].copy_from_slice(&2u64.to_be_bytes());
        announce[47] = 128;
        announce[52] = 128;
        for _ in 0..3 {
            assert!(time.lock().on_packet(first, &announce, 1).is_empty());
        }
        assert!(time.lock().on_packet(second, &announce, 1).is_empty());
        assert!(time.lock().on_packet(second, &announce, 2).is_empty());
        assert!(!time.lock().on_packet(second, &announce, 3).is_empty());
    }
}
