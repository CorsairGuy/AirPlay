//! Application-facing relay handle. No files, device routes or protocol translation.
use anyhow::{Result, bail};
use sender::{MediaProgress, Volume};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

#[derive(Clone, Debug)]
pub struct TargetStatus {
    pub endpoint: SocketAddr,
    /// Desired value; not an acknowledgement from the receiver.
    pub requested_volume: Volume,
    /// UDP transmission progress; not an acoustic playback acknowledgement.
    pub media: Option<MediaProgress>,
}
struct Target {
    volume: Gain,
    media: watch::Receiver<Option<MediaProgress>>,
}
enum Gain {
    #[cfg(test)]
    Watch(watch::Sender<Volume>),
    Session(sender::pool::SessionHandle),
}
impl Gain {
    fn requested(&self) -> Volume {
        match self {
            #[cfg(test)]
            Self::Watch(v) => *v.borrow(),
            Self::Session(v) => v.requested_volume(),
        }
    }
    fn available(&self) -> bool {
        match self {
            #[cfg(test)]
            Self::Watch(v) => !v.is_closed(),
            Self::Session(v) => *v.state.borrow() == sender::pool::SessionState::Running,
        }
    }
    fn request(&self, volume: Volume) -> Result<()> {
        match self {
            #[cfg(test)]
            Self::Watch(v) => {
                v.send_replace(volume);
                Ok(())
            }
            Self::Session(v) => v.set_volume(volume),
        }
    }
}
#[derive(Default)]
struct State {
    active: bool,
    expected: usize,
    targets: HashMap<SocketAddr, Target>,
}
#[derive(Clone, Default)]
pub struct RelayControl(Arc<Mutex<State>>);
impl RelayControl {
    /// Startup observation only: successful sends to every configured target.
    /// Not proof of acoustic readiness, ongoing health or clock alignment.
    pub fn all_media_started(&self) -> bool {
        let state = self.0.lock().unwrap();
        state.active
            && state.expected > 0
            && state.targets.len() == state.expected
            && state.targets.values().all(|v| v.media.borrow().is_some())
    }
    pub fn snapshot(&self) -> Vec<TargetStatus> {
        self.status_snapshot().1
    }
    pub fn status_snapshot(&self) -> (bool, Vec<TargetStatus>) {
        let state = self.0.lock().unwrap();
        let mut targets: Vec<_> = state
            .targets
            .iter()
            .map(|(endpoint, target)| TargetStatus {
                endpoint: *endpoint,
                requested_volume: target.volume.requested(),
                media: *target.media.borrow(),
            })
            .collect();
        targets.sort_by_key(|v| v.endpoint);
        let started = state.active
            && state.expected > 0
            && targets.len() == state.expected
            && targets.iter().all(|v| v.media.is_some());
        (started, targets)
    }
    pub fn set_volume(&self, endpoint: SocketAddr, volume: Volume) -> Result<()> {
        let state = self.0.lock().unwrap();
        let Some(target) = state.targets.get(&endpoint) else {
            bail!("unknown or inactive relay target")
        };
        target.volume.request(volume)
    }
    /// One logical output row, e.g. a stereo pair. This sets its absolute level;
    /// overall multiroom master-volume balance is a separate adapter policy.
    pub fn set_route_volume(
        &self,
        catalog: &sender::outputs::OutputCatalog,
        uid: &str,
        volume: Volume,
    ) -> Result<()> {
        let receivers = catalog.receivers(&[uid.to_owned()])?;
        let state = self.0.lock().unwrap();
        if !state.active
            || receivers.iter().any(|receiver| {
                state
                    .targets
                    .get(&receiver.address)
                    .is_none_or(|target| !target.volume.available())
            })
        {
            bail!("logical output has inactive members");
        }
        for receiver in receivers {
            state.targets[&receiver.address].volume.request(volume)?;
        }
        Ok(())
    }
    pub fn set_member_volume(
        &self,
        catalog: &sender::outputs::OutputCatalog,
        uid: &str,
        volume: Volume,
    ) -> Result<()> {
        self.set_volume(catalog.receiver(uid)?.address, volume)
    }
    pub fn progress(&self, endpoint: SocketAddr) -> Option<watch::Receiver<Option<MediaProgress>>> {
        self.0
            .lock()
            .unwrap()
            .targets
            .get(&endpoint)
            .map(|v| v.media.clone())
    }
    pub(crate) fn claim(&self, expected: usize) -> Result<RunGuard> {
        let mut state = self.0.lock().unwrap();
        if state.active {
            bail!("relay control handle already belongs to a running group")
        }
        if !(1..=8).contains(&expected) {
            bail!("invalid relay target count")
        }
        state.active = true;
        state.expected = expected;
        Ok(RunGuard(self.clone()))
    }
    #[cfg(test)]
    pub(crate) fn register(
        &self,
        endpoint: SocketAddr,
        volume: watch::Sender<Volume>,
        media: watch::Receiver<Option<MediaProgress>>,
    ) {
        self.0.lock().unwrap().targets.insert(
            endpoint,
            Target {
                volume: Gain::Watch(volume),
                media,
            },
        );
    }
    pub(crate) fn register_session(
        &self,
        endpoint: SocketAddr,
        handle: sender::pool::SessionHandle,
    ) {
        let media = handle.progress.clone();
        self.0.lock().unwrap().targets.insert(
            endpoint,
            Target {
                volume: Gain::Session(handle),
                media,
            },
        );
    }
    pub(crate) fn invalidate(&self) {
        self.0.lock().unwrap().targets.clear();
    }
}
pub(crate) struct RunGuard(RelayControl);
impl Drop for RunGuard {
    fn drop(&mut self) {
        let mut state = self.0.0.lock().unwrap();
        state.targets.clear();
        state.active = false;
        state.expected = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logical_row_volume_is_atomic_and_member_volume_remains_independent() {
        use sender::outputs::{OutputCatalog, OutputRoute, ReceiverEndpoint};
        let a = "127.0.0.1:7001".parse().unwrap();
        let b = "127.0.0.1:7002".parse().unwrap();
        let catalog = OutputCatalog::new(vec![OutputRoute {
            uid: "theater".into(),
            name: "Home Theater".into(),
            model: "Group".into(),
            aliases: vec!["group-alias".into()],
            receivers: vec![
                ReceiverEndpoint {
                    uid: "a".into(),
                    address: a,
                },
                ReceiverEndpoint {
                    uid: "b".into(),
                    address: b,
                },
            ],
        }])
        .unwrap();
        let control = RelayControl::default();
        let _guard = control.claim(2).unwrap();
        let (va, ra) = watch::channel(Volume::db(-30.0).unwrap());
        let (_, ma) = watch::channel(None);
        control.register(a, va, ma);
        assert!(
            control
                .set_route_volume(&catalog, "theater", Volume::db(-20.0).unwrap())
                .is_err()
        );
        assert_eq!(ra.borrow().as_db(), -30.0);
        let (vb, rb) = watch::channel(Volume::db(-30.0).unwrap());
        let (_, mb) = watch::channel(None);
        control.register(b, vb, mb);
        control
            .set_member_volume(&catalog, "a", Volume::db(-25.0).unwrap())
            .unwrap();
        assert_eq!(ra.borrow().as_db(), -25.0);
        assert_eq!(rb.borrow().as_db(), -30.0);
        control
            .set_route_volume(&catalog, "group-alias", Volume::db(-20.0).unwrap())
            .unwrap();
        assert_eq!(ra.borrow().as_db(), -20.0);
        assert_eq!(rb.borrow().as_db(), -20.0);
    }
    #[test]
    fn targets_have_independent_coalesced_volumes_and_stale_handles_are_cleared() {
        let handle = RelayControl::default();
        assert!(!handle.all_media_started());
        let guard = handle.claim(2).unwrap();
        assert!(handle.claim(2).is_err());
        let a = "127.0.0.1:7000".parse().unwrap();
        let b = "127.0.0.2:7000".parse().unwrap();
        let (va, ra) = watch::channel(Volume::db(-30.0).unwrap());
        let (vb, rb) = watch::channel(Volume::db(-30.0).unwrap());
        let (ma_tx, ma) = watch::channel(None);
        let (mb_tx, mb) = watch::channel(None);
        handle.register(a, va, ma);
        handle.register(b, vb, mb);
        handle.set_volume(a, Volume::db(-21.0).unwrap()).unwrap();
        handle.set_volume(a, Volume::db(-20.0).unwrap()).unwrap();
        assert_eq!(ra.borrow().as_db(), -20.0);
        assert_eq!(rb.borrow().as_db(), -30.0);
        assert!(handle.snapshot().iter().all(|v| v.media.is_none()));
        assert!(!handle.all_media_started());
        let event = MediaProgress {
            packets_sent: 1,
            frame_index: 0,
            play_at: std::time::Instant::now(),
        };
        ma_tx.send_replace(Some(event));
        assert!(!handle.all_media_started());
        mb_tx.send_replace(Some(event));
        assert!(handle.all_media_started());
        drop(guard);
        assert!(handle.snapshot().is_empty());
        assert!(handle.set_volume(a, Volume::db(-25.0).unwrap()).is_err());
        assert!(!handle.all_media_started());
        assert!(handle.claim(2).is_ok());
    }
}
