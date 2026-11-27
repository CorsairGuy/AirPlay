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
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
const MAX_SAMPLE_AGE: Duration = Duration::from_secs(10);

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

/// Current NTP timestamp on the same monotonic timeline as timing replies.
/// Suitable for sender sync packets; does not jump when wall time changes.
pub fn now() -> u64 {
    u64::from_be_bytes(ns_to_ntp(local_ns()))
}

fn read_ntp(b: &[u8]) -> u64 {
    ntp_to_ns(u64::from_be_bytes(b[..8].try_into().unwrap()))
}

fn request() -> [u8; 32] {
    request_with_sequence(7)
}

/// AirPlay timing request on the same clock used by timing responses.
pub fn request_with_sequence(sequence: u16) -> [u8; 32] {
    let mut req = [0u8; 32];
    req[..2].copy_from_slice(&[0x80, 0x80 | REQUEST]);
    req[2..4].copy_from_slice(&sequence.to_be_bytes());
    req[24..32].copy_from_slice(&ns_to_ntp(local_ns()));
    req
}

/// Ответ на запрос: его время отправки — в «origin», наше — в «receive» и
/// «transmit».
pub fn reply(req: &[u8]) -> Option<[u8; 32]> {
    if req.len() < 32 || req[0] != 0x80 || req[1] & 0x7F != REQUEST {
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
    last_received: Option<Instant>,
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
        let t1 = i128::from(read_ntp(&packet[8..16]));
        let t2 = i128::from(read_ntp(&packet[16..24]));
        let t3 = i128::from(read_ntp(&packet[24..32]));
        let t4 = i128::from(t4);
        if t2 == 0 || t3 == 0 || t3 < t2 {
            return;
        }
        let rtt = (t4 - t1) - (t3 - t2);
        if rtt < 0 {
            return;
        }
        let offset = ((t2 - t1) + (t3 - t4)) / 2;
        let (Ok(rtt), Ok(offset)) = (i64::try_from(rtt), i64::try_from(offset)) else {
            return;
        };
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.last_received
            .is_some_and(|at| at.elapsed() > MAX_SAMPLE_AGE)
        {
            s.recent.clear();
        }
        if s.recent.len() == SAMPLES {
            s.recent.remove(0);
        }
        s.recent.push((rtt, offset));
        s.last_received = Some(Instant::now());
        debug!(rtt_us = rtt / 1000, offset_ms = offset / 1_000_000, "NTP");
    }

    /// «Его NTP − наше NTP», нс.
    pub fn ntp_offset(&self) -> Option<i64> {
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !s
            .last_received
            .is_some_and(|at| at.elapsed() <= MAX_SAMPLE_AGE)
        {
            return None;
        }
        s.recent.iter().min_by_key(|(rtt, _)| *rtt).map(|(_, o)| *o)
    }
}

impl TimeSource for NtpClock {
    /// Смещение относительно монотонных часов плеера.
    fn offset(&self, timeline: u64) -> Option<i64> {
        (timeline == NTP_TIMELINE)
            .then(|| self.ntp_offset())?
            .and_then(|o| o.checked_add(base_ns()))
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
        let mut outstanding: Option<([u8; 8], Instant)> = None;
        loop {
            tokio::select! {
                r = socket.recv_from(&mut buf) => match r {
                    Ok((n, from)) => {
                        let packet = &buf[..n];
                        if from.ip().to_canonical() != remote.ip().to_canonical() { continue; }
                        if let Some(resp) = reply(packet) {
                            let _ = socket.send_to(&resp, from).await;
                        } else if n >= 32 && packet[0] == 0x80 && packet[1] & 0x7F == REPLY
                            && from.port() == remote.port()
                            && outstanding.as_ref().is_some_and(|(origin, sent)|
                                packet[8..16] == origin[..] && sent.elapsed() <= Duration::from_secs(10)) {
                            outstanding = None;
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
                        let packet = request();
                        let origin = packet[24..32].try_into().expect("NTP timestamp length");
                        if socket.send_to(&packet, remote).await.is_ok() {
                            outstanding = Some((origin, Instant::now()));
                        }
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

    #[tokio::test]
    async fn timing_accepts_only_reply_to_outstanding_request_from_configured_peer() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let stranger = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (port, clock, task) = start(server.local_addr().unwrap()).unwrap();
        let mut request = [0; 128];
        let (n, client) =
            tokio::time::timeout(Duration::from_secs(2), server.recv_from(&mut request))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(client.port(), port);
        let valid = reply(&request[..n]).unwrap();
        stranger.send_to(&valid, client).await.unwrap();
        let mut wrong = valid;
        wrong[8] ^= 1;
        server.send_to(&wrong, client).await.unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(clock.ntp_offset().is_none());
        server.send_to(&valid, client).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while clock.ntp_offset().is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        task.abort();
    }

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
    fn stale_ntp_samples_stop_providing_a_mapping() {
        let clock = NtpClock::default();
        let mut state = clock.0.lock().unwrap();
        state.recent.push((1, 2));
        state.last_received = Some(Instant::now() - MAX_SAMPLE_AGE - Duration::from_secs(1));
        drop(state);
        assert!(clock.ntp_offset().is_none());
    }

    #[test]
    fn ntp_format_roundtrip() {
        let ns = 3_900_000_000_123_456_789u64;
        let back = ntp_to_ns(u64::from_be_bytes(ns_to_ntp(ns)));
        assert!(back.abs_diff(ns) < 2, "{back}");
    }

    #[test]
    fn sender_timestamp_uses_the_timing_reply_clock() {
        let req = request();
        let before = now();
        let resp = reply(&req).unwrap();
        let after = now();
        let received = u64::from_be_bytes(resp[16..24].try_into().unwrap());
        let transmitted = u64::from_be_bytes(resp[24..32].try_into().unwrap());
        // Wrapping differences also work across the NTP seconds rollover.
        let span = after.wrapping_sub(before);
        assert!(received.wrapping_sub(before) <= span);
        assert!(transmitted.wrapping_sub(before) <= span);
    }

    /// Дубли и переупорядоченные ответы не портят смещение: берётся замер с
    /// наименьшим RTT, а не последний пришедший.
    #[test]
    fn duplicate_and_reordered_replies_keep_the_best_sample() {
        let clock = NtpClock::default();
        let t1 = local_ns();
        let ahead = 5_000_000_000;
        let mut good = [0u8; 32];
        good[8..16].copy_from_slice(&ns_to_ntp(t1));
        good[16..24].copy_from_slice(&ns_to_ntp(t1 + 1_000_000 + ahead));
        good[24..32].copy_from_slice(&ns_to_ntp(t1 + 1_000_000 + ahead));
        clock.on_reply(&good, t1 + 2_000_000);
        clock.on_reply(&good, t1 + 2_000_000); // дубль

        // Более поздний, но медленный замер: 500 мс RTT и смещение 30 с.
        let mut slow = [0u8; 32];
        slow[8..16].copy_from_slice(&ns_to_ntp(t1));
        slow[16..24].copy_from_slice(&ns_to_ntp(t1 + 250_000_000 + 30_000_000_000));
        slow[24..32].copy_from_slice(&ns_to_ntp(t1 + 250_000_000 + 30_000_000_000));
        clock.on_reply(&slow, t1 + 500_000_000);

        let offset = clock.ntp_offset().unwrap();
        assert!((offset - ahead as i64).abs() < 1000, "выбран медленный замер: {offset}");
    }

    /// Нулевые, обратные и обрезанные метки не создают отображение.
    #[test]
    fn malformed_replies_do_not_create_a_mapping() {
        let clock = NtpClock::default();
        let t1 = local_ns();
        let mut resp = [0u8; 32];
        resp[8..16].copy_from_slice(&ns_to_ntp(t1));
        resp[16..24].copy_from_slice(&ns_to_ntp(t1 + 1_000_000));
        resp[24..32].copy_from_slice(&ns_to_ntp(t1 + 1_000_000));

        clock.on_reply(&resp[..31], t1 + 2_000_000);
        assert!(clock.ntp_offset().is_none(), "обрезанный ответ");

        let mut zero = resp;
        zero[16..24].fill(0);
        clock.on_reply(&zero, t1 + 2_000_000);
        assert!(clock.ntp_offset().is_none(), "нулевое receive");

        let mut backward = resp;
        backward[24..32].copy_from_slice(&ns_to_ntp(t1 - 1_000_000));
        clock.on_reply(&backward, t1 + 2_000_000);
        assert!(clock.ntp_offset().is_none(), "transmit раньше receive");

        let mut reversed = resp;
        reversed[16..24].copy_from_slice(&ns_to_ntp(t1 + 3_000_000));
        reversed[24..32].copy_from_slice(&ns_to_ntp(t1 + 1_000_000));
        clock.on_reply(&reversed, t1 + 4_000_000);
        assert!(clock.ntp_offset().is_none(), "t3 < t2");

        clock.on_reply(&resp, t1.saturating_sub(1_000)); // ответ раньше запроса: RTT < 0
        assert!(clock.ntp_offset().is_none());
    }

    /// После истечения срока старые замеры отбрасываются, а новый ответ
    /// снова даёт отображение.
    #[test]
    fn an_expired_source_can_return_with_a_fresh_sample() {
        let clock = NtpClock::default();
        {
            let mut state = clock.0.lock().unwrap();
            state.recent.push((1, 42));
            state.last_received =
                Some(Instant::now() - MAX_SAMPLE_AGE - Duration::from_secs(1));
        }
        assert!(clock.ntp_offset().is_none());

        let t1 = local_ns();
        let mut resp = [0u8; 32];
        resp[8..16].copy_from_slice(&ns_to_ntp(t1));
        resp[16..24].copy_from_slice(&ns_to_ntp(t1 + 2_000_000));
        resp[24..32].copy_from_slice(&ns_to_ntp(t1 + 2_000_000));
        clock.on_reply(&resp, t1 + 4_000_000);
        assert!(clock.ntp_offset().is_some());
    }

    /// Максимальная 64-битная метка NTP переводится без переполнения.
    #[test]
    fn maximum_ntp_timestamp_does_not_overflow() {
        let ns = ntp_to_ns(u64::MAX);
        let secs = u64::MAX >> 32;
        let frac = (u64::MAX & 0xFFFF_FFFF) as u128;
        let expected = secs * 1_000_000_000 + ((frac * 1_000_000_000) >> 32) as u64;
        assert_eq!(ns, expected);
        assert!(ns < u64::MAX);
    }

    /// Ответ на запрос сохраняет его origin и не принимает чужой тип пакета.
    #[test]
    fn reply_requires_an_exact_timing_request() {
        let req = request();
        let resp = reply(&req).unwrap();
        assert_eq!(resp[8..16], req[24..32]);
        for len in 0..32 {
            assert!(reply(&req[..len]).is_none(), "len={len}");
        }
        let mut wrong = req;
        wrong[0] = 0x81;
        assert!(reply(&wrong).is_none());
        wrong[0] = 0x80;
        wrong[1] = 0x80 | REPLY;
        assert!(reply(&wrong).is_none());
    }

    /// Симметричный обмен: t2=t3=t1+offset+q, t4=t1+rtt, q=rtt/2 — тогда
    /// формула `((t2-t1)+(t3-t4))/2` даёт ровно `offset`.
    fn sample_reply(t1: u64, offset: i64, rtt: i64) -> [u8; 32] {
        let mut resp = [0u8; 32];
        resp[8..16].copy_from_slice(&ns_to_ntp(t1));
        let t2 = t1 as i64 + offset + rtt / 2;
        resp[16..24].copy_from_slice(&ns_to_ntp(t2 as u64));
        resp[24..32].copy_from_slice(&ns_to_ntp(t2 as u64));
        resp
    }

    #[test]
    fn stale_sample_stops_the_mapping_and_a_fresh_reply_restores_it() {
        let clock = NtpClock::default();
        let t1 = local_ns();
        let resp = sample_reply(t1, 5_000_000_000, 200_000);
        clock.on_reply(&resp, t1 + 200_000);
        assert!(clock.ntp_offset().is_some());
        assert!(clock.offset(NTP_TIMELINE).is_some());

        // Срок давности истёк: формальное наличие значения не заменяет свежесть.
        {
            let mut state = clock.0.lock().unwrap();
            state.last_received =
                Some(Instant::now() - MAX_SAMPLE_AGE - Duration::from_millis(1));
        }
        assert!(clock.ntp_offset().is_none());
        assert!(clock.offset(NTP_TIMELINE).is_none(), "устаревший mapping не отдаётся плееру");

        // Новый ответ очищает историю и снова даёт mapping.
        let t1 = local_ns();
        let resp = sample_reply(t1, 7_000_000_000, 200_000);
        clock.on_reply(&resp, t1 + 200_000);
        let offset = clock.ntp_offset().unwrap();
        assert!((offset - 7_000_000_000).abs() < 1_000, "{offset}");
        assert_eq!(clock.0.lock().unwrap().recent.len(), 1, "старые сэмплы сброшены");
    }

    #[test]
    fn the_lowest_rtt_sample_wins() {
        let clock = NtpClock::default();
        let t1 = local_ns();
        // Первый сэмпл: смещение 1 мс, RTT 2 мс.
        let slow = sample_reply(t1, 1_000_000, 2_000_000);
        clock.on_reply(&slow, t1 + 2_000_000);
        // Второй: смещение 5 мс, RTT близок к нулю — он точнее.
        let fast = sample_reply(t1, 5_000_000, 100_000);
        clock.on_reply(&fast, t1 + 100_000);
        let offset = clock.ntp_offset().unwrap();
        assert!((offset - 5_000_000).abs() < 1_000, "{offset}");
    }

    #[test]
    fn replies_with_zero_or_reversed_server_time_are_ignored() {
        let clock = NtpClock::default();
        let t1 = local_ns();
        let mut zero = sample_reply(t1, 0, 100_000);
        zero[16..24].fill(0);
        clock.on_reply(&zero, t1 + 100_000);
        assert!(clock.ntp_offset().is_none(), "t2=0 не принимается");

        let mut reversed = sample_reply(t1, 0, 100_000);
        reversed[24..32].copy_from_slice(&ns_to_ntp(t1.saturating_sub(1_000_000)));
        clock.on_reply(&reversed, t1 + 100_000);
        assert!(clock.ntp_offset().is_none(), "t3<t2 не принимается");

        let short = [0u8; 16];
        clock.on_reply(&short, t1);
        assert!(clock.ntp_offset().is_none());
    }

    #[test]
    fn only_the_ntp_timeline_is_mapped() {
        let clock = NtpClock::default();
        let t1 = local_ns();
        let resp = sample_reply(t1, 3_000_000_000, 100_000);
        clock.on_reply(&resp, t1 + 100_000);
        assert!(clock.offset(NTP_TIMELINE).is_some());
        for timeline in [0, 1, 2, NTP_TIMELINE.wrapping_add(1)] {
            assert!(clock.offset(timeline).is_none(), "timeline {timeline}");
        }
    }
}
