//! Plaintext Card transport behind an already authenticated local receiver bridge.
//! No AirPlay listener, pairing protocol or implicit network bind is created.
use crate::card::Card;
use anyhow::{Result, ensure};
use std::sync::Arc;
use tokio::{net::TcpListener, sync::watch, task::JoinSet};

pub async fn serve(
    listener: TcpListener,
    card: Arc<Card>,
    limit: usize,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    ensure!(
        listener.local_addr()?.ip().is_loopback() && (1..=64).contains(&limit),
        "Card bridge requires loopback and 1..64 connections"
    );
    let mut clients = JoinSet::new();
    let (client_stop, _) = watch::channel(false);
    let result = loop {
        if *stop.borrow() || stop.has_changed().is_err() {
            break Ok(());
        }
        tokio::select! {
            biased;
            changed = stop.changed() => { if changed.is_err() || *stop.borrow() { break Ok(()); } }
            _ = clients.join_next(), if !clients.is_empty() => {}
            accepted = listener.accept(), if clients.len() < limit => {
                let (stream, peer) = match accepted { Ok(value) => value, Err(error) => break Err(error.into()) };
                if !peer.ip().is_loopback() { continue; }
                if let Err(error) = stream.set_nodelay(true) { break Err(error.into()); }
                let card = card.clone();
                let mut closed = client_stop.subscribe();
                clients.spawn(async move {
                    if *closed.borrow() || closed.has_changed().is_err() { return; }
                    tokio::select! {
                        _ = card.serve(stream) => {}
                        _ = async {
                            loop {
                                if *closed.borrow() { return; }
                                if closed.changed().await.is_err() { return; }
                            }
                        } => {}
                    }
                });
            }
        }
    };
    // Every client has the same stop channel; drop listener before awaiting.
    drop(listener);
    client_stop.send_replace(true);
    while clients.join_next().await.is_some() {}
    result
}
