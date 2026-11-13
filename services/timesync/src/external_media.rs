//! Cached qualified external timeline. No file I/O, sockets or daemon commands
//! on the producer path. One publisher owns observation updates and revocation.
use clock::{
    domain::{Domain, Reading},
    external::{Correlation, Qualifier, Selection},
    nqptp::Snapshot,
};
use std::{
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;
#[derive(Clone, Copy)]
struct Observation {
    selection: Selection,
    sample: Snapshot,
    correlation: Correlation,
}
struct State {
    qualifier: Qualifier,
    observation: Option<Observation>,
}
struct Shared {
    state: Mutex<State>,
    lost: watch::Sender<bool>,
    domain: Domain,
    max_ahead: Duration,
}
pub struct Timeline {
    shared: Arc<Shared>,
}
/// Not cloneable: losing the only publisher invalidates every consumer.
pub struct Publisher {
    shared: Arc<Shared>,
}
fn error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, message)
}
fn revoke(shared: &Shared) {
    if let Ok(mut state) = shared.state.lock() {
        state.qualifier.revoke();
    }
    shared.lost.send_replace(true);
}
fn checked(
    shared: &Shared,
    state: &mut State,
    observation: Observation,
    produced: Instant,
) -> io::Result<Reading> {
    if *shared.lost.borrow() {
        return Err(error("external clock revoked"));
    }
    let now = Instant::now();
    if produced
        .checked_duration_since(now)
        .is_some_and(|d| d > shared.max_ahead)
    {
        state.qualifier.revoke();
        shared.lost.send_replace(true);
        return Err(error("external producer prediction exceeds bound"));
    }
    state
        .qualifier
        .observe(
            observation.selection,
            observation.sample,
            observation.correlation,
            produced,
            now,
        )
        .map_err(|e| {
            shared.lost.send_replace(true);
            io::Error::other(e)
        })
}
impl Timeline {
    /// First valid publication must precede group/lease construction. The caller
    /// attests the daemon/role; this API never derives trust from an ABI header.
    pub fn new(
        selection: Selection,
        max_age: Duration,
        max_uncertainty: Duration,
        max_ahead: Duration,
    ) -> io::Result<(Arc<Self>, Publisher)> {
        if max_ahead > Duration::from_secs(1) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unbounded external prediction",
            ));
        }
        let qualifier =
            Qualifier::new(selection, max_age, max_uncertainty).map_err(io::Error::other)?;
        let (lost, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                qualifier,
                observation: None,
            }),
            lost,
            domain: selection.domain,
            max_ahead,
        });
        Ok((
            Arc::new(Self {
                shared: shared.clone(),
            }),
            Publisher { shared },
        ))
    }
    pub fn domain(&self) -> Domain {
        self.shared.domain
    }
    pub fn loss(&self) -> watch::Receiver<bool> {
        self.shared.lost.subscribe()
    }
    /// Bounded cached conversion; no SHM reads or clock_gettime here. Errors
    /// revoke the selection and cannot be undone by a subsequent publication.
    pub fn reading_at(&self, produced: Instant) -> io::Result<Reading> {
        let mut state = self.shared.state.lock().map_err(|_| {
            self.shared.lost.send_replace(true);
            error("external state poisoned")
        })?;
        let observation = state
            .observation
            .ok_or_else(|| error("external clock not published"))?;
        checked(&self.shared, &mut state, observation, produced)
    }
}
impl Publisher {
    pub fn update(
        &mut self,
        selection: Selection,
        sample: Snapshot,
        correlation: Correlation,
    ) -> io::Result<()> {
        let mut state = self.shared.state.lock().map_err(|_| {
            self.shared.lost.send_replace(true);
            error("external state poisoned")
        })?;
        let observation = Observation {
            selection,
            sample,
            correlation,
        };
        checked(&self.shared, &mut state, observation, Instant::now())?;
        state.observation = Some(observation);
        Ok(())
    }
    pub fn revoke(&mut self) {
        revoke(&self.shared);
    }
}
impl Drop for Publisher {
    fn drop(&mut self) {
        revoke(&self.shared);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use clock::external::StartMarker;
    fn fixture() -> (Selection, Snapshot, Correlation) {
        let selection = Selection {
            daemon_instance: 1,
            domain: Domain::new(9, 1).unwrap(),
            master_start_raw_ns: 100,
            start_marker: StartMarker::RefreshedPublication,
            clock_change_offset: None,
            sender_master_verified: true,
        };
        let snapshot = Snapshot {
            version: 10,
            clock_id: 9,
            local_time: 1_000_000_000,
            offset: 50,
            clock_change_offset: None,
            master_start_time: 100,
        };
        let correlation = Correlation {
            at: Instant::now(),
            raw_ns: 1_000_000_100,
            rate_ppb: 0,
            rate_error_ppb: 10,
            uncertainty_ns: 10,
            valid_for: Duration::from_secs(1),
        };
        (selection, snapshot, correlation)
    }
    #[test]
    fn publication_drives_real_producer_instant_without_relabeling() {
        let (s, p, c) = fixture();
        let (t, mut owner) = Timeline::new(
            s,
            Duration::from_secs(1),
            Duration::from_micros(1),
            Duration::from_millis(100),
        )
        .unwrap();
        assert!(t.reading_at(c.at).is_err());
        owner.update(s, p, c).unwrap();
        assert_eq!(t.reading_at(c.at).unwrap(), s.domain.reading(c.raw_ns + 50));
        assert!(t.loss().borrow().eq(&false));
        owner
            .update(
                s,
                Snapshot {
                    master_start_time: p.local_time,
                    ..p
                },
                c,
            )
            .unwrap();
        assert_eq!(t.domain(), s.domain);
    }
    #[test]
    fn invalid_update_or_publisher_loss_is_permanent() {
        let (s, p, c) = fixture();
        let (t, mut owner) = Timeline::new(
            s,
            Duration::from_secs(1),
            Duration::from_micros(1),
            Duration::ZERO,
        )
        .unwrap();
        owner.update(s, p, c).unwrap();
        assert!(
            owner
                .update(
                    Selection {
                        daemon_instance: 2,
                        ..s
                    },
                    p,
                    c
                )
                .is_err()
        );
        assert!(*t.loss().borrow());
        assert!(owner.update(s, p, c).is_err());
        assert!(t.reading_at(c.at).is_err());
        let (t, mut owner) = Timeline::new(
            s,
            Duration::from_secs(1),
            Duration::from_micros(1),
            Duration::ZERO,
        )
        .unwrap();
        owner.update(s, p, c).unwrap();
        drop(owner);
        assert!(*t.loss().borrow());
        assert!(t.reading_at(c.at).is_err());
    }
    #[tokio::test]
    async fn external_lease_cancellation_does_not_revoke_shared_owner() {
        let (s, p, c) = fixture();
        let (t, mut owner) = Timeline::new(
            s,
            Duration::from_secs(1),
            Duration::from_micros(1),
            Duration::ZERO,
        )
        .unwrap();
        owner.update(s, p, c).unwrap();
        let (_user, upstream) = watch::channel(false);
        let lease = crate::media::MasterLease::spawn_external(t.clone(), upstream);
        lease.cancel();
        assert!(*lease.subscribe().borrow());
        assert!(!*t.loss().borrow());
        assert!(t.reading_at(c.at).is_ok());
        let (_user, upstream) = watch::channel(false);
        let lease = crate::media::MasterLease::spawn_external(t, upstream);
        let mut stop = lease.subscribe();
        drop(owner);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !*stop.borrow_and_update() {
                stop.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn lease_observes_silent_expiry_without_holding_loss_read_guard() {
        let (s, p, mut c) = fixture();
        c.valid_for = Duration::from_millis(20);
        let (t, mut owner) = Timeline::new(
            s,
            Duration::from_secs(1),
            Duration::from_micros(1),
            Duration::ZERO,
        )
        .unwrap();
        owner.update(s, p, c).unwrap();
        let (_user, upstream) = watch::channel(false);
        let lease = crate::media::MasterLease::spawn_external(t.clone(), upstream);
        let mut stop = lease.subscribe();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !*stop.borrow_and_update() {
                stop.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert!(*t.loss().borrow());
        assert!(owner.update(s, p, c).is_err());
    }
    #[test]
    fn silent_expiry_revokes_cached_clock() {
        let (s, p, c) = fixture();
        let (t, mut owner) = Timeline::new(
            s,
            Duration::from_secs(1),
            Duration::from_micros(1),
            Duration::ZERO,
        )
        .unwrap();
        owner.update(s, p, c).unwrap();
        t.shared
            .state
            .lock()
            .unwrap()
            .observation
            .as_mut()
            .unwrap()
            .correlation
            .valid_for = Duration::from_nanos(1);
        assert!(t.reading_at(Instant::now()).is_err());
        assert!(*t.loss().borrow());
        assert!(owner.update(s, p, c).is_err());
    }
}
