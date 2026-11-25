//! Dynamic ownership of already authenticated receiver sessions. The caller owns
//! discovery, pairing and the common paced PCM producer. No implicit reconnects.
use crate::tx::{AudioPacket, MediaProgress, Session, StreamStats, Volume};
use anyhow::{Result, bail};
use std::{collections::BTreeMap, future::Future, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    sync::{broadcast, watch},
    task::JoinHandle,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Running,
    Ended,
    Failed,
}

#[derive(Clone)]
pub struct SessionHandle {
    volume: watch::Sender<Volume>,
    peers: Option<crate::tx::PtpPeerControl>,
    pub state: watch::Receiver<SessionState>,
    pub progress: watch::Receiver<Option<MediaProgress>>,
}
impl SessionHandle {
    pub fn ptp_peer_control(&self) -> Option<crate::tx::PtpPeerControl> {
        self.peers.clone()
    }
    pub fn requested_volume(&self) -> Volume {
        *self.volume.borrow()
    }
    pub fn set_volume(&self, volume: Volume) -> Result<()> {
        if *self.state.borrow() != SessionState::Running {
            bail!("receiver session ended");
        }
        self.volume
            .send(volume)
            .map_err(|_| anyhow::anyhow!("receiver session ended"))
    }
}
struct Entry {
    endpoint: SocketAddr,
    stop: watch::Sender<bool>,
    handle: SessionHandle,
    task: Option<JoinHandle<Result<StreamStats>>>,
    shutdown_deadline: Duration,
}
struct Completion(watch::Sender<SessionState>);
impl Drop for Completion {
    fn drop(&mut self) {
        if *self.0.borrow() == SessionState::Running {
            self.0.send_replace(SessionState::Failed);
        }
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        // Existing Session waits and control requests have bounded deadlines.
        // Signal graceful teardown; do not abort before it can send TEARDOWN.
        self.stop.send_replace(true);
    }
}
pub struct SessionPool {
    audio: broadcast::Receiver<Arc<AudioPacket>>,
    entries: BTreeMap<String, Entry>,
}

/// A physical receiver session currently owned by this pool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalMember {
    pub uid: String,
    pub address: SocketAddr,
    pub running: bool,
}
impl SessionPool {
    /// Queue one peer list to a single-domain PTP pool after complete local
    /// preflight. RTSP acceptance is observed on each control's applied watch;
    /// it does not prove group adoption. No session/packetizer is recreated.
    pub fn set_ptp_peers(&self, receivers: &[std::net::IpAddr]) -> Result<()> {
        self.update_ptp_peers(receivers, false)
    }
    /// Reconcile the peer list against actual running workers, not requested
    /// discovery rows. Ended workers do not keep a peer alive. Canonical peer
    /// controls suppress duplicates; no session/clock/RTP stream is restarted.
    pub fn sync_ptp_members(&self) -> Result<Vec<std::net::IpAddr>> {
        let mut ips: Vec<_> = self
            .entries
            .values()
            .filter(|entry| *entry.handle.state.borrow() == SessionState::Running)
            .map(|entry| entry.endpoint.ip())
            .collect();
        ips.sort_unstable();
        ips.dedup();
        if !ips.is_empty() {
            self.update_ptp_peers(&ips, true)?;
        }
        Ok(ips)
    }
    fn update_ptp_peers(&self, receivers: &[std::net::IpAddr], running_only: bool) -> Result<()> {
        if self.entries.is_empty() {
            bail!("no PTP sessions");
        }
        let mut context = None;
        let mut pending = Vec::new();
        for entry in self.entries.values() {
            if *entry.handle.state.borrow() != SessionState::Running {
                if running_only {
                    continue;
                }
                bail!("inactive PTP member");
            }
            let control = entry
                .handle
                .peers
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("non-PTP member in peer update"))?;
            let update = control.prepare(receivers)?;
            let identity = (update.domain(), update.group());
            if context.is_some_and(|current| current != identity) {
                bail!("PTP pool has different group/domain");
            }
            context = Some(identity);
            pending.push((control, update));
        }
        for (control, update) in pending {
            control.publish(update)?;
        }
        Ok(())
    }
    pub fn new(audio: broadcast::Receiver<Arc<AudioPacket>>) -> Self {
        Self {
            audio,
            entries: BTreeMap::new(),
        }
    }
    pub fn handle(&self, uid: &str) -> Option<SessionHandle> {
        self.entries.get(uid).map(|entry| entry.handle.clone())
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    /// Physical workers currently owned by this pool. Task membership alone
    /// is not media progress, acoustic readiness or native group adoption.
    pub fn members(&self) -> Vec<PhysicalMember> {
        self.entries
            .iter()
            .map(|(uid, entry)| PhysicalMember {
                uid: uid.clone(),
                address: entry.endpoint,
                running: *entry.handle.state.borrow() == SessionState::Running,
            })
            .collect()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Apply distinct requested gains to physical session UIDs. Reject an
    /// invalid/inactive/duplicate batch before changing any watch. As with
    /// individual gain, later receiver failure can race an accepted revision.
    pub fn set_member_volumes(&self, levels: &[(String, Volume)]) -> Result<()> {
        if levels.is_empty() || levels.len() > 64 {
            bail!("invalid volume batch size");
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut changes = Vec::new();
        for (uid, gain) in levels {
            if !seen.insert(uid) {
                bail!("duplicate member volume");
            }
            let handle = self
                .handle(uid)
                .ok_or_else(|| anyhow::anyhow!("inactive volume member"))?;
            if *handle.state.borrow() != SessionState::Running {
                bail!("inactive volume member");
            }
            changes.push((handle, *gain));
        }
        for (handle, gain) in changes {
            handle.set_volume(gain)?;
        }
        Ok(())
    }

    /// Accept a prepared session without resetting existing targets or media clock.
    pub fn add(&mut self, uid: &str, session: Session) -> Result<SessionHandle> {
        let endpoint = session.endpoint();
        let volume = session.initial_volume();
        let deadline = session.shutdown_deadline();
        let peers = session.ptp_peer_control();
        let mut handle = self.launch(
            uid,
            endpoint,
            volume,
            deadline,
            move |audio, volume, stop, progress| {
                session.run_with_progress(audio, volume, stop, progress)
            },
        )?;
        handle.peers = peers.clone();
        self.entries
            .get_mut(uid)
            .expect("just inserted session")
            .handle
            .peers = peers;
        Ok(handle)
    }
    fn launch<F, Fut>(
        &mut self,
        uid: &str,
        endpoint: SocketAddr,
        volume: Volume,
        deadline: Duration,
        worker: F,
    ) -> Result<SessionHandle>
    where
        F: FnOnce(
            broadcast::Receiver<Arc<AudioPacket>>,
            watch::Receiver<Volume>,
            watch::Receiver<bool>,
            watch::Sender<Option<MediaProgress>>,
        ) -> Fut,
        Fut: Future<Output = Result<StreamStats>> + Send + 'static,
    {
        if uid.is_empty()
            || uid.len() > 256
            || uid.chars().any(char::is_control)
            || self.entries.contains_key(uid)
            || self.entries.len() >= 64
            || self
                .entries
                .values()
                .any(|entry| entry.endpoint == endpoint)
        {
            bail!("duplicate or invalid receiver session");
        }
        let (volume, volumes) = watch::channel(volume);
        let (stop, stopped) = watch::channel(false);
        let (progress, observed) = watch::channel(None);
        let (state, status) = watch::channel(SessionState::Running);
        let handle = SessionHandle {
            volume,
            peers: None,
            state: status,
            progress: observed,
        };
        let future = worker(self.audio.resubscribe(), volumes, stopped, progress);
        let completion = Completion(state);
        let task = tokio::spawn(async move {
            let completion = completion;
            let result = future.await;
            completion.0.send_replace(if result.is_ok() {
                SessionState::Ended
            } else {
                SessionState::Failed
            });
            result
        });
        self.entries.insert(
            uid.into(),
            Entry {
                endpoint,
                stop,
                handle: handle.clone(),
                task: Some(task),
                shutdown_deadline: deadline,
            },
        );
        Ok(handle)
    }
    pub async fn remove(&mut self, uid: &str) -> Result<StreamStats> {
        self.finish(uid, true).await
    }
    async fn finish(&mut self, uid: &str, stop: bool) -> Result<StreamStats> {
        let mut entry = self
            .entries
            .remove(uid)
            .ok_or_else(|| anyhow::anyhow!("unknown receiver session"))?;
        if stop {
            entry.stop.send_replace(true);
        }
        let mut task = entry.task.take().unwrap();
        let deadline = entry.shutdown_deadline
            + if stop {
                Duration::ZERO
            } else {
                Duration::from_secs(2)
            };
        match tokio::time::timeout(deadline, &mut task).await {
            Ok(result) => result?,
            Err(_) => {
                task.abort();
                let _ = task.await;
                bail!("receiver shutdown deadline exceeded");
            }
        }
    }
    pub async fn shutdown(&mut self) -> Vec<(String, Result<StreamStats>)> {
        for entry in self.entries.values() {
            entry.stop.send_replace(true);
        }
        let ids: Vec<_> = self.entries.keys().cloned().collect();
        let mut results = Vec::new();
        for uid in ids {
            let result = self.remove(&uid).await;
            results.push((uid, result));
        }
        results
    }
    /// Wait for source EOF and each receiver's scheduled tail, without cutting
    /// buffered audio by sending stop. Caller must close its PCM producer first.
    pub async fn drain(&mut self) -> Vec<(String, Result<StreamStats>)> {
        let ids: Vec<_> = self.entries.keys().cloned().collect();
        let mut results = Vec::new();
        for uid in ids {
            let result = self.finish(&uid, false).await;
            results.push((uid, result));
        }
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tx::{MediaClock, PACKET_BYTES};
    use tokio::sync::mpsc;
    fn add(
        pool: &mut SessionPool,
        id: &str,
        port: u16,
        events: mpsc::UnboundedSender<(String, u64)>,
    ) -> SessionHandle {
        let uid = id.to_owned();
        pool.launch(id,([127,0,0,1],port).into(),Volume::db(-30.0).unwrap(),Duration::from_secs(1),move|mut audio,_volume,mut stop,_progress|async move{
            let _volumes = _volume;
            let mut stats=StreamStats::default();
            loop {
                if *stop.borrow(){break;}
                tokio::select! {
                    packet=audio.recv()=>match packet {Ok(packet)=>{events.send((uid.clone(),packet.frame_index())).unwrap();stats.packets_sent+=1;},Err(_)=>break},
                    _=stop.changed()=>break,
                }
            }
            Ok(stats)
        }).unwrap()
    }
    #[tokio::test]
    async fn ptp_reconciliation_rejects_running_ntp_workers_and_accepts_empty_pool() {
        let (_source, packets) = broadcast::channel(4);
        let mut pool = SessionPool::new(packets);
        assert!(pool.sync_ptp_members().unwrap().is_empty());
        let (events, _received) = mpsc::unbounded_channel();
        let handle = add(&mut pool, "ntp", 7001, events);
        assert!(pool.sync_ptp_members().is_err());
        assert_eq!(*handle.state.borrow(), SessionState::Running);
        pool.shutdown().await;
        assert!(pool.sync_ptp_members().unwrap().is_empty());
    }
    #[tokio::test]
    async fn group_volume_batch_keeps_balance_and_rejects_missing_members() {
        let (_source, packets) = broadcast::channel(16);
        let mut pool = SessionPool::new(packets);
        let (events, _received) = mpsc::unbounded_channel();
        let a = add(&mut pool, "a", 7001, events.clone());
        let b = add(&mut pool, "b", 7002, events);
        let mut policy = coordinator::group_volume::GroupVolume::default();
        let levels = policy
            .plan(&[("a".into(), 0.1), ("b".into(), 0.3)], 0.2)
            .unwrap();
        let gains: Vec<_> = levels
            .iter()
            .map(|(id, v)| (id.clone(), Volume::normalized(*v).unwrap()))
            .collect();
        pool.set_member_volumes(&gains).unwrap();
        assert_eq!(a.requested_volume(), gains[0].1);
        assert_eq!(b.requested_volume(), gains[1].1);
        assert_ne!(a.requested_volume(), b.requested_volume());
        assert!(
            pool.set_member_volumes(&[
                ("a".into(), Volume::MUTE),
                ("missing".into(), Volume::MUTE)
            ])
            .is_err()
        );
        assert_eq!(a.requested_volume(), gains[0].1);
        assert!(
            pool.set_member_volumes(&[("a".into(), Volume::MUTE), ("a".into(), Volume::MUTE)])
                .is_err()
        );
        pool.shutdown().await;
    }
    #[tokio::test]
    async fn continuous_fanout_shares_packets_and_removal_preserves_survivor() {
        use crate::tx::continuous_source::{self, SourceFrame, State};
        let origin = std::time::Instant::now();
        let clock = MediaClock::at(origin, 0);
        let (audio, packets) = broadcast::channel(64);
        let mut pool = SessionPool::new(packets);
        let (events, mut received) = mpsc::unbounded_channel();
        let mut handles = Vec::new();
        for (id, port) in [("a", 7001), ("b", 7002)] {
            let events = events.clone();
            handles.push(pool.launch(id, ([127,0,0,1],port).into(), Volume::MUTE,
                Duration::from_secs(1), move |mut audio, _volume, mut stop, _progress| async move {
                    let mut stats = StreamStats::default();
                    loop {
                        tokio::select! {
                            biased;
                            _ = stop.changed() => break,
                            packet = audio.recv() => match packet {
                                Ok(packet) => { events.send((id, packet))?; stats.packets_sent += 1; },
                                Err(_) => break,
                            }
                        }
                    }
                    Ok(stats)
                }).unwrap());
        }
        let (source, input) = mpsc::channel(8);
        let (state, controls) = watch::channel(State::default());
        let (_stop, stop) = watch::channel(false);
        source
            .send(SourceFrame::new(0, Box::new([7; PACKET_BYTES])))
            .await
            .unwrap();
        let task = tokio::spawn(continuous_source::broadcast(
            crate::tx::continuous::Timeline::new(origin, Duration::from_millis(250)).unwrap(),
            input,
            controls,
            audio,
            stop,
            move |index, pcm| clock.packet(index, pcm),
        ));
        async fn next(
            received: &mut mpsc::UnboundedReceiver<(&'static str, Arc<AudioPacket>)>,
        ) -> (&'static str, Arc<AudioPacket>) {
            tokio::time::timeout(Duration::from_secs(1), received.recv())
                .await
                .unwrap()
                .unwrap()
        }
        let first = next(&mut received).await;
        let second = next(&mut received).await;
        assert_ne!(first.0, second.0);
        assert!(Arc::ptr_eq(&first.1, &second.1));
        assert_eq!(first.1.pcm()[0], 7);
        assert!(first.1.source_is_current());
        pool.remove("b").await.unwrap();
        assert_eq!(*handles[1].state.borrow(), SessionState::Ended);
        state.send_replace(State {
            generation: 1,
            paused: false,
        });
        assert!(!first.1.source_is_current());
        source
            .send(SourceFrame::new(1, Box::new([9; PACKET_BYTES])))
            .await
            .unwrap();
        let resumed = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let (id, packet) = next(&mut received).await;
                if packet.source_is_current() && packet.pcm()[0] == 9 {
                    assert_eq!(id, "a");
                    break packet;
                }
            }
        })
        .await
        .unwrap();
        assert!(resumed.frame_index() > first.1.frame_index());
        assert_eq!(*handles[0].state.borrow(), SessionState::Running);
        drop(source);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            continuous_source::End::Eof
        );
        let results = pool.shutdown().await;
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_ok());
        assert_eq!(*handles[0].state.borrow(), SessionState::Ended);
    }
    #[tokio::test]
    async fn add_remove_and_failure_do_not_restart_surviving_receivers() {
        let (audio, packets) = broadcast::channel(16);
        let mut pool = SessionPool::new(packets);
        let (events, mut received) = mpsc::unbounded_channel();
        let first = add(&mut pool, "a", 7001, events.clone());
        assert_eq!(
            pool.members(),
            vec![PhysicalMember {
                uid: "a".into(),
                address: ([127, 0, 0, 1], 7001).into(),
                running: true,
            }]
        );
        let clock = MediaClock::now();
        audio
            .send(Arc::new(clock.packet(0, &[0; PACKET_BYTES]).unwrap()))
            .unwrap();
        assert_eq!(received.recv().await.unwrap(), ("a".into(), 0));
        let second = add(&mut pool, "b", 7002, events);
        audio
            .send(Arc::new(clock.packet(352, &[0; PACKET_BYTES]).unwrap()))
            .unwrap();
        let mut pair = vec![
            received.recv().await.unwrap(),
            received.recv().await.unwrap(),
        ];
        pair.sort();
        assert_eq!(pair, vec![("a".into(), 352), ("b".into(), 352)]);
        assert_eq!(pool.remove("b").await.unwrap().packets_sent, 1);
        assert_eq!(*second.state.borrow(), SessionState::Ended);
        assert!(second.set_volume(Volume::MUTE).is_err());
        let mut failed = pool
            .launch(
                "failed",
                ([127, 0, 0, 1], 7003).into(),
                Volume::MUTE,
                Duration::from_secs(1),
                |_, _, _, _| async { bail!("test peer failure") },
            )
            .unwrap();
        while *failed.state.borrow_and_update() == SessionState::Running {
            failed.state.changed().await.unwrap();
        }
        assert_eq!(*failed.state.borrow(), SessionState::Failed);
        assert!(pool.remove("failed").await.is_err());
        audio
            .send(Arc::new(clock.packet(704, &[0; PACKET_BYTES]).unwrap()))
            .unwrap();
        assert_eq!(received.recv().await.unwrap(), ("a".into(), 704));
        assert_eq!(*first.state.borrow(), SessionState::Running);
        assert_eq!(pool.shutdown().await[0].1.as_ref().unwrap().packets_sent, 3);
        assert!(pool.is_empty());
    }
    #[tokio::test]
    async fn duplicate_endpoints_are_rejected_and_source_eof_ends_workers() {
        let (audio, packets) = broadcast::channel(16);
        let mut pool = SessionPool::new(packets);
        let (events, _) = mpsc::unbounded_channel();
        let handle = add(&mut pool, "a", 7001, events);
        let rejected = pool.launch(
            "b",
            ([127, 0, 0, 1], 7001).into(),
            Volume::MUTE,
            Duration::from_secs(1),
            |_, _, _, _| async { Ok(StreamStats::default()) },
        );
        assert!(rejected.is_err());
        let mut observed = handle.state.clone();
        drop(audio);
        tokio::time::timeout(Duration::from_secs(1), async {
            while *observed.borrow_and_update() == SessionState::Running {
                observed.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(*observed.borrow(), SessionState::Ended);
        assert_eq!(pool.remove("a").await.unwrap().packets_sent, 0);
        assert!(handle.set_volume(Volume::MUTE).is_err());
    }
    #[tokio::test]
    async fn stuck_worker_is_aborted_only_after_shutdown_deadline() {
        let (_audio, packets) = broadcast::channel(1);
        let mut pool = SessionPool::new(packets);
        let handle = pool
            .launch(
                "stuck",
                ([127, 0, 0, 1], 7000).into(),
                Volume::MUTE,
                Duration::from_millis(20),
                |_, _, _, _| std::future::pending::<Result<StreamStats>>(),
            )
            .unwrap();
        assert!(pool.remove("stuck").await.is_err());
        assert_eq!(*handle.state.borrow(), SessionState::Failed);
        assert!(pool.is_empty());
    }
    #[tokio::test]
    async fn draining_source_eof_keeps_queued_pcm_instead_of_signalling_stop() {
        let (audio, packets) = broadcast::channel(16);
        let mut pool = SessionPool::new(packets);
        let (events, mut received) = mpsc::unbounded_channel();
        add(&mut pool, "a", 7001, events);
        let clock = MediaClock::now();
        for index in [0, 352] {
            audio
                .send(Arc::new(clock.packet(index, &[0; PACKET_BYTES]).unwrap()))
                .unwrap();
        }
        drop(audio);
        let result = pool.drain().await;
        assert_eq!(result[0].1.as_ref().unwrap().packets_sent, 2);
        assert_eq!(received.recv().await.unwrap(), ("a".into(), 0));
        assert_eq!(received.recv().await.unwrap(), ("a".into(), 352));
    }
}

#[cfg(test)]
mod live_group_tests {
    use super::*;
    use crate::tx::{MediaClock, PACKET_BYTES};
    use tokio::sync::mpsc;

    /// Сбойная колонка удаляется и подключается заново на тот же endpoint,
    /// не перезапуская уже играющую соседнюю.
    #[tokio::test]
    async fn failed_member_is_replaced_without_restarting_survivor() {
        let (audio, packets) = broadcast::channel(16);
        let mut pool = SessionPool::new(packets);
        let (events, mut received) = mpsc::unbounded_channel();
        let survivor = pool
            .launch(
                "a",
                ([127, 0, 0, 1], 7101).into(),
                Volume::MUTE,
                Duration::from_secs(1),
                move |mut audio, _volume, mut stop, _progress| async move {
                    let mut stats = StreamStats::default();
                    loop {
                        tokio::select! {
                            _ = stop.changed() => break,
                            packet = audio.recv() => match packet {
                                Ok(packet) => { events.send(packet.frame_index())?; stats.packets_sent += 1; }
                                Err(_) => break,
                            }
                        }
                    }
                    Ok(stats)
                },
            )
            .unwrap();
        let mut failed = pool
            .launch(
                "b",
                ([127, 0, 0, 1], 7102).into(),
                Volume::MUTE,
                Duration::from_secs(1),
                |_, _, _, _| async { anyhow::bail!("speaker refused") },
            )
            .unwrap();
        while *failed.state.borrow_and_update() == SessionState::Running {
            failed.state.changed().await.unwrap();
        }
        assert_eq!(*failed.state.borrow(), SessionState::Failed);
        // Удаление сбойной сессии возвращает ошибку, но освобождает место.
        assert!(pool.remove("b").await.is_err());
        let replacement = pool
            .launch(
                "b",
                ([127, 0, 0, 1], 7102).into(),
                Volume::MUTE,
                Duration::from_secs(1),
                |mut audio, _volume, mut stop, _progress| async move {
                    let mut stats = StreamStats::default();
                    loop {
                        tokio::select! {
                            _ = stop.changed() => break,
                            packet = audio.recv() => match packet {
                                Ok(_) => stats.packets_sent += 1,
                                Err(_) => break,
                            }
                        }
                    }
                    Ok(stats)
                },
            )
            .expect("same endpoint may rejoin after failed member removal");
        assert_eq!(*replacement.state.borrow(), SessionState::Running);
        let clock = MediaClock::now();
        audio
            .send(Arc::new(clock.packet(0, &[0; PACKET_BYTES]).unwrap()))
            .unwrap();
        assert_eq!(received.recv().await.unwrap(), 0);
        assert_eq!(*survivor.state.borrow(), SessionState::Running);
        assert_eq!(pool.members().len(), 2);
        pool.shutdown().await;
    }
}
