//! Opt-in two-step gPTP peer-delay requester. ONE outstanding exchange.
//! Measures the neighbor's source clock, never silently labels it as the GM.
use crate::{
    domain::{Domain, LocalClock, LocalInstant},
    measurement::{self, Estimate, Estimator, Exchange},
    role::Dataset,
};
use std::{
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
};
static INSTANCE: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub interval_ns: u64,
    pub timeout_ns: u64,
    pub measurement: measurement::Config,
}
impl Config {
    pub fn validate(self) -> Result<(), measurement::Error> {
        if self.interval_ns < 10_000_000 || self.timeout_ns == 0 || self.timeout_ns > 5_000_000_000
        {
            return Err(measurement::Error::Invalid);
        }
        self.measurement.validate()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    owner: u64,
    generation: u64,
}
pub struct Request {
    pub peer: IpAddr,
    pub bytes: Vec<u8>,
    pub token: Token,
}
struct Pending {
    token: Token,
    seq: u16,
    issued: u64,
    sent: Option<LocalInstant>,
    response: Option<(u64, LocalInstant, i64)>,
    followup: Option<(u64, i64)>,
}
pub struct Requester {
    owner: u64,
    generation: u64,
    own: [u8; 10],
    peer: IpAddr,
    source: Dataset,
    neighbor: Domain,
    local: LocalClock,
    config: Config,
    last_seen: Option<u64>,
    last_issue: Option<u64>,
    pending: Option<Pending>,
    estimator: Estimator,
}
impl Requester {
    pub fn new(
        own_clock: u64,
        own_port: u16,
        peer: IpAddr,
        source: Dataset,
        neighbor: Domain,
        local: LocalClock,
        config: Config,
    ) -> Result<Self, measurement::Error> {
        config.validate()?;
        if own_clock == 0
            || own_port == 0
            || source.source_clock != neighbor.id()
            || own_clock == source.source_clock
            || source.source_port == 0
        {
            return Err(measurement::Error::Invalid);
        }
        let mut owner = INSTANCE.load(Ordering::Relaxed);
        loop {
            let next = owner.checked_add(1).ok_or(measurement::Error::Overflow)?;
            match INSTANCE.compare_exchange_weak(owner, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(current) => owner = current,
            }
        }
        let mut own = [0; 10];
        own[..8].copy_from_slice(&own_clock.to_be_bytes());
        own[8..].copy_from_slice(&own_port.to_be_bytes());
        Ok(Self {
            owner,
            generation: 0,
            own,
            peer,
            source,
            neighbor,
            local,
            config,
            last_seen: None,
            last_issue: None,
            pending: None,
            estimator: Estimator::new(neighbor, local, config.measurement)?,
        })
    }
    fn check(&mut self, now: LocalInstant) -> Result<(), measurement::Error> {
        if now.clock() != self.local {
            self.pending = None;
            self.estimator.reset();
            return Err(measurement::Error::WrongDomain);
        }
        if self.last_seen.is_some_and(|last| now.ns() < last) {
            self.pending = None;
            self.estimator.reset();
            return Err(measurement::Error::TimeOrder);
        }
        self.last_seen = Some(now.ns());
        Ok(())
    }
    pub fn poll(&mut self, now: LocalInstant) -> Result<Option<Request>, measurement::Error> {
        self.check(now)?;
        if let Some(p) = &self.pending {
            if now.ns() - p.issued <= self.config.timeout_ns {
                return Ok(None);
            }
            self.pending = None;
        }
        if self
            .last_issue
            .is_some_and(|last| now.ns() - last < self.config.interval_ns)
        {
            return Ok(None);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(measurement::Error::Overflow)?;
        let token = Token {
            owner: self.owner,
            generation: self.generation,
        };
        let seq = self.generation as u16;
        let mut bytes = vec![0; 54];
        bytes[0] = 0x12;
        bytes[1] = 2;
        bytes[2..4].copy_from_slice(&54u16.to_be_bytes());
        bytes[6..8].copy_from_slice(&0x0408u16.to_be_bytes());
        bytes[20..30].copy_from_slice(&self.own);
        bytes[30..32].copy_from_slice(&seq.to_be_bytes());
        bytes[32] = 5;
        bytes[33] = 0x7f;
        self.pending = Some(Pending {
            token,
            seq,
            issued: now.ns(),
            sent: None,
            response: None,
            followup: None,
        });
        self.last_issue = Some(now.ns());
        Ok(Some(Request {
            peer: self.peer,
            bytes,
            token,
        }))
    }
    pub fn sent(&mut self, token: Token, now: LocalInstant) -> Result<bool, measurement::Error> {
        self.check(now)?;
        let Some(p) = self.pending.as_mut() else {
            return Ok(false);
        };
        if p.token != token || p.sent.is_some() || now.ns() - p.issued > self.config.timeout_ns {
            return Ok(false);
        }
        p.sent = Some(now);
        Ok(true)
    }
    pub fn receive(
        &mut self,
        peer: IpAddr,
        packet: &[u8],
        now: LocalInstant,
    ) -> Result<Option<Estimate>, measurement::Error> {
        self.check(now)?;
        if peer != self.peer
            || !(54..=512).contains(&packet.len())
            || packet[0] >> 4 != 1
            || packet[1] & 15 != 2
            || packet[4] != 0
            || u16::from_be_bytes([packet[2], packet[3]]) as usize != packet.len()
            || packet[20..28] != self.source.source_clock.to_be_bytes()
            || packet[28..30] != self.source.source_port.to_be_bytes()
            || packet[44..54] != self.own
        {
            return Ok(None);
        }
        let Some(p) = self.pending.as_mut() else {
            return Ok(None);
        };
        if p.sent.is_none()
            || now.ns() - p.issued > self.config.timeout_ns
            || u16::from_be_bytes([packet[30], packet[31]]) != p.seq
        {
            return Ok(None);
        }
        let Some(stamp) = timestamp(&packet[34..44]) else {
            return Ok(None);
        };
        let correction = i64::from_be_bytes(packet[8..16].try_into().unwrap()) / 65536;
        match packet[0] & 15 {
            3 if u16::from_be_bytes([packet[6], packet[7]]) & 0x0200 != 0
                && p.response.is_none() =>
            {
                p.response = Some((stamp, now, correction))
            }
            0xa if p.followup.is_none() => p.followup = Some((stamp, correction)),
            _ => return Ok(None),
        }
        let (Some(sent), Some((t2, received, c1)), Some((t3, c2))) =
            (p.sent, p.response, p.followup)
        else {
            return Ok(None);
        };
        self.pending = None;
        let exchange = Exchange {
            sent,
            peer_received: self.neighbor.reading(t2),
            peer_sent: self.neighbor.reading(t3),
            received,
            response_correction_ns: c1,
            followup_correction_ns: c2,
        };
        match self.estimator.observe(exchange) {
            Ok(value) => Ok(value),
            Err(_) => {
                self.estimator.reset();
                Ok(None)
            }
        }
    }
    pub fn latest(&self, now: LocalInstant) -> Option<Estimate> {
        self.estimator.latest(now)
    }
}
fn timestamp(data: &[u8]) -> Option<u64> {
    let seconds = u64::from_be_bytes([0, 0, data[0], data[1], data[2], data[3], data[4], data[5]]);
    let nanos = u32::from_be_bytes(data[6..10].try_into().ok()?);
    if nanos >= 1_000_000_000 {
        return None;
    }
    seconds
        .checked_mul(1_000_000_000)?
        .checked_add(u64::from(nanos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptp_sender::Master;
    fn local() -> LocalClock {
        LocalClock::new(1).unwrap()
    }
    fn setup() -> (Requester, Master) {
        let mut m = Master::new(Domain::new(200, 1).unwrap()).unwrap();
        m.set_peers(&["192.0.2.10".parse().unwrap()]).unwrap();
        let config = Config {
            interval_ns: 125_000_000,
            timeout_ns: 100_000_000,
            measurement: measurement::Config {
                max_roundtrip_ns: 100_000_000,
                rate_window_ns: 1_000_000_000,
                valid_for_ns: 2_000_000_000,
                timestamp_error_ns: 0,
            },
        };
        (
            Requester::new(
                100,
                0x8005,
                "192.0.2.20".parse().unwrap(),
                m.dataset(),
                Domain::new(200, 9).unwrap(),
                local(),
                config,
            )
            .unwrap(),
            m,
        )
    }
    #[test]
    fn real_responder_wire_correlates_and_reordered_followup_is_supported() {
        let (mut p, mut m) = setup();
        let request = p.poll(local().at(1_000_000_000)).unwrap().unwrap();
        p.sent(request.token, local().at(1_000_000_000)).unwrap();
        let response = m
            .receive(
                "192.0.2.10".parse().unwrap(),
                &request.bytes,
                Domain::new(200, 1).unwrap().reading(11_002_000_000),
            )
            .unwrap();
        let follow = m
            .complete_send(
                response.completion.as_ref().unwrap(),
                Domain::new(200, 1).unwrap().reading(11_003_000_000),
            )
            .unwrap();
        assert!(
            p.receive(request.peer, &follow.bytes, local().at(1_004_000_000))
                .unwrap()
                .is_none()
        );
        let value = p
            .receive(request.peer, &response.bytes, local().at(1_005_000_000))
            .unwrap()
            .unwrap();
        assert_eq!(value.mean_path_ns, 2_000_000);
        assert_eq!(value.neighbor.id(), 200);
        assert!(
            p.receive(request.peer, &response.bytes, local().at(1_006_000_000))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn neighbor_clock_is_not_silently_replaced_by_its_grandmaster() {
        let (p, m) = setup();
        let mut source = m.dataset();
        source.grandmaster = 50;
        source.steps_removed = 1;
        assert!(
            Requester::new(
                100,
                0x8005,
                p.peer,
                source,
                Domain::new(200, 9).unwrap(),
                local(),
                p.config
            )
            .is_ok()
        );
        assert!(
            Requester::new(
                100,
                0x8005,
                p.peer,
                source,
                Domain::new(50, 9).unwrap(),
                local(),
                p.config
            )
            .is_err()
        );
    }
    #[test]
    fn stale_callbacks_wrong_requester_and_timeout_cannot_complete_another_exchange() {
        let (mut p, mut m) = setup();
        let old = p.poll(local().at(0)).unwrap().unwrap();
        let new = p.poll(local().at(125_000_000)).unwrap().unwrap();
        assert!(!p.sent(old.token, local().at(125_000_001)).unwrap());
        p.sent(new.token, local().at(125_000_002)).unwrap();
        let mut response = m
            .receive(
                "192.0.2.10".parse().unwrap(),
                &new.bytes,
                Domain::new(200, 1).unwrap().reading(1_000_000_000),
            )
            .unwrap();
        response.bytes[44] ^= 1;
        assert!(
            p.receive(new.peer, &response.bytes, local().at(125_001_000))
                .unwrap()
                .is_none()
        );
        assert!(p.latest(local().at(125_001_000)).is_none());
        let (mut other, _) = setup();
        other.poll(local().at(0)).unwrap();
        assert!(!other.sent(old.token, local().at(1)).unwrap());
    }
}
