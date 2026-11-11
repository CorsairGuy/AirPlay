//! Dynamic ownership of already authenticated receiver sessions. The caller owns
//! discovery, pairing and the common paced PCM producer. No implicit reconnects.
use crate::{AudioPacket, MediaProgress, Session, StreamStats, Volume};
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
    pub state: watch::Receiver<SessionState>,
    pub progress: watch::Receiver<Option<MediaProgress>>,
}
impl SessionHandle {
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
impl SessionPool {
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
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Accept a prepared session without resetting existing targets or media clock.
    pub fn add(&mut self, uid: &str, session: Session) -> Result<SessionHandle> {
        let endpoint = session.endpoint();
        let volume = session.initial_volume();
        let deadline = session.shutdown_deadline();
        self.launch(
            uid,
            endpoint,
            volume,
            deadline,
            move |audio, volume, stop, progress| {
                session.run_with_progress(audio, volume, stop, progress)
            },
        )
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
        let mut entry = self
            .entries
            .remove(uid)
            .ok_or_else(|| anyhow::anyhow!("unknown receiver session"))?;
        entry.stop.send_replace(true);
        let mut task = entry.task.take().unwrap();
        match tokio::time::timeout(entry.shutdown_deadline, &mut task).await {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MediaClock, PACKET_BYTES};
    use tokio::sync::mpsc;
    fn add(
        pool: &mut SessionPool,
        id: &str,
        port: u16,
        events: mpsc::UnboundedSender<(String, u64)>,
    ) -> SessionHandle {
        let uid = id.to_owned();
        pool.launch(id,([127,0,0,1],port).into(),Volume::db(-30.0).unwrap(),Duration::from_secs(1),move|mut audio,_volume,mut stop,_progress|async move{
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
    async fn add_remove_and_failure_do_not_restart_surviving_receivers() {
        let (audio, packets) = broadcast::channel(16);
        let mut pool = SessionPool::new(packets);
        let (events, mut received) = mpsc::unbounded_channel();
        let first = add(&mut pool, "a", 7001, events.clone());
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
}
