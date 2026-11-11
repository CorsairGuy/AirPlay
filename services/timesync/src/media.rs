//! One injected Instant-backed physical source for the unified service and PCM
//! producer. No sockets, clock election, daemon startup or wall-clock reads.
//! External MONOTONIC_RAW/NQPTP sources need their own explicit domain bridge;
//! do not label those counters as this Instant epoch.
use crate::{
    engine::{Quality, Snapshot},
    sender::ClockSource,
};
use clock::{
    domain::{Domain, LocalClock, LocalInstant, Reading},
    role::Role,
};
use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;

#[derive(Debug)]
pub struct InstantSource {
    origin: Instant,
    local: LocalClock,
    own: Domain,
    network_origin_ns: u64,
}
impl InstantSource {
    /// Caller chooses one origin/domain for the group, rather than one per
    /// receiver. Network origin is explicit; no guessed RAW/UTC offset is added.
    pub fn new(origin: Instant, local: LocalClock, own: Domain, network_origin_ns: u64) -> Self {
        Self {
            origin,
            local,
            own,
            network_origin_ns,
        }
    }
    pub fn local_at(&self, instant: Instant) -> io::Result<LocalInstant> {
        let elapsed = instant.checked_duration_since(self.origin).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "instant precedes physical clock epoch",
            )
        })?;
        let ns = u64::try_from(elapsed.as_nanos()).map_err(io::Error::other)?;
        Ok(self.local.at(ns))
    }
    pub fn sample_at(&self, instant: Instant) -> io::Result<(LocalInstant, Reading)> {
        let local = self.local_at(instant)?;
        let ns = self
            .network_origin_ns
            .checked_add(local.ns())
            .ok_or_else(|| io::Error::other("physical clock overflow"))?;
        Ok((local, self.own.reading(ns)))
    }
}
impl ClockSource for InstantSource {
    fn sample(&self) -> io::Result<(LocalInstant, Reading)> {
        self.sample_at(Instant::now())
    }
}

/// Pins a currently qualified local-master epoch. Follower/one-way estimates
/// are intentionally not promoted to an outgoing master or claimed servo lock.
pub struct MasterTimeline {
    source: Arc<InstantSource>,
    status: watch::Receiver<Snapshot>,
    domain: Domain,
    max_ahead: Duration,
}
impl MasterTimeline {
    pub fn new(
        source: Arc<InstantSource>,
        status: watch::Receiver<Snapshot>,
        max_ahead: Duration,
    ) -> io::Result<Self> {
        if max_ahead > Duration::from_secs(1) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unbounded producer prediction",
            ));
        }
        let domain = status.borrow().selection.domain.ok_or_else(|| {
            io::Error::new(io::ErrorKind::WouldBlock, "PTP role is not qualified")
        })?;
        if domain.id() != source.own.id() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "selected master is not the injected physical source",
            ));
        }
        let timeline = Self {
            source,
            status,
            domain,
            max_ahead,
        };
        timeline.reading_at(Instant::now())?;
        Ok(timeline)
    }
    pub fn domain(&self) -> Domain {
        self.domain
    }
    /// Call once on the PCM producer before broadcasting a frame. Uses exactly
    /// the same source epoch supplied to Service::bind, never per-target clocks.
    pub fn reading_at(&self, produced: Instant) -> io::Result<Reading> {
        self.reading_at_now(produced, Instant::now())
    }
    fn reading_at_now(&self, produced: Instant, now: Instant) -> io::Result<Reading> {
        if self.status.has_changed().is_err() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "clock service closed",
            ));
        }
        let snapshot = *self.status.borrow();
        if snapshot.selection.role != Role::Master
            || snapshot.selection.domain != Some(self.domain)
            || snapshot.quality != Some(Quality::Local)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "PTP role/domain/epoch changed",
            ));
        }
        if produced
            .checked_duration_since(now)
            .is_some_and(|ahead| ahead > self.max_ahead)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "producer instant too far ahead",
            ));
        }
        let (local_now, own) = self.source.sample_at(now)?;
        let mapping = snapshot
            .mapping
            .ok_or_else(|| io::Error::new(io::ErrorKind::WouldBlock, "PTP mapping absent"))?;
        if mapping.domain() != self.domain || mapping.rate_ppb() != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "local-master mapping inconsistent",
            ));
        }
        let mapped_now = mapping
            .to_network(local_now, local_now)
            .map_err(io::Error::other)?;
        if mapped_now.ns() != own.ns() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "service and producer physical source differ",
            ));
        }
        mapping
            .to_network(self.source.local_at(produced)?, local_now)
            .map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clock::{domain::ClockMap, role::Selection};
    #[tokio::test]
    async fn unified_service_and_producer_share_source_and_shutdown_invalidates_media() {
        use crate::{
            sender::{Config as Network, Ports},
            service::{Config, Service},
        };
        let origin = Instant::now();
        let domain = Domain::new(100, 1).unwrap();
        let source = Arc::new(InstantSource::new(
            origin,
            LocalClock::new(9).unwrap(),
            domain,
            1_000_000_000,
        ));
        let service = Service::bind(
            Config {
                network: Network {
                    domain,
                    bind_ip: "127.0.0.1".parse().unwrap(),
                    ports: Ports {
                        event: 0,
                        general: 0,
                    },
                    peers: vec![],
                },
                election: clock::role::Config {
                    policy: clock::role::Policy::Elect,
                    settle_ns: 10_000_000,
                    announce_lease_ns: 200_000_000,
                    min_announces: 2,
                },
                follow: clock::follower::Config {
                    pairing_lease_ns: 100_000_000,
                    mapping_lease_ns: 150_000_000,
                    warmup_samples: 2,
                    step_threshold_ns: 50_000_000,
                },
                probe: None,
            },
            source.clone(),
        )
        .await
        .unwrap();
        assert!(service.addresses().0.ip().is_loopback());
        let mut status = service.subscribe();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if status.borrow_and_update().selection.role == Role::Master {
                    break;
                }
                status.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let timeline =
            MasterTimeline::new(source.clone(), status, Duration::from_millis(100)).unwrap();
        let produced = Instant::now();
        assert_eq!(
            timeline.reading_at(produced).unwrap().ns(),
            source.sample_at(produced).unwrap().1.ns()
        );
        service.shutdown().await.unwrap();
        assert!(timeline.reading_at(Instant::now()).is_err());
    }
    fn fixture() -> (Instant, watch::Sender<Snapshot>, MasterTimeline) {
        let origin = Instant::now();
        let local = LocalClock::new(1).unwrap();
        let domain = Domain::new(7, 2).unwrap();
        let source = Arc::new(InstantSource::new(
            origin,
            local,
            Domain::new(7, 1).unwrap(),
            1_000_000_000,
        ));
        let snapshot = Snapshot {
            selection: Selection {
                role: Role::Master,
                domain: Some(domain),
            },
            mapping: Some(
                ClockMap::new(local.at(0), domain.reading(1_000_000_000), 0, 2_000_000_000)
                    .unwrap(),
            ),
            quality: Some(Quality::Local),
            link: None,
        };
        let (updates, status) = watch::channel(snapshot);
        let timeline = MasterTimeline::new(source, status, Duration::from_millis(100)).unwrap();
        (origin, updates, timeline)
    }
    #[test]
    fn service_and_producer_use_same_physical_source_and_selected_epoch() {
        let (origin, _updates, timeline) = fixture();
        let reading = timeline
            .reading_at_now(
                origin + Duration::from_millis(10),
                origin + Duration::from_millis(20),
            )
            .unwrap();
        assert_eq!(reading, timeline.domain().reading(1_010_000_000));
        let sample = timeline
            .source
            .sample_at(origin + Duration::from_millis(10))
            .unwrap();
        assert_eq!(sample.0.ns(), 10_000_000);
        assert_eq!(sample.1.ns(), reading.ns());
        assert_ne!(
            sample.1.domain(),
            reading.domain(),
            "role epoch is separate from the fixed physical epoch"
        );
    }
    #[test]
    fn stale_mapping_and_future_producer_are_rejected() {
        let (origin, _updates, timeline) = fixture();
        assert!(
            timeline
                .reading_at_now(origin, origin + Duration::from_secs(3))
                .is_err()
        );
        assert!(
            timeline
                .reading_at_now(origin + Duration::from_secs(1), origin)
                .is_err()
        );
        assert!(
            timeline
                .source
                .local_at(origin - Duration::from_nanos(1))
                .is_err()
        );
    }
    #[test]
    fn role_loss_epoch_change_and_closed_service_are_not_relabelled() {
        let (origin, updates, timeline) = fixture();
        let initial = *updates.borrow();
        updates.send_replace(Snapshot::inactive());
        assert!(timeline.reading_at_now(origin, origin).is_err());
        let mut changed = initial;
        changed.selection.domain = Some(Domain::new(7, 3).unwrap());
        updates.send_replace(changed);
        assert!(timeline.reading_at_now(origin, origin).is_err());
        updates.send_replace(initial);
        drop(updates);
        assert!(timeline.reading_at_now(origin, origin).is_err());
    }
    #[test]
    fn different_physical_source_and_one_way_quality_are_rejected() {
        let (origin, updates, timeline) = fixture();
        let mut snapshot = *updates.borrow();
        snapshot.mapping = Some(
            ClockMap::new(
                LocalClock::new(1).unwrap().at(0),
                timeline.domain().reading(2_000_000_000),
                0,
                2_000_000_000,
            )
            .unwrap(),
        );
        updates.send_replace(snapshot);
        assert!(timeline.reading_at_now(origin, origin).is_err());
        snapshot.quality = Some(Quality::OneWay);
        updates.send_replace(snapshot);
        assert!(timeline.reading_at_now(origin, origin).is_err());
    }
}
