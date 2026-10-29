//! Разбор PTP (IEEE 1588-2008/2019) в объёме, нужном приёмнику AirPlay 2.

use std::collections::HashMap;
use std::net::IpAddr;

use crate::OffsetFilter;

pub const EVENT_PORT: u16 = 319;
pub const GENERAL_PORT: u16 = 320;

const HEADER_LEN: usize = 34;
const SYNC: u8 = 0x0;
const FOLLOW_UP: u8 = 0x8;
const ANNOUNCE: u8 = 0xB;
/// После стольких Announce без Follow_Up часы считаем уснувшими.
const SILENT_ANNOUNCES: u32 = 3;

/// Что нужно сделать службе после очередного пакета.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Отправить `data` на `to`:`GENERAL_PORT` через `delay_ms`.
    Send {
        to: IpAddr,
        data: Vec<u8>,
        delay_ms: u64,
    },
}

/// Шкала времени мастера: `мастер = локальные + offset_ns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeline {
    pub clock_id: u64,
    pub offset_ns: i64,
    /// Локальное время последней выборки, нс.
    pub updated_ns: u64,
}

#[derive(Debug, Default)]
struct Source {
    clock_id: u64,
    grandmaster: u64,
    priority1: u8,
    priority2: u8,
    filter: OffsetFilter,
    updated_ns: u64,
    silent_announces: u32,
}

#[derive(Debug, Clone, Copy)]
struct Header {
    kind: u8,
    correction_ns: i64,
    clock_id: u64,
}

fn header(data: &[u8]) -> Option<Header> {
    if data.len() < HEADER_LEN || data[1] & 0x0f != 2 {
        return None;
    }
    let correction = i64::from_be_bytes(data[8..16].try_into().ok()?);
    Some(Header {
        kind: data[0] & 0x0f,
        correction_ns: correction >> 16,
        clock_id: u64::from_be_bytes(data[20..28].try_into().ok()?),
    })
}

/// 48 бит секунд и 32 бита наносекунд, как в поле Timestamp PTP.
fn timestamp(b: &[u8]) -> Option<u64> {
    let b: &[u8; 10] = b.get(..10)?.try_into().ok()?;
    let secs = u64::from_be_bytes([0, 0, b[0], b[1], b[2], b[3], b[4], b[5]]);
    let nanos = u32::from_be_bytes([b[6], b[7], b[8], b[9]]) as u64;
    Some(secs * 1_000_000_000 + nanos)
}

/// Состояние PTP-слушателя: по одному источнику на адрес отправителя.
#[derive(Debug)]
pub struct Ptp {
    self_id: u64,
    peers: Vec<IpAddr>,
    sources: HashMap<IpAddr, Source>,
}

impl Ptp {
    /// `self_id` — наш Clock ID для «будящих» Announce (обычно из MAC).
    pub fn new(self_id: u64) -> Self {
        Self {
            self_id,
            peers: Vec::new(),
            sources: HashMap::new(),
        }
    }

    /// Участники из SETPEERS. Пока их нет, принимаем пакеты от всех.
    pub fn set_peers(&mut self, peers: Vec<IpAddr>) {
        self.sources.retain(|ip, _| peers.contains(ip));
        self.peers = peers;
    }

    pub fn on_packet(&mut self, from: IpAddr, data: &[u8], local_ns: u64) -> Vec<Action> {
        if !self.peers.is_empty() && !self.peers.contains(&from) {
            return Vec::new();
        }
        let Some(h) = header(data) else {
            return Vec::new();
        };
        match h.kind {
            ANNOUNCE => self.announce(from, &h, data),
            FOLLOW_UP => {
                self.follow_up(from, &h, data, local_ns);
                Vec::new()
            }
            // Sync двухшаговых часов несёт нулевое время — всё нужное в Follow_Up.
            SYNC => Vec::new(),
            _ => Vec::new(),
        }
    }

    fn announce(&mut self, from: IpAddr, h: &Header, data: &[u8]) -> Vec<Action> {
        if data.len() < 64 {
            return Vec::new();
        }
        let src = self.sources.entry(from).or_default();
        if src.clock_id != h.clock_id {
            src.filter.reset();
        }
        src.clock_id = h.clock_id;
        src.priority1 = data[47];
        src.priority2 = data[52];
        src.grandmaster = u64::from_be_bytes(data[53..61].try_into().unwrap_or_default());
        src.silent_announces += 1;
        if src.silent_announces != SILENT_ANNOUNCES || src.clock_id != src.grandmaster {
            return Vec::new();
        }
        // Как nqptp: сначала объявляем себя чуть лучше мастера, затем чуть хуже.
        // Это заставляет часы отправителя снова начать рассылать Sync.
        let (p1, p2) = (src.priority1, src.priority2);
        vec![
            Action::Send {
                to: from,
                data: self.announce_message(p1.saturating_sub(1).max(1), p2),
                delay_ms: 0,
            },
            Action::Send {
                to: from,
                data: self.announce_message(p1.saturating_add(1).min(254), p2),
                delay_ms: 150,
            },
        ]
    }

    fn follow_up(&mut self, from: IpAddr, h: &Header, data: &[u8], local_ns: u64) {
        let Some(origin) = data.get(HEADER_LEN..).and_then(timestamp) else {
            return;
        };
        let Some(src) = self.sources.get_mut(&from) else {
            return; // ждём Announce, чтобы знать, чьи это часы
        };
        if src.clock_id != h.clock_id {
            return;
        }
        src.silent_announces = 0;
        let master = origin as i64 + h.correction_ns;
        src.filter.update(master - local_ns as i64);
        src.updated_ns = local_ns;
    }

    /// Смещение шкалы `clock_id` (networkTimeTimelineID из SETRATEANCHORTIME).
    /// Подходит и источник, у которого этот Clock ID — гроссмейстер.
    pub fn timeline(&self, clock_id: u64) -> Option<Timeline> {
        self.sources
            .values()
            .filter(|s| s.clock_id == clock_id || s.grandmaster == clock_id)
            .filter_map(|s| {
                Some(Timeline {
                    clock_id,
                    offset_ns: s.filter.offset()?,
                    updated_ns: s.updated_ns,
                })
            })
            .max_by_key(|t| t.updated_ns)
    }

    /// Самая свежая шкала, если отправитель ещё не назвал свою.
    pub fn latest(&self) -> Option<Timeline> {
        self.sources
            .values()
            .filter_map(|s| {
                Some(Timeline {
                    clock_id: s.clock_id,
                    offset_ns: s.filter.offset()?,
                    updated_ns: s.updated_ns,
                })
            })
            .max_by_key(|t| t.updated_ns)
    }

    fn announce_message(&self, priority1: u8, priority2: u8) -> Vec<u8> {
        let mut m = vec![0u8; 64];
        m[0] = 0x10 | ANNOUNCE;
        m[1] = 0x02;
        m[2..4].copy_from_slice(&64u16.to_be_bytes());
        m[6..8].copy_from_slice(&0x0408u16.to_be_bytes());
        m[20..28].copy_from_slice(&self.self_id.to_be_bytes());
        m[28..30].copy_from_slice(&32776u16.to_be_bytes());
        m[32] = 0x05;
        m[33] = 0xFE;
        m[44..46].copy_from_slice(&37u16.to_be_bytes());
        m[47] = priority1;
        m[48..52].copy_from_slice(&0xf8fe_436au32.to_be_bytes());
        m[52] = priority2;
        m[53..61].copy_from_slice(&self.self_id.to_be_bytes());
        m[63] = 0xA0; // внутренний генератор
        m
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const IPHONE: u64 = 0x1122_3344_5566_7788;

    pub fn packet(kind: u8, clock: u64, body_ts: u64, correction_ns: i64) -> Vec<u8> {
        let mut m = vec![0u8; if kind == ANNOUNCE { 64 } else { 44 }];
        m[0] = 0x10 | kind;
        m[1] = 0x02;
        m[8..16].copy_from_slice(&(correction_ns << 16).to_be_bytes());
        m[20..28].copy_from_slice(&clock.to_be_bytes());
        let secs = body_ts / 1_000_000_000;
        m[34..40].copy_from_slice(&secs.to_be_bytes()[2..]);
        m[40..44].copy_from_slice(&((body_ts % 1_000_000_000) as u32).to_be_bytes());
        if kind == ANNOUNCE {
            m[47] = 248;
            m[52] = 248;
            m[53..61].copy_from_slice(&clock.to_be_bytes());
        }
        m
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([192, 168, 1, last])
    }

    #[test]
    fn follow_up_gives_offset() {
        let mut p = Ptp::new(7);
        assert!(p.timeline(IPHONE).is_none());
        p.on_packet(ip(5), &packet(ANNOUNCE, IPHONE, 0, 0), 0);
        // Мастер на 1000 с впереди, коррекция +250 нс.
        for i in 0..10u64 {
            let local = 5_000_000_000 + i * 125_000_000;
            let master = local + 1_000_000_000_000;
            p.on_packet(ip(5), &packet(SYNC, IPHONE, 0, 0), local);
            p.on_packet(ip(5), &packet(FOLLOW_UP, IPHONE, master, 250), local);
        }
        let t = p.timeline(IPHONE).unwrap();
        assert_eq!(t.offset_ns, 1_000_000_000_250);
        assert_eq!(p.latest().unwrap().clock_id, IPHONE);
    }

    #[test]
    fn follow_up_before_announce_is_ignored() {
        let mut p = Ptp::new(7);
        p.on_packet(ip(5), &packet(FOLLOW_UP, IPHONE, 10, 0), 0);
        assert!(p.latest().is_none());
    }

    #[test]
    fn peers_filter_sources() {
        let mut p = Ptp::new(7);
        p.set_peers(vec![ip(9)]);
        p.on_packet(ip(5), &packet(ANNOUNCE, IPHONE, 0, 0), 0);
        p.on_packet(ip(5), &packet(FOLLOW_UP, IPHONE, 10, 0), 0);
        assert!(p.latest().is_none());
    }

    #[test]
    fn silent_clock_gets_awakened() {
        let mut p = Ptp::new(7);
        let mut actions = Vec::new();
        for _ in 0..SILENT_ANNOUNCES {
            actions = p.on_packet(ip(5), &packet(ANNOUNCE, IPHONE, 0, 0), 0);
        }
        assert_eq!(actions.len(), 2);
        let Action::Send { to, data, delay_ms } = &actions[1];
        assert_eq!((*to, *delay_ms, data[47]), (ip(5), 150, 249));
        let Action::Send { data, .. } = &actions[0];
        assert_eq!(data[47], 247);
        assert_eq!(&data[53..61], &7u64.to_be_bytes());
    }
}
