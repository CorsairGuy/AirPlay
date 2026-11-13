//! Explicit serialized controller for the patched NQPTP sender interface.
//! Caller must hold exclusive command authority and independently attest the
//! daemon/role on every operation. This module never starts or stops a daemon.
use crate::ptp_group::{ExternalPeerControl, PeerControlUpdate};
use anyhow::{Result, bail, ensure};
use clock::{domain::Domain, external::Selection};
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use timesync::{
    peer_interests::peer_union,
    sender::{Peer, Ports},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot, watch},
    time::{MissedTickBehavior, interval, timeout},
};

struct Update {
    domain: Domain,
    owner: u128,
    peers: Vec<IpAddr>,
    reply: oneshot::Sender<Result<()>>,
}
#[derive(Clone)]
pub struct NqptpControl {
    updates: mpsc::Sender<Update>,
    loss: watch::Receiver<bool>,
}
impl NqptpControl {
    /// Connect this to the host clock/relay stop path.
    pub fn loss(&self) -> watch::Receiver<bool> {
        self.loss.clone()
    }
}
impl ExternalPeerControl for NqptpControl {
    fn replace(&self, domain: Domain, owner: u128, peers: Vec<IpAddr>) -> PeerControlUpdate<'_> {
        Box::pin(async move {
            let (reply, result) = oneshot::channel();
            self.updates
                .send(Update {
                    domain,
                    owner,
                    peers,
                    reply,
                })
                .await?;
            result.await?
        })
    }
}
/// The worker owns the socket and processes queued updates even when the
/// requesting future is cancelled. Later withdrawals follow in queue order.
pub struct NqptpWorker {
    socket: UdpSocket,
    destination: SocketAddr,
    smi: String,
    selection: Selection,
    updates: mpsc::Receiver<Update>,
    loss: watch::Sender<bool>,
}
impl Drop for NqptpWorker {
    fn drop(&mut self) {
        self.loss.send_replace(true);
    }
}
impl NqptpWorker {
    pub fn new(
        socket: UdpSocket,
        destination: SocketAddr,
        smi: String,
        selection: Selection,
    ) -> Result<(NqptpControl, Self)> {
        let bind = socket.local_addr()?;
        ensure!(
            bind.ip().is_loopback()
                && destination.ip().is_loopback()
                && bind.is_ipv4() == destination.is_ipv4()
                && destination.port() != 0,
            "NQPTP control must use explicit matching loopback addresses"
        );
        ensure!(
            smi.starts_with('/')
                && smi.len() <= 128
                && smi
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/-_".contains(&b)),
            "invalid NQPTP interface"
        );
        ensure!(
            selection.sender_master_verified,
            "unverified sender-master selection"
        );
        let (updates, receiver) = mpsc::channel(8);
        let (loss, lost) = watch::channel(false);
        Ok((
            NqptpControl {
                updates,
                loss: lost,
            },
            Self {
                socket,
                destination,
                smi,
                selection,
                updates: receiver,
                loss,
            },
        ))
    }
    fn attest(&self, evidence: &mut impl FnMut() -> Result<Selection>) -> Result<()> {
        ensure!(
            evidence()? == self.selection,
            "NQPTP daemon/role/epoch changed"
        );
        Ok(())
    }
    async fn send(
        &self,
        peers: &BTreeMap<IpAddr, Ports>,
        evidence: &mut impl FnMut() -> Result<Selection>,
    ) -> Result<()> {
        self.attest(evidence)?;
        let mut command = format!("{} M", self.smi);
        for ip in peers.keys() {
            command.push(' ');
            command.push_str(&ip.to_string());
        }
        command.push('\0');
        let sent = timeout(
            Duration::from_millis(200),
            self.socket.send_to(command.as_bytes(), self.destination),
        )
        .await??;
        ensure!(sent == command.len(), "short NQPTP command");
        self.attest(evidence)?;
        Ok(())
    }
    /// Evidence must be cached/bounded and local. Cancellation cannot interrupt
    /// a blocking callback. A successful UDP send is not daemon acknowledgement.
    /// On identity loss, no command is sent to a replacement daemon. Abrupt task
    /// cancellation relies on the patched daemon's existing keepalive expiry.
    pub async fn run(
        mut self,
        mut evidence: impl FnMut() -> Result<Selection>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<()> {
        let mut owners = BTreeMap::new();
        let mut union = BTreeMap::new();
        let mut ticks = interval(Duration::from_secs(4));
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ticks.tick().await;
        loop {
            if *stop.borrow() || stop.has_changed().is_err() {
                if !owners.is_empty() {
                    self.send(&BTreeMap::new(), &mut evidence).await?;
                }
                return Ok(());
            }
            tokio::select! {
                biased;
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        if !owners.is_empty() { self.send(&BTreeMap::new(), &mut evidence).await?; }
                        return Ok(());
                    }
                }
                request = self.updates.recv() => {
                    let Some(request) = request else {
                        if !owners.is_empty() { self.send(&BTreeMap::new(), &mut evidence).await?; }
                        return Ok(());
                    };
                    if request.owner == 0 || request.domain != self.selection.domain {
                        let _ = request.reply.send(Err(anyhow::anyhow!("wrong peer owner/domain")));
                        continue;
                    }
                    let peers = request.peers.into_iter().map(|ip| Peer { ip, ports: Ports { event: 319, general: 320 } }).collect();
                    let planned = peer_union(self.destination.ip(), &owners, request.owner, peers);
                    let (next_owners, next) = match planned {
                        Ok(value) => value,
                        Err(error) => { let _ = request.reply.send(Err(error.into())); continue; }
                    };
                    if let Err(error) = self.send(&next, &mut evidence).await {
                        let _ = request.reply.send(Err(anyhow::anyhow!("NQPTP control failed: {error}")));
                        bail!("NQPTP control failed: {error}");
                    }
                    owners = next_owners; union = next;
                    let _ = request.reply.send(Ok(()));
                }
                _ = ticks.tick(), if !owners.is_empty() => { self.send(&union, &mut evidence).await?; }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use clock::external::StartMarker;
    fn selection() -> Selection {
        Selection {
            daemon_instance: 1,
            domain: Domain::new(9, 1).unwrap(),
            master_start_raw_ns: 100,
            start_marker: StartMarker::RefreshedPublication,
            clock_change_offset: None,
            sender_master_verified: true,
        }
    }
    #[tokio::test]
    async fn shared_peer_withdrawal_preserves_other_owner_and_stop_clears() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (control, worker) = NqptpWorker::new(
            socket,
            server.local_addr().unwrap(),
            "/test".into(),
            selection(),
        )
        .unwrap();
        let (stop, receiver) = watch::channel(false);
        let task = tokio::spawn(worker.run(|| Ok(selection()), receiver));
        let ip = "127.0.0.2".parse().unwrap();
        let mut packet = [0; 256];
        for (owner, peers, expected) in [
            (1, vec![ip], "/test M 127.0.0.2\0"),
            (2, vec![ip], "/test M 127.0.0.2\0"),
            (1, vec![], "/test M 127.0.0.2\0"),
        ] {
            control
                .replace(selection().domain, owner, peers)
                .await
                .unwrap();
            let n = timeout(Duration::from_secs(1), server.recv(&mut packet))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&packet[..n], expected.as_bytes());
        }
        assert!(
            control
                .replace(Domain::new(9, 2).unwrap(), 2, vec![])
                .await
                .is_err()
        );
        stop.send(true).unwrap();
        task.await.unwrap().unwrap();
        assert!(*control.loss().borrow());
        let n = timeout(Duration::from_secs(1), server.recv(&mut packet))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&packet[..n], b"/test M\0");
    }
    #[tokio::test]
    async fn changed_daemon_rejects_command() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (control, worker) = NqptpWorker::new(
            socket,
            server.local_addr().unwrap(),
            "/test".into(),
            selection(),
        )
        .unwrap();
        let (_stop, receiver) = watch::channel(false);
        let task = tokio::spawn(worker.run(
            || {
                let mut s = selection();
                s.daemon_instance += 1;
                Ok(s)
            },
            receiver,
        ));
        assert!(
            control
                .replace(selection().domain, 1, vec!["127.0.0.2".parse().unwrap()])
                .await
                .is_err()
        );
        assert!(task.await.unwrap().is_err());
        let mut packet = [0; 256];
        assert!(server.try_recv(&mut packet).is_err());
    }
}
