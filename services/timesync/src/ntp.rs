//! Часы по NTP для зеркалирования (AirPlay 1): отправитель ждёт обмена
//! метками времени, прежде чем начать поток. Мы спрашиваем его время раз в
//! 3 с и отвечаем на его запросы, как UxPlay.
//!
//! Свои метки берём из монотонных часов [`clock::now_ns`], сдвинутых к
//! настоящему времени NTP при запуске: отправителю нужна только
//! непрерывная шкала, а нам — смещение его часов относительно той же
//! шкалы, по которой играет плеер.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use clock::{NTP_TIMELINE, ntp_to_ns};
use clock::{TimeSource, now_ns};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tracing::debug;

/// Секунды между эпохами NTP (1900) и UNIX (1970).
const NTP_UNIX_OFFSET: u64 = 2_208_988_800;
const REQUEST: u8 = 0x52;
const REPLY: u8 = 0x53;
/// Сколько последних замеров держим; берём замер с наименьшей задержкой.
const SAMPLES: usize = 8;

/// Сдвиг монотонных часов в «время NTP», постоянный с момента запуска.
fn base_ns() -> i64 {
    static BASE: OnceLock<i64> = OnceLock::new();
    *BASE.get_or_init(|| {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let wall_ns =
            (wall.as_secs() + NTP_UNIX_OFFSET) * 1_000_000_000 + wall.subsec_nanos() as u64;
        wall_ns as i64 - now_ns() as i64
    })
}

/// Наше «время NTP», нс: монотонные часы плюс сдвиг.
fn local_ns() -> u64 {
    (base_ns() + now_ns() as i64) as u64
}

fn ns_to_ntp(ns: u64) -> [u8; 8] {
    let secs = ns / 1_000_000_000;
    let frac = ((ns % 1_000_000_000) << 32) / 1_000_000_000;
    ((secs << 32) | frac).to_be_bytes()
}

fn read_ntp(b: &[u8]) -> u64 {
    ntp_to_ns(u64::from_be_bytes(b[..8].try_into().unwrap()))
}

fn request() -> [u8; 32] {
    let mut req = [0u8; 32];
    req[..4].copy_from_slice(&[0x80, 0x80 | REQUEST, 0x00, 0x07]);
    req[24..32].copy_from_slice(&ns_to_ntp(local_ns()));
    req
}

/// Ответ на запрос: его время отправки — в «origin», наше — в «receive» и
/// «transmit».
pub fn reply(req: &[u8]) -> Option<[u8; 32]> {
    if req.len() < 32 || req[1] & 0x7F != REQUEST {
        return None;
    }
    let mut resp = [0u8; 32];
    resp.copy_from_slice(&req[..32]);
    resp[1] = 0x80 | REPLY;
    resp[8..16].copy_from_slice(&req[24..32]);
    let now = ns_to_ntp(local_ns());
    resp[16..24].copy_from_slice(&now);
    resp[24..32].copy_from_slice(&now);
    Some(resp)
}

#[derive(Default)]
struct Samples {
    /// (задержка туда-обратно, смещение «его NTP − наше NTP»), нс.
    recent: Vec<(i64, i64)>,
}

/// Смещение часов отправителя, по которому плеер ставит звук
/// зеркалирования.
#[derive(Clone, Default)]
pub struct NtpClock(Arc<Mutex<Samples>>);

impl NtpClock {
    /// Ответ отправителя на наш запрос (`t4` — когда пришёл, наше NTP, нс).
    fn on_reply(&self, packet: &[u8], t4: u64) {
        if packet.len() < 32 {
            return;
        }
        let t1 = read_ntp(&packet[8..16]) as i64;
        let t2 = read_ntp(&packet[16..24]) as i64;
        let t3 = read_ntp(&packet[24..32]) as i64;
        let t4 = t4 as i64;
        let rtt = (t4 - t1) - (t3 - t2);
        if rtt < 0 {
            return;
        }
        let offset = ((t2 - t1) + (t3 - t4)) / 2;
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.recent.len() == SAMPLES {
            s.recent.remove(0);
        }
        s.recent.push((rtt, offset));
        debug!(rtt_us = rtt / 1000, offset_ms = offset / 1_000_000, "NTP");
    }

    /// «Его NTP − наше NTP», нс.
    pub fn ntp_offset(&self) -> Option<i64> {
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.recent.iter().min_by_key(|(rtt, _)| *rtt).map(|(_, o)| *o)
    }
}

impl TimeSource for NtpClock {
    /// Смещение относительно монотонных часов плеера.
    fn offset(&self, timeline: u64) -> Option<i64> {
        (timeline == NTP_TIMELINE)
            .then(|| self.ntp_offset())?
            .map(|o| o + base_ns())
    }
}

/// Обмен временем с отправителем, у которого порт времени `remote`.
/// Возвращает наш порт (его отдаём в `timingPort`), часы и задачу.
pub fn start(remote: SocketAddr) -> io::Result<(u16, NtpClock, JoinHandle<()>)> {
    let any = match remote.ip() {
        IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
    };
    let socket = std::net::UdpSocket::bind((any, 0))?;
    socket.set_nonblocking(true)?;
    let socket = UdpSocket::from_std(socket)?;
    let port = socket.local_addr()?.port();
    let clock = NtpClock::default();
    let task_clock = clock.clone();
    let task = tokio::spawn(async move {
        let mut buf = [0u8; 128];
        let mut tick = tokio::time::interval(Duration::from_secs(3));
        loop {
            tokio::select! {
                r = socket.recv_from(&mut buf) => match r {
                    Ok((n, from)) => {
                        let packet = &buf[..n];
                        if let Some(resp) = reply(packet) {
                            let _ = socket.send_to(&resp, from).await;
                        } else if n >= 32 && packet[1] & 0x7F == REPLY {
                            task_clock.on_reply(packet, local_ns());
                        }
                    }
                    Err(e) => {
                        debug!("NTP: {e}");
                        return;
                    }
                },
                _ = tick.tick() => {
                    if remote.port() != 0 {
                        let _ = socket.send_to(&request(), remote).await;
                    }
                }
            }
        }
    });
    Ok((port, clock, task))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_echoes_origin() {
        let req = request();
        let resp = reply(&req).unwrap();
        assert_eq!(resp[1] & 0x7F, REPLY);
        assert_eq!(resp[8..16], req[24..32]);
        assert!(reply(&resp).is_none());
    }

    #[test]
    fn offset_from_reply() {
        // Отправитель спешит на 5 с, путь в одну сторону — 1 мс.
        let clock = NtpClock::default();
        let t1 = local_ns();
        let ahead = 5_000_000_000;
        let mut resp = [0u8; 32];
        resp[8..16].copy_from_slice(&ns_to_ntp(t1));
        resp[16..24].copy_from_slice(&ns_to_ntp(t1 + 1_000_000 + ahead));
        resp[24..32].copy_from_slice(&ns_to_ntp(t1 + 1_000_000 + ahead));
        clock.on_reply(&resp, t1 + 2_000_000);
        let offset = clock.ntp_offset().unwrap();
        assert!((offset - ahead as i64).abs() < 1000, "{offset}");
        // Для плеера смещение — относительно монотонных часов.
        let mono = clock.offset(NTP_TIMELINE).unwrap();
        assert_eq!(mono, offset + base_ns());
        assert!(clock.offset(1).is_none());
    }

    #[test]
    fn ntp_format_roundtrip() {
        let ns = 3_900_000_000_123_456_789u64;
        let back = ntp_to_ns(u64::from_be_bytes(ns_to_ntp(ns)));
        assert!(back.abs_diff(ns) < 2, "{back}");
    }
}
