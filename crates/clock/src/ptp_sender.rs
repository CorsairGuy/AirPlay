//! Experimental fixed-master PTP protocol core, without sockets or threads.
//!
//! Not a BMCA implementation: any configured peer advertising another clock
//! inhibits transmission. A future adapter must resolve clock ownership, sample
//! real egress/ingress times and complete unicast negotiation before LAN use.
//! Wire profile: Music Assistant ap2_ptp.c at 0a58c239 (Apache-2.0), checked
//! against OwnTone. No changes to the existing receiver Ptp or NTP transport.
use crate::domain::{Domain, Reading};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
};

const FLAGS: u16 = 0x0408;
const TWO_STEP: u16 = 0x0200;
const SYNC_NS: u64 = 125_000_000;
const ANNOUNCE_NS: u64 = 1_000_000_000;
const MAX_PEERS: usize = 64;
static INSTANCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidIdentity,
    InvalidPeers,
    TimeReversed,
    Exhausted,
    ForeignClock,
    InvalidRate,
    Overflow,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendToken {
    instance: u64,
    generation: u64,
    peer: IpAddr,
    seq: u16,
    kind: Completion,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Completion {
    Sync,
    PeerDelay([u8; 10]),
}
impl SendToken {
    fn key(&self) -> (IpAddr, u8) {
        (
            self.peer,
            match self.kind {
                Completion::Sync => 0,
                Completion::PeerDelay(_) => 2,
            },
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Datagram {
    pub peer: IpAddr,
    pub port: u16,
    pub bytes: Vec<u8>,
    /// After successful Sync/Pdelay_Resp send, pass this to complete_send with
    /// the actual
    /// software/hardware egress timestamp in the advertised master domain.
    pub completion: Option<SendToken>,
}

fn header(kind: u8, len: usize, flags: u16, clock: u64, seq: u16, interval: i8) -> Vec<u8> {
    let mut b = vec![0; len];
    b[0] = 0x10 | kind;
    b[1] = 2;
    b[2..4].copy_from_slice(&(len as u16).to_be_bytes());
    b[6..8].copy_from_slice(&flags.to_be_bytes());
    b[20..28].copy_from_slice(&clock.to_be_bytes());
    b[28..30].copy_from_slice(&0x8005u16.to_be_bytes());
    b[30..32].copy_from_slice(&seq.to_be_bytes());
    b[33] = interval as u8;
    b
}

fn timestamp(b: &mut [u8], ns: u64) {
    let seconds = (ns / 1_000_000_000).to_be_bytes();
    b[..6].copy_from_slice(&seconds[2..]);
    b[6..10].copy_from_slice(&((ns % 1_000_000_000) as u32).to_be_bytes());
}

fn announce(clock: u64, seq: u16) -> Vec<u8> {
    let mut b = header(0xb, 76, FLAGS, clock, seq, 0);
    b[47] = 128;
    b[48] = 6;
    b[49] = 0x21;
    b[50..52].copy_from_slice(&0x436au16.to_be_bytes());
    b[52] = 128;
    b[53..61].copy_from_slice(&clock.to_be_bytes());
    b[63] = 0x20;
    b[64..68].copy_from_slice(&[0, 8, 0, 8]);
    b[68..76].copy_from_slice(&clock.to_be_bytes());
    b
}

fn follow_up(clock: u64, seq: u16, ns: u64) -> Vec<u8> {
    let mut b = header(8, 96, FLAGS, clock, seq, -3);
    timestamp(&mut b[34..44], ns);
    b[44..54].copy_from_slice(&[0, 3, 0, 28, 0, 0x80, 0xc2, 0, 0, 1]);
    b[76..86].copy_from_slice(&[0, 3, 0, 16, 0, 0x0d, 0x93, 0, 0, 4]);
    b[86..94].copy_from_slice(&clock.to_be_bytes());
    b
}

pub struct Master {
    clock: u64,
    domain: Domain,
    instance: u64,
    peers: BTreeSet<IpAddr>,
    pending: BTreeMap<(IpAddr, u8), SendToken>,
    generation: u64,
    announce_seq: u16,
    last_poll: Option<u64>,
    last_sync: Option<u64>,
    last_announce: Option<u64>,
    foreign_clock: bool,
}

impl Master {
    pub fn new(domain: Domain) -> Result<Self, Error> {
        let clock = domain.id();
        if clock == 0 {
            return Err(Error::InvalidIdentity);
        }
        // Keep the workspace's Rust 1.85 API baseline: the newer try_update
        // replacement for deprecated fetch_update is not available there.
        let mut instance = INSTANCE.load(Ordering::Relaxed);
        loop {
            let next = instance.checked_add(1).ok_or(Error::Exhausted)?;
            match INSTANCE.compare_exchange_weak(
                instance,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => instance = current,
            }
        }
        Ok(Self {
            clock,
            domain,
            instance,
            peers: BTreeSet::new(),
            pending: BTreeMap::new(),
            generation: 0,
            announce_seq: 0,
            last_poll: None,
            last_sync: None,
            last_announce: None,
            foreign_clock: false,
        })
    }
    pub fn set_peers(&mut self, peers: &[IpAddr]) -> Result<(), Error> {
        if peers.len() > MAX_PEERS
            || peers
                .iter()
                .any(|ip| ip.is_unspecified() || ip.is_multicast())
        {
            return Err(Error::InvalidPeers);
        }
        let next: BTreeSet<_> = peers.iter().copied().collect();
        if next.iter().any(|ip| !self.peers.contains(ip)) {
            self.last_announce = None;
            self.last_sync = None;
        }
        self.pending.retain(|(ip, _), _| next.contains(ip));
        self.peers = next;
        Ok(())
    }
    /// Supply monotonic scheduler time, not an audio send-head timestamp. A
    /// late poll emits one fresh batch, never a catch-up burst.
    pub fn poll(&mut self, monotonic_ns: u64) -> Result<Vec<Datagram>, Error> {
        if self.foreign_clock {
            return Err(Error::ForeignClock);
        }
        if self.last_poll.is_some_and(|last| monotonic_ns < last) {
            return Err(Error::TimeReversed);
        }
        self.last_poll = Some(monotonic_ns);
        if self.peers.is_empty() {
            return Ok(Vec::new());
        }
        let sync_due = self
            .last_sync
            .is_none_or(|last| monotonic_ns - last >= SYNC_NS);
        let announce_due = self
            .last_announce
            .is_none_or(|last| monotonic_ns - last >= ANNOUNCE_NS);
        if sync_due {
            self.generation = self.generation.checked_add(1).ok_or(Error::Exhausted)?;
        }
        let mut out = Vec::new();
        for &peer in &self.peers {
            if announce_due {
                out.push(Datagram {
                    peer,
                    port: 320,
                    bytes: announce(self.clock, self.announce_seq),
                    completion: None,
                });
            }
            if sync_due {
                let token = SendToken {
                    instance: self.instance,
                    generation: self.generation,
                    peer,
                    seq: self.generation as u16,
                    kind: Completion::Sync,
                };
                out.push(Datagram {
                    peer,
                    port: 319,
                    bytes: header(0, 44, FLAGS | TWO_STEP, self.clock, token.seq, -3),
                    completion: Some(token.clone()),
                });
                self.pending.insert(token.key(), token);
            }
        }
        if announce_due {
            self.last_announce = Some(monotonic_ns);
            self.announce_seq = self.announce_seq.wrapping_add(1);
        }
        if sync_due {
            self.last_sync = Some(monotonic_ns);
        }
        Ok(out)
    }
    /// Failed sends must not call this. A token is single-use; removed peers,
    /// old poll batches and other Master instances cannot generate Follow_Up.
    pub fn complete_send(&mut self, token: &SendToken, egress: Reading) -> Option<Datagram> {
        if egress.domain() != self.domain
            || self.foreign_clock
            || self.pending.get(&token.key()) != Some(token)
        {
            return None;
        }
        self.pending.remove(&token.key());
        let bytes = match token.kind {
            Completion::Sync => follow_up(self.clock, token.seq, egress.ns()),
            Completion::PeerDelay(requester) => {
                let mut b = header(0xa, 54, FLAGS, self.clock, token.seq, -3);
                timestamp(&mut b[34..44], egress.ns());
                b[44..54].copy_from_slice(&requester);
                b
            }
        };
        Some(Datagram {
            peer: token.peer,
            port: 320,
            bytes,
            completion: None,
        })
    }
    /// Only strict, configured-peer Delay_Req is answered, unicast to that peer.
    /// Reception of a foreign Announce inhibits the fixed-master experiment;
    /// it is not silently ignored in favour of fighting another master.
    pub fn receive(&mut self, peer: IpAddr, packet: &[u8], ingress: Reading) -> Option<Datagram> {
        if ingress.domain() != self.domain
            || !self.peers.contains(&peer)
            || packet.len() < 34
            || packet.len() > 512
            || packet[0] >> 4 != 1
            || packet[1] & 15 != 2
            || packet[4] != 0
        {
            return None;
        }
        let len = u16::from_be_bytes([packet[2], packet[3]]) as usize;
        if len != packet.len() {
            return None;
        }
        let clock = u64::from_be_bytes(packet[20..28].try_into().ok()?);
        if clock == 0 || packet[28..30] == [0, 0] {
            return None;
        }
        if packet[0] & 15 == 0xb && len >= 64 && clock != self.clock {
            if packet[53..61] == [0; 8] {
                return None;
            }
            self.foreign_clock = true;
            self.pending.clear();
            return None;
        }
        if self.foreign_clock || clock == 0 || clock == self.clock {
            return None;
        }
        let seq = u16::from_be_bytes([packet[30], packet[31]]);
        if packet[0] & 15 == 0xc {
            return self.signaling(peer, packet, seq);
        }
        if packet[0] & 15 == 2 && len == 54 {
            let requester: [u8; 10] = packet[20..30].try_into().ok()?;
            let generation = self.generation.checked_add(1)?;
            self.generation = generation;
            let token = SendToken {
                instance: self.instance,
                generation,
                peer,
                seq,
                kind: Completion::PeerDelay(requester),
            };
            let mut bytes = header(3, 54, FLAGS | TWO_STEP, self.clock, seq, -3);
            timestamp(&mut bytes[34..44], ingress.ns());
            bytes[44..54].copy_from_slice(&requester);
            self.pending.insert(token.key(), token.clone());
            return Some(Datagram {
                peer,
                port: 319,
                bytes,
                completion: Some(token),
            });
        }
        if packet[0] & 15 != 1 || len != 44 {
            return None;
        }
        let mut bytes = header(9, 54, FLAGS | TWO_STEP, self.clock, seq, -3);
        timestamp(&mut bytes[34..44], ingress.ns());
        bytes[44..54].copy_from_slice(&packet[20..30]);
        Some(Datagram {
            peer,
            port: 320,
            bytes,
            completion: None,
        })
    }
    fn signaling(&self, peer: IpAddr, packet: &[u8], seq: u16) -> Option<Datagram> {
        if packet.len() < 48 {
            return None;
        }
        let target = &packet[34..44];
        let mut own = [0u8; 10];
        own[..8].copy_from_slice(&self.clock.to_be_bytes());
        own[8..].copy_from_slice(&0x8005u16.to_be_bytes());
        if target != [0; 10] && target != [0xff; 10] && target != own {
            return None;
        }
        let mut offset = 44;
        let mut grants = Vec::new();
        while offset < packet.len() {
            let tlv = packet.get(offset..offset + 4)?;
            let kind = u16::from_be_bytes([tlv[0], tlv[1]]);
            let len = u16::from_be_bytes([tlv[2], tlv[3]]) as usize;
            let value = packet.get(offset + 4..offset + 4 + len)?;
            if len % 2 != 0 {
                return None;
            }
            if kind == 4 && len == 6 && grants.len() < 8 {
                let message = value[0] >> 4;
                let interval = value[1] as i8;
                // Grant only the cadence this fixed-master core implements.
                let supported = (message == 0xb && interval == 0)
                    || ((message == 0 || message == 9) && interval == -3);
                let duration = u32::from_be_bytes(value[2..6].try_into().ok()?);
                if supported && duration > 0 {
                    grants.push((message, interval, duration.min(300)));
                }
            }
            offset += 4 + len;
        }
        if grants.is_empty() {
            return None;
        }
        let mut bytes = header(0xc, 44 + 12 * grants.len(), 0x0400, self.clock, seq, 0x7f);
        bytes[32] = 5;
        bytes[34..44].copy_from_slice(&packet[20..30]);
        for (index, (kind, period, duration)) in grants.into_iter().enumerate() {
            let at = 44 + index * 12;
            bytes[at..at + 6].copy_from_slice(&[0, 5, 0, 8, kind << 4, period as u8]);
            bytes[at + 6..at + 10].copy_from_slice(&duration.to_be_bytes());
            bytes[at + 11] = 1;
        }
        Some(Datagram {
            peer,
            port: 320,
            bytes,
            completion: None,
        })
    }
}

/// A frozen sample-to-master-time line. Created once after clock selection;
/// queue depth, packet arrival time and target connection time do not move it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaTimeline {
    domain: Domain,
    start_frame: u64,
    start_rtp: u32,
    start_ns: u64,
    rate: u32,
}
impl MediaTimeline {
    pub fn new(frame: u64, rtp: u32, start: Reading, rate: u32) -> Result<Self, Error> {
        if rate == 0 {
            return Err(Error::InvalidRate);
        }
        Ok(Self {
            domain: start.domain(),
            start_frame: frame,
            start_rtp: rtp,
            start_ns: start.ns(),
            rate,
        })
    }
    pub fn anchor(&self, frame: u64, current_domain: Domain) -> Result<crate::Anchor, Error> {
        if current_domain != self.domain {
            return Err(Error::ForeignClock);
        }
        let delta = i128::from(frame) - i128::from(self.start_frame);
        let ns = i128::from(self.start_ns) + delta * 1_000_000_000 / i128::from(self.rate);
        let network_ns = u64::try_from(ns).map_err(|_| Error::Overflow)?;
        Ok(crate::Anchor {
            rtp: self.start_rtp.wrapping_add(delta as u32),
            network_ns,
            timeline: self.domain.id(),
            playing: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn peer() -> IpAddr {
        "192.0.2.1".parse().unwrap()
    }
    fn master() -> Master {
        let mut m = Master::new(domain()).unwrap();
        m.set_peers(&[peer()]).unwrap();
        m
    }
    fn domain() -> Domain {
        Domain::new(0x1122334455667788, 1).unwrap()
    }
    fn time(ns: u64) -> Reading {
        domain().reading(ns)
    }
    fn read_ts(bytes: &[u8]) -> u64 {
        let s = u64::from_be_bytes([
            0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
        ]);
        s * 1_000_000_000 + u32::from_be_bytes(bytes[6..10].try_into().unwrap()) as u64
    }
    #[test]
    fn wire_profile_uses_two_step_egress_and_path_trace() {
        let mut m = master();
        let batch = m.poll(0).unwrap();
        assert_eq!(batch.len(), 2);
        let a = &batch[0].bytes;
        assert_eq!(&a[..8], &[0x1b, 2, 0, 76, 0, 0, 4, 8]);
        assert_eq!(&a[64..68], &[0, 8, 0, 8]);
        assert_eq!(&a[68..76], &a[20..28]);
        let sync = &batch[1];
        assert_eq!(sync.port, 319);
        assert_eq!(&sync.bytes[34..], &[0; 10]);
        let token = sync.completion.as_ref().unwrap();
        let f = m.complete_send(token, time(12_345_678_901)).unwrap();
        assert_eq!(&f.bytes[30..32], &sync.bytes[30..32]);
        assert_eq!(read_ts(&f.bytes[34..44]), 12_345_678_901);
        assert_eq!(&f.bytes[86..94], &a[20..28]);
        assert!(m.complete_send(token, time(0)).is_none());
    }
    #[test]
    fn receiver_parser_accepts_generated_master_timeline() {
        let mut m = master();
        let mut receiver = crate::Ptp::new(9);
        let batch = m.poll(0).unwrap();
        receiver.on_packet(peer(), &batch[0].bytes, 0);
        for n in 0..12u64 {
            let sync = m
                .poll((n + 1) * SYNC_NS)
                .unwrap()
                .into_iter()
                .find(|p| p.completion.is_some())
                .unwrap();
            let f = m
                .complete_send(
                    sync.completion.as_ref().unwrap(),
                    time(10_000_000_000 + n * SYNC_NS),
                )
                .unwrap();
            receiver.on_packet(peer(), &sync.bytes, n * SYNC_NS);
            receiver.on_packet(peer(), &f.bytes, n * SYNC_NS);
        }
        assert_eq!(
            receiver.timeline(m.clock).unwrap().offset_ns,
            10_000_000_000
        );
    }
    #[test]
    fn scheduling_is_bounded_and_removed_or_stale_tokens_cannot_complete() {
        let mut m = master();
        let old = m.poll(0).unwrap()[1].completion.clone().unwrap();
        assert!(m.poll(1).unwrap().is_empty());
        assert_eq!(m.poll(50_000_000_000).unwrap().len(), 2);
        assert!(m.complete_send(&old, time(1)).is_none());
        assert_eq!(m.poll(0), Err(Error::TimeReversed));
        let current = m.poll(51_000_000_000).unwrap()[1]
            .completion
            .clone()
            .unwrap();
        m.set_peers(&[]).unwrap();
        m.set_peers(&[peer()]).unwrap();
        assert!(m.complete_send(&current, time(1)).is_none());
        let mut other = master();
        other.poll(0).unwrap();
        assert!(other.complete_send(&old, time(1)).is_none());
    }
    #[test]
    fn delay_response_preserves_request_identity_sequence_and_ingress() {
        let mut m = master();
        let mut req = header(1, 44, FLAGS, 99, 0xfffe, -3);
        req[28..30].copy_from_slice(&7u16.to_be_bytes());
        let r = m.receive(peer(), &req, time(2_999_999_999)).unwrap();
        assert_eq!(r.peer, peer());
        assert_eq!(r.port, 320);
        assert_eq!(&r.bytes[44..54], &req[20..30]);
        assert_eq!(&r.bytes[30..32], &req[30..32]);
        assert_eq!(read_ts(&r.bytes[34..44]), 2_999_999_999);
        assert!(
            m.receive("192.0.2.9".parse().unwrap(), &req, time(0))
                .is_none()
        );
        req[2..4].copy_from_slice(&43u16.to_be_bytes());
        assert!(m.receive(peer(), &req, time(0)).is_none());
    }
    #[test]
    fn foreign_clock_inhibits_transmission_instead_of_uncontrolled_election() {
        let mut m = master();
        let token = m.poll(0).unwrap()[1].completion.clone().unwrap();
        m.receive(peer(), &announce(999, 1), time(0));
        assert_eq!(m.poll(1), Err(Error::ForeignClock));
        assert!(m.complete_send(&token, time(0)).is_none());
    }
    #[test]
    fn wrong_clock_or_restart_cannot_timestamp_a_sync_or_reuse_media_anchor() {
        let mut m = master();
        let token = m.poll(0).unwrap()[1].completion.clone().unwrap();
        let restart = Domain::new(domain().id(), 2).unwrap();
        assert!(m.complete_send(&token, restart.reading(100)).is_none());
        assert!(
            m.complete_send(&token, Domain::new(999, 1).unwrap().reading(100))
                .is_none()
        );
        assert!(m.complete_send(&token, time(100)).is_some());
        let line = MediaTimeline::new(0, 0, time(100), 44_100).unwrap();
        assert_eq!(line.anchor(1, restart), Err(Error::ForeignClock));
        assert!(line.anchor(1, domain()).is_ok());
    }
    #[test]
    fn peer_delay_uses_real_send_completion_and_does_not_consume_sync() {
        let mut m = master();
        let sync = m.poll(0).unwrap()[1].completion.clone().unwrap();
        let request = header(2, 54, FLAGS, 99, 17, -3);
        let response = m.receive(peer(), &request, time(500)).unwrap();
        assert_eq!(response.port, 319);
        assert_eq!(response.bytes[0], 0x13);
        assert_eq!(read_ts(&response.bytes[34..44]), 500);
        let follow = m
            .complete_send(response.completion.as_ref().unwrap(), time(700))
            .unwrap();
        assert_eq!(follow.port, 320);
        assert_eq!(follow.bytes[0], 0x1a);
        assert_eq!(read_ts(&follow.bytes[34..44]), 700);
        assert_eq!(&follow.bytes[44..54], &request[20..30]);
        assert!(m.complete_send(&sync, time(800)).is_some());
        assert!(
            m.complete_send(response.completion.as_ref().unwrap(), time(900))
                .is_none()
        );
    }
    #[test]
    fn signaling_grants_only_supported_cadence_and_valid_complete_tlvs() {
        let mut m = master();
        let mut request = header(0xc, 64, 0x0400, 99, 42, 0x7f);
        request[44..54].copy_from_slice(&[0, 4, 0, 6, 0, -3i8 as u8, 0, 0, 3, 0]);
        request[54..64].copy_from_slice(&[0, 4, 0, 6, 0xb0, 0, 0, 0, 0, 30]);
        let response = m.receive(peer(), &request, time(0)).unwrap();
        assert_eq!(response.port, 320);
        assert_eq!(response.bytes.len(), 68);
        assert_eq!(&response.bytes[34..44], &request[20..30]);
        assert_eq!(&response.bytes[44..50], &[0, 5, 0, 8, 0, -3i8 as u8]);
        assert_eq!(
            u32::from_be_bytes(response.bytes[50..54].try_into().unwrap()),
            300
        );
        request[45] = 0xff; // Unsupported TLV is skipped; the second grant remains.
        assert_eq!(
            m.receive(peer(), &request, time(0)).unwrap().bytes.len(),
            56
        );
        request[56..58].copy_from_slice(&99u16.to_be_bytes());
        assert!(m.receive(peer(), &request, time(0)).is_none());
    }
    #[test]
    fn removing_peer_revokes_pending_peer_delay_response() {
        let mut m = master();
        let request = header(2, 54, FLAGS, 99, 1, -3);
        let response = m.receive(peer(), &request, time(0)).unwrap();
        m.set_peers(&[]).unwrap();
        assert!(
            m.complete_send(response.completion.as_ref().unwrap(), time(1))
                .is_none()
        );
    }
    #[test]
    fn malformed_clock_identity_cannot_inhibit_master_or_obtain_grant() {
        let mut m = master();
        let mut packet = announce(999, 1);
        packet[20..28].fill(0);
        assert!(m.receive(peer(), &packet, time(0)).is_none());
        assert!(m.poll(0).is_ok());
        packet = announce(999, 1);
        packet[53..61].fill(0);
        m.receive(peer(), &packet, time(0));
        assert!(m.poll(1).is_ok());
        packet = header(2, 54, FLAGS, 99, 1, -3);
        packet[28..30].fill(0);
        assert!(m.receive(peer(), &packet, time(0)).is_none());
    }
    #[test]
    fn frozen_media_line_handles_rtp_wrap_and_late_member_without_reanchoring() {
        let d = Domain::new(8, 1).unwrap();
        let line =
            MediaTimeline::new(100, u32::MAX - 100, d.reading(10_000_000_000), 44_100).unwrap();
        let a = line.anchor(44_200, d).unwrap();
        assert_eq!(a.rtp, (u32::MAX - 100).wrapping_add(44_100));
        assert_eq!(a.network_ns, 11_000_000_000);
        assert_eq!(a.timeline, 8);
        assert_eq!(line.anchor(99, d).unwrap().network_ns, 9_999_977_325);
        assert_eq!(line.anchor(44_200, d), line.anchor(44_200, d));
        assert!(MediaTimeline::new(0, 0, d.reading(0), 0).is_err());
        assert!(
            MediaTimeline::new(100, 0, d.reading(0), 1)
                .unwrap()
                .anchor(0, d)
                .is_err()
        );
    }
}
