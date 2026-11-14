//! Explicit shared-clock host seam; no daemon or automatic startup.
pub use crate::ptp_group::{ExternalPeerControl, PeerControlUpdate};
use anyhow::Result;
use clock::external::Selection;
use std::{io, path::Path, sync::Arc, time::Duration};
use timesync::{
    external_acquisition::{Acquisition, Policy, Progress},
    external_media::Timeline,
    nqptp_reader::Reader,
};

/// Retained by the host for the entire relay lifetime. Drop or failed evidence
/// revokes all groups; stopping a group does not stop the external daemon.
pub struct ExternalClock {
    owner: Acquisition,
    timeline: Arc<Timeline>,
}
/// Cloneable group factory; acquisition remains owned by one independent task.
/// Keeping this handle does not keep revoked clock observations valid.
#[derive(Clone)]
pub struct ExternalClockHandle {
    timeline: Arc<Timeline>,
}
impl ExternalClockHandle {
    pub fn domain(&self) -> clock::domain::Domain {
        self.timeline.domain()
    }
    pub fn group(
        &self,
        id: u128,
        owner: u128,
        control: Arc<dyn ExternalPeerControl>,
        stop: tokio::sync::watch::Receiver<bool>,
    ) -> Result<crate::ptp_group::PtpGroup> {
        crate::ptp_group::PtpGroup::new_external(self.timeline.clone(), id, stop)?
            .with_external_peer_control(owner, control)
    }
}
impl ExternalClock {
    pub fn handle(&self) -> ExternalClockHandle {
        ExternalClockHandle {
            timeline: self.timeline.clone(),
        }
    }
    /// The caller attests the installed daemon/patch and selected master role.
    /// Supplying SHM-derived IDs alone is insufficient. Construction reads only
    /// the explicit file metadata; it neither publishes nor binds PTP ports.
    pub fn open(
        path: &Path,
        uid: u32,
        selection: Selection,
        policy: Policy,
        max_age: Duration,
        max_uncertainty: Duration,
        max_ahead: Duration,
    ) -> Result<Self> {
        let reader = Reader::open(path, uid)?;
        let (timeline, publisher) = Timeline::new(selection, max_age, max_uncertainty, max_ahead)?;
        let owner = Acquisition::new(reader, publisher, selection, policy)?;
        Ok(Self { owner, timeline })
    }
    /// Invoke from the host acquisition scheduler, never an audio callback.
    pub fn poll(&mut self, evidence: impl FnMut() -> io::Result<Selection>) -> Result<Progress> {
        Ok(self.owner.poll(evidence)?)
    }
    /// A dead command owner must also revoke audio clock consumers. Cancelling
    /// the acquisition future drops its publisher; no extra timer or daemon is
    /// created. Independent selection evidence is still mandatory.
    pub async fn maintain_with_control(
        self,
        period: Duration,
        evidence: impl FnMut() -> io::Result<Selection>,
        stop: tokio::sync::watch::Receiver<bool>,
        control: &crate::nqptp_control::NqptpControl,
    ) -> Result<()> {
        let mut loss = control.loss();
        let lost = async {
            loop {
                if *loss.borrow() || loss.changed().await.is_err() {
                    return;
                }
            }
        };
        tokio::select! {
            biased;
            _ = lost => anyhow::bail!("external clock command owner lost"),
            result = self.maintain(period, evidence, stop) => result,
        }
    }
    /// Host-owned acquisition loop. Run outside audio callbacks; `evidence`
    /// must perform bounded local checks, never network I/O or daemon startup.
    /// Consuming the owner ensures stop, channel closure, errors and task
    /// cancellation all revoke the timeline. The external daemon stays alive.
    pub async fn maintain(
        mut self,
        period: Duration,
        mut evidence: impl FnMut() -> io::Result<Selection>,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        anyhow::ensure!(
            !period.is_zero() && period <= Duration::from_secs(1),
            "external clock polling period must be within (0, 1s]"
        );
        let mut ticks = tokio::time::interval(period);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if *stop.borrow() || stop.has_changed().is_err() {
                return Ok(());
            }
            tokio::select! {
                biased;
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() { return Ok(()); }
                }
                _ = ticks.tick() => { self.poll(&mut evidence)?; }
            }
        }
    }
    /// Fails until a measured observation has actually been published. Native
    /// peer union/master commands remain owned by the existing host controller.
    pub fn group(
        &self,
        id: u128,
        owner: u128,
        control: Arc<dyn ExternalPeerControl>,
        stop: tokio::sync::watch::Receiver<bool>,
    ) -> Result<crate::ptp_group::PtpGroup> {
        self.handle().group(id, owner, control, stop)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use clock::{domain::Domain, external::StartMarker};
    use std::{
        os::unix::fs::MetadataExt,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct UnusedPeers;
    impl ExternalPeerControl for UnusedPeers {
        fn replace(&self, _: Domain, _: u128, _: Vec<std::net::IpAddr>) -> PeerControlUpdate<'_> {
            panic!("peer commands must not run before first publication")
        }
    }
    fn fixture() -> (std::path::PathBuf, ExternalClock) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "external-clock-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, [0u8; 72]).unwrap();
        let uid = std::fs::metadata(&path).unwrap().uid();
        let selection = Selection {
            daemon_instance: 1,
            domain: Domain::new(9, 1).unwrap(),
            master_start_raw_ns: 100,
            start_marker: StartMarker::RefreshedPublication,
            clock_change_offset: None,
            sender_master_verified: true,
        };
        let policy = Policy {
            min_interval: Duration::from_millis(100),
            max_bracket: Duration::from_millis(10),
            capture_uncertainty_ns: 1000,
            max_capture_uncertainty: Duration::from_millis(1),
            wander_ppb: 100,
            valid_for: Duration::from_secs(1),
        };
        let host = ExternalClock::open(
            &path,
            uid,
            selection,
            policy,
            Duration::from_secs(1),
            Duration::from_millis(1),
            Duration::ZERO,
        )
        .unwrap();
        (path, host)
    }
    #[test]
    fn no_group_before_publication_and_evidence_failure_revokes() {
        let (path, mut host) = fixture();
        let (_stop, receiver) = tokio::sync::watch::channel(false);
        assert!(host.group(7, 99, Arc::new(UnusedPeers), receiver).is_err());
        assert!(
            host.poll(|| Err(io::Error::other("lost daemon evidence")))
                .is_err()
        );
        assert!(*host.timeline.loss().borrow());
        assert!(
            host.poll(|| panic!("cannot reacquire revoked owner"))
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn scheduler_stop_or_closed_channel_never_polls_and_revokes() {
        for closed in [false, true] {
            let (path, host) = fixture();
            let timeline = host.timeline.clone();
            let (stop, receiver) = tokio::sync::watch::channel(!closed);
            if closed {
                drop(stop);
            }
            host.maintain(
                Duration::from_millis(100),
                || panic!("stopped owner must not inspect evidence"),
                receiver,
            )
            .await
            .unwrap();
            assert!(*timeline.loss().borrow());
            std::fs::remove_file(path).unwrap();
        }
    }
    #[tokio::test]
    async fn scheduler_evidence_failure_revokes_and_exits() {
        let (path, host) = fixture();
        let timeline = host.timeline.clone();
        let (_stop, receiver) = tokio::sync::watch::channel(false);
        let mut calls = 0;
        assert!(
            host.maintain(
                Duration::from_millis(100),
                || {
                    calls += 1;
                    Err(io::Error::other("daemon identity changed"))
                },
                receiver
            )
            .await
            .is_err()
        );
        assert_eq!(calls, 1);
        assert!(*timeline.loss().borrow());
        std::fs::remove_file(path).unwrap();
    }
    #[tokio::test]
    async fn lost_command_owner_revokes_acquisition_without_polling() {
        let (path, host) = fixture();
        let timeline = host.timeline.clone();
        let selected = Selection {
            daemon_instance: 1,
            domain: Domain::new(9, 1).unwrap(),
            master_start_raw_ns: 100,
            start_marker: StartMarker::RefreshedPublication,
            clock_change_offset: None,
            sender_master_verified: true,
        };
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (control, worker) = crate::nqptp_control::NqptpWorker::new(
            socket,
            server.local_addr().unwrap(),
            "/test".into(),
            selected,
        )
        .unwrap();
        drop(worker);
        let (_stop, receiver) = tokio::sync::watch::channel(false);
        assert!(
            host.maintain_with_control(
                Duration::from_millis(100),
                || panic!("lost control must prevent acquisition"),
                receiver,
                &control,
            )
            .await
            .is_err()
        );
        assert!(*timeline.loss().borrow());
        std::fs::remove_file(path).unwrap();
    }
}
