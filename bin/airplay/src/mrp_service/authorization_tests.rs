//! Full MRP selection/PIN/worker path. Loopback RTSP and null audio only.
use super::*;
use sender::{
    outputs::{OutputCatalog, OutputRoute, ReceiverEndpoint},
    pool::SessionPool,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

async fn message(
    client: &mut tokio::io::DuplexStream,
    decoder: &mut mrp::frame::Decoder,
    wanted: u64,
) -> mrp::wire::Node {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            while let Some(frame) = decoder.next_frame().unwrap() {
                if let mrp::frame::Frame::Sync { seq, messages } = frame {
                    client.write_all(&mrp::frame::reply(seq)).await.unwrap();
                    for message in messages {
                        if message.number(1) == Some(wanted) {
                            return message;
                        }
                        assert_ne!(message.number(1), Some(59), "unexpected second PIN sheet");
                    }
                }
            }
            let mut bytes = [0; 16384];
            let n = client.read(&mut bytes).await.unwrap();
            assert_ne!(n, 0, "MRP connection closed before expected message");
            decoder.push(&bytes[..n]);
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn mrp_selection_pin_persists_key_then_starts_and_removes_receiver() {
    selection_pin(false).await;
}
#[tokio::test]
async fn controller_disconnect_during_pin_never_selects_or_saves_receiver() {
    selection_pin(true).await;
}
async fn selection_pin(disconnect: bool) {
    let receiver = receiver_context(true);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let receiver_server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        // PIN setup, then persistent pair-verify/media on a new connection.
        for _ in 0..if disconnect { 1 } else { 3 } {
            let (socket, peer) = listener.accept().await.unwrap();
            connections.spawn(crate::serve::serve_connection(
                socket,
                receiver.clone(),
                peer,
            ));
        }
        while let Some(result) = connections.join_next().await {
            result.unwrap().unwrap();
        }
    });
    let directory = std::env::temp_dir().join(format!("airplay-wire-pin-{}", Uuid::new_v4()));
    let store = Arc::new(crate::sender_store::SenderStore::open(&directory).unwrap());
    let mut config =
        sender::SessionConfig::new(address, sender::Identity::new("wire-controller", "test"));
    config.preserve_receiver_volume = true;
    let preparer = crate::session_preparer::StoredSessionPreparer::new(
        config,
        store.clone(),
        crate::session_preparer::AuthenticationPolicy::InteractivePinForUnpaired,
    );
    let catalog = Arc::new(
        OutputCatalog::new(vec![OutputRoute {
            uid: "route".into(),
            name: "Receiver".into(),
            model: "".into(),
            aliases: vec![],
            receivers: vec![ReceiverEndpoint {
                uid: "physical".into(),
                address,
            }],
        }])
        .unwrap(),
    );
    let (control, requests) = crate::output_control::OutputControl::channel(4);
    let (_pcm, packets) = tokio::sync::broadcast::channel(4);
    let (updates, state) = watch::channel(crate::output_worker::Snapshot::default());
    let (stop, stopped) = watch::channel(false);
    let worker = tokio::spawn(crate::output_worker::run(
        SessionPool::new(packets),
        requests,
        catalog,
        vec![],
        preparer,
        updates,
        stopped,
    ));
    let (feed, feed_rx) = watch::channel(Feed::default());
    let (mut client, socket) = tokio::io::duplex(65536);
    let server = tokio::spawn(connection(
        socket,
        Identity {
            device: "local".into(),
            group: "local-group".into(),
        },
        feed_rx,
        Control::new(feed, crate::remote::Remote::new()).with_outputs(control),
    ));
    let selection = model::envelope(
        "MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE",
        Node::default().text(2, "route").clone(),
        Some("select"),
    );
    let selection = mrp::wire::Node::parse(&selection.encode()).unwrap();
    client
        .write_all(&mrp::frame::sync(1, &[selection]))
        .await
        .unwrap();
    let mut decoder = mrp::frame::Decoder::new();
    let prompt = message(&mut client, &mut decoder, 59).await;
    assert!(store.get("physical").unwrap().is_none());
    assert!(state.borrow().selected.is_empty());
    if disconnect {
        drop(client);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), receiver_server)
            .await
            .unwrap()
            .unwrap();
        assert!(state.borrow().selected.is_empty());
        assert!(store.get("physical").unwrap().is_none());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
        return;
    }
    let answer = mrp::messages::envelope(
        mrp::schema::kind::PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE,
        mrp::wire::Node::default().text(1, "4821"),
        prompt.string(2),
    );
    client
        .write_all(&mrp::frame::sync(2, &[answer]))
        .await
        .unwrap();
    let status = message(&mut client, &mut decoder, 61).await;
    assert_eq!(status.nested(65).unwrap().number(2), Some(0));
    let reply = message(&mut client, &mut decoder, 0).await;
    assert_eq!(reply.string(2), Some("select"));
    assert_eq!(reply.number(f("ProtocolMessage", "errorCode")), Some(0));
    assert!(store.get("physical").unwrap().is_some());
    assert_eq!(state.borrow().selected, vec!["route"]);
    let remove = model::envelope(
        "MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE",
        Node::default().text(3, "route").clone(),
        Some("remove"),
    );
    client
        .write_all(&mrp::frame::sync(
            3,
            &[mrp::wire::Node::parse(&remove.encode()).unwrap()],
        ))
        .await
        .unwrap();
    let reply = message(&mut client, &mut decoder, 0).await;
    assert_eq!(reply.string(2), Some("remove"));
    assert_eq!(reply.number(f("ProtocolMessage", "errorCode")), Some(0));
    assert!(state.borrow().selected.is_empty());
    // Reselect: saved credentials must use pair-verify without a new PIN sheet.
    let again = model::envelope(
        "MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE",
        Node::default().text(2, "route").clone(),
        Some("again"),
    );
    client
        .write_all(&mrp::frame::sync(
            4,
            &[mrp::wire::Node::parse(&again.encode()).unwrap()],
        ))
        .await
        .unwrap();
    let reply = message(&mut client, &mut decoder, 0).await;
    assert_eq!(reply.string(2), Some("again"));
    assert_eq!(reply.number(f("ProtocolMessage", "errorCode")), Some(0));
    assert_eq!(state.borrow().selected, vec!["route"]);
    drop(client);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    stop.send_replace(true);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), receiver_server)
        .await
        .unwrap()
        .unwrap();
    drop(store);
    assert!(
        crate::sender_store::SenderStore::open(&directory)
            .unwrap()
            .get("physical")
            .unwrap()
            .is_some()
    );
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn receiver_control_failure_keeps_other_session_and_media_epoch_alive() {
    use sender::pool::SessionState;
    let mut servers = Vec::new();
    let mut sessions = Vec::new();
    for id in ["first", "second"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        servers.push(tokio::spawn(async move {
            let (socket, peer) = listener.accept().await.unwrap();
            crate::serve::serve_connection(socket, receiver_context(false), peer).await
        }));
        let mut config = sender::SessionConfig::new(address, sender::Identity::new(id, "test"));

        config.preserve_receiver_volume = true;
        sessions.push(sender::Session::connect(config).await.unwrap());
    }
    let (audio, packets) = tokio::sync::broadcast::channel(8);
    let mut pool = SessionPool::new(packets);
    let mut handles = Vec::new();
    for (id, session) in ["first", "second"].into_iter().zip(sessions) {
        handles.push(pool.add(id, session).unwrap());
    }
    let origin = std::time::Instant::now();
    let clock = sender::MediaClock::at(origin, 0);
    audio
        .send(Arc::new(
            clock.packet(0, &[7; sender::PACKET_BYTES]).unwrap(),
        ))
        .unwrap();
    for handle in &mut handles {
        tokio::time::timeout(Duration::from_secs(1), async {
            while handle.progress.borrow_and_update().is_none() {
                handle.progress.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
    }
    // Lose only the second encrypted RTSP connection. Its next feedback fails.
    servers[1].abort();
    tokio::time::timeout(Duration::from_secs(4), async {
        while *handles[1].state.borrow_and_update() == SessionState::Running {
            handles[1].state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(*handles[1].state.borrow(), SessionState::Failed);
    assert_eq!(*handles[0].state.borrow(), SessionState::Running);
    assert!(pool.remove("second").await.is_err());
    let index = ((origin.elapsed().as_nanos() * u128::from(sender::SAMPLE_RATE) / 1_000_000_000)
        / sender::FRAMES_PER_PACKET as u128) as u64
        * sender::FRAMES_PER_PACKET as u64;
    audio
        .send(Arc::new(
            clock.packet(index, &[9; sender::PACKET_BYTES]).unwrap(),
        ))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if handles[0]
                .progress
                .borrow_and_update()
                .as_ref()
                .is_some_and(|p| p.packets_sent == 2 && p.frame_index == index)
            {
                break;
            }
            handles[0].progress.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(index > 0);
    let result = pool.shutdown().await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].0, "first");
    assert_eq!(result[0].1.as_ref().unwrap().packets_sent, 2);
    assert_eq!(*handles[0].state.borrow(), SessionState::Ended);
    let failed = servers.pop().unwrap();
    assert!(failed.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3), servers.pop().unwrap())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

fn receiver_context(require_pin: bool) -> Arc<crate::session::Context> {
    let identity = pair::Identity::from_seed("pin-wire-receiver", &[37; 32]);
    let receiver = Arc::new(crate::session::Context {
        device: device::DeviceInfo {
            name: "PIN loopback".into(),
            device_id: "02:00:00:00:00:37".into(),
            model: "AirPlay".into(),
            features: device::Features::AUDIO_RECEIVER,
            status_flags: 4,
            pairing_id: identity.pairing_id.clone(),
            public_key: identity.public_key(),
            source_version: "366.0".into(),
            display: None,
            audio_only_ui: false,
            group: None,
            pgid: "00000000-0000-4000-8000-000000000000".into(),
            enclosure_color: None,
        },
        identity,
        store: Box::new(pair::MemoryStore::default()),
        pin: "4821".into(),
        require_pin,
        mdns: Mutex::new(None),
        port: 0,
        #[cfg(feature = "mirroring")]
        mirror: None,
        remote: Default::default(),
        mrp: None,
        session_group: Default::default(),
        player: Arc::new(crate::media::Player::new(
            audio_stream::Config {
                device: "null".into(),
                ..Default::default()
            },
            timesync::TimeSync::idle(37),
        )),
    });
    receiver
}
