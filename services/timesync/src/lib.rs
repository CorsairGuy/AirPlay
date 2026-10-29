//! Служба единых часов (аналог TimeSync у Apple): слушает PTP на портах
//! 319/320 и отдаёт плеерам смещение часов отправителя.
//!
//! Порты ниже 1024 требуют root или `CAP_NET_BIND_SERVICE`. Без них служба
//! работает вхолостую, и плеер играет по своим часам, без мультирума.

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
}

impl TimeSync {
    /// Служба без сокетов: для тестов и когда PTP не нужен.
    pub fn idle(self_id: u64) -> Self {
        Self {
            ptp: Arc::new(Mutex::new(Ptp::new(self_id))),
            general: Arc::new(Vec::new()),
            listening: false,
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

    pub fn set_peers(&self, peers: Vec<IpAddr>) {
        debug!(?peers, "PTP: участники");
        self.lock().set_peers(peers);
    }

    pub fn timeline(&self, clock_id: u64) -> Option<Timeline> {
        self.lock().timeline(clock_id)
    }

    pub fn latest(&self) -> Option<Timeline> {
        self.lock().latest()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ptp> {
        self.ptp.lock().unwrap_or_else(|e| e.into_inner())
    }

    async fn receive(self, socket: Arc<UdpSocket>) {
        let mut buf = [0u8; 256];
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
