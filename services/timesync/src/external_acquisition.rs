//! Explicit acquisition owner for an already attested external NQPTP daemon.
//! Call poll on a host scheduling/blocking path, never the PCM callback. No
//! listener, peer command, task, sleep or automatic daemon startup is created.
use crate::{
    external_media::Publisher,
    nqptp_reader::Reader,
    raw_correlation::{self, Bracket},
};
use clock::external::Selection;
use std::{io, time::Duration};

pub struct Policy {
    pub min_interval: Duration,
    pub max_bracket: Duration,
    pub capture_uncertainty_ns: u64,
    pub max_capture_uncertainty: Duration,
    pub wander_ppb: u64,
    pub valid_for: Duration,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Progress {
    Warming,
    Published,
    Unpublished,
}

/// Sole publisher; dropping it revokes consumers. The host supplies independent
/// daemon instance/role/epoch evidence on EVERY poll, both before and after I/O.
/// A successful SHM read is never treated as such evidence.
pub struct Acquisition {
    reader: Reader,
    publisher: Publisher,
    selection: Selection,
    policy: Policy,
    first: Option<Bracket>,
    failed: bool,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
impl Acquisition {
    pub fn new(
        reader: Reader,
        publisher: Publisher,
        selection: Selection,
        policy: Policy,
    ) -> io::Result<Self> {
        if policy.min_interval.is_zero()
            || policy.min_interval > Duration::from_secs(10)
            || policy.max_bracket.is_zero()
            || policy.max_bracket > Duration::from_secs(1)
            || policy.max_capture_uncertainty.is_zero()
            || policy.wander_ppb > 1_000_000
            || policy.valid_for.is_zero()
            || policy.valid_for > Duration::from_secs(10)
        {
            return Err(invalid("invalid external acquisition policy"));
        }
        Ok(Self {
            reader,
            publisher,
            selection,
            policy,
            first: None,
            failed: false,
        })
    }
    /// Evidence errors/changes, clock capture errors and invalid snapshots revoke
    /// permanently. A torn/unpublished snapshot alone is retryable; timeline
    /// freshness/lease expiry still apply, and stale publication is not renewed.
    pub fn poll(
        &mut self,
        mut evidence: impl FnMut() -> io::Result<Selection>,
    ) -> io::Result<Progress> {
        if self.failed {
            return Err(invalid("external acquisition revoked"));
        }
        let result = (|| {
            verify(self.selection, evidence()?)?;
            let bracket = raw_correlation::capture(
                self.policy.max_bracket,
                self.policy.capture_uncertainty_ns,
            )?;
            let snapshot = self.reader.snapshot();
            verify(self.selection, evidence()?)?;
            let sample = match snapshot {
                Ok(sample) => sample,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(Progress::Unpublished);
                }
                Err(error) => return Err(error),
            };
            let Some(first) = self.first else {
                self.first = Some(bracket);
                return Ok(Progress::Warming);
            };
            if bracket
                .at()
                .checked_duration_since(first.at())
                .ok_or_else(|| invalid("reversed acquisition"))?
                < self.policy.min_interval
            {
                return Ok(Progress::Warming);
            }
            let correlation = raw_correlation::estimate(
                first,
                bracket,
                self.policy.wander_ppb,
                self.policy.valid_for,
                self.policy.max_capture_uncertainty,
            )?;
            self.publisher.update(self.selection, sample, correlation)?;
            self.first = Some(bracket);
            Ok(Progress::Published)
        })();
        if result.is_err() {
            self.failed = true;
            self.publisher.revoke();
        }
        result
    }
}
fn verify(expected: Selection, actual: Selection) -> io::Result<()> {
    if expected != actual || !actual.sender_master_verified {
        return Err(invalid("external daemon selection/role evidence changed"));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use clock::{domain::Domain, external::StartMarker};
    #[test]
    fn instance_epoch_and_role_cannot_be_relabelled() {
        let selection = Selection {
            daemon_instance: 1,
            domain: Domain::new(9, 1).unwrap(),
            master_start_raw_ns: 100,
            start_marker: StartMarker::RefreshedPublication,
            clock_change_offset: None,
            sender_master_verified: true,
        };
        assert!(verify(selection, selection).is_ok());
        let mut changed = selection;
        changed.daemon_instance += 1;
        assert!(verify(selection, changed).is_err());
        changed = selection;
        changed.master_start_raw_ns += 1;
        assert!(verify(selection, changed).is_err());
        changed = selection;
        changed.sender_master_verified = false;
        assert!(verify(selection, changed).is_err());
    }
    #[test]
    fn evidence_failure_latches_loss_and_cannot_rearm() {
        use crate::external_media::Timeline;
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "external-acquisition-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, [0u8; 72]).unwrap();
        let selection = Selection {
            daemon_instance: 1,
            domain: Domain::new(9, 1).unwrap(),
            master_start_raw_ns: 100,
            start_marker: StartMarker::RefreshedPublication,
            clock_change_offset: None,
            sender_master_verified: true,
        };
        let (timeline, publisher) = Timeline::new(
            selection,
            Duration::from_secs(1),
            Duration::from_millis(1),
            Duration::ZERO,
        )
        .unwrap();
        let reader = Reader::open(&path, unsafe { libc::geteuid() }).unwrap();
        let mut owner = Acquisition::new(
            reader,
            publisher,
            selection,
            Policy {
                min_interval: Duration::from_millis(100),
                max_bracket: Duration::from_millis(10),
                capture_uncertainty_ns: 1000,
                max_capture_uncertainty: Duration::from_millis(1),
                wander_ppb: 100,
                valid_for: Duration::from_secs(1),
            },
        )
        .unwrap();
        assert!(
            owner
                .poll(|| Err(io::Error::other("daemon exited")))
                .is_err()
        );
        assert!(*timeline.loss().borrow());
        assert!(
            owner
                .poll(|| panic!("must not reacquire after loss"))
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }
}
