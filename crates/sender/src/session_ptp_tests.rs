use super::*;
use crate::{
    MediaClock, PACKET_BYTES,
    group::{ClockDomain, GroupContext},
    timing::PtpPlan,
};
use pair::{
    PairingStore,
    hap::{PairVerify, VerifyOutcome},
};

#[tokio::test]
async fn ptp_negotiates_setpeers_sends_d7_and_rejects_ntp_downgrade() {
    const CLOCK: u64 = 0xf842885f71750008;
    for mode in 0..4 {
        let accept = mode == 0 || mode == 3;
        let controller = PairIdentity::from_seed("controller", &[7; 32]);
        let accessory = PairIdentity::from_seed("receiver", &[8; 32]);
        let credentials = PairingCredentials::new(
            controller.clone(),
            b"receiver".to_vec(),
            accessory.public_key(),
        )
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let domain = clock::domain::Domain::new(CLOCK, 1).unwrap();
        let plan = PtpPlan::new(
            GroupContext::new(123, ClockDomain::Ptp(CLOCK), true).unwrap(),
            domain,
            456,
            vec!["127.0.0.1".parse().unwrap()],
            vec!["127.0.0.1".parse().unwrap()],
        )
        .unwrap();
        let data = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let control = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let data_port = data.local_addr().unwrap().port();
        let control_port = control.local_addr().unwrap().port();
        let peer = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut conn = Connection::new(socket);
            let store = pair::MemoryStore::default();
            store.add(controller.pairing_id.as_bytes(), controller.public_key());
            let mut verifier = PairVerify::new(accessory);
            for _ in 0..2 {
                let request = conn.event_request().await.unwrap();
                assert_eq!(request.uri, "/pair-verify");
                let (body, keys) = match verifier.handle(&request.body, &store).unwrap() {
                    VerifyOutcome::Continue(body) => (body, None),
                    VerifyOutcome::Verified { reply, keys, .. } => (reply, Some(keys)),
                    _ => panic!("pair verify failed"),
                };
                let mut response = rtsp::Response::to(&request, 200);
                response.body = body;
                let mut bytes = Vec::new();
                response.encode(&mut bytes);
                conn.send(&bytes).await.unwrap();
                if let Some(keys) = keys {
                    conn.encrypt(keys);
                }
            }
            let mut stage = 0;
            loop {
                let request = timeout(Duration::from_secs(3), conn.event_request())
                    .await
                    .unwrap()
                    .unwrap();
                let mut response = rtsp::Response::to(&request, 200);
                if request.method == "TEARDOWN" {
                    let mut bytes = Vec::new();
                    response.encode(&mut bytes);
                    conn.send(&bytes).await.unwrap();
                    break;
                }
                match stage {
                    0 => {
                        assert_eq!(request.method, "SETUP");
                        let value =
                            plist::Value::from_reader(std::io::Cursor::new(&request.body)).unwrap();
                        let setup = value.as_dictionary().unwrap();
                        assert_eq!(
                            setup.get("timingProtocol").unwrap().as_string(),
                            Some("PTP")
                        );
                        assert!(setup.get("timingPort").is_none());
                        assert_eq!(
                            setup
                                .get("timingPeerInfo")
                                .unwrap()
                                .as_dictionary()
                                .unwrap()
                                .get("ClockID")
                                .unwrap()
                                .as_signed_integer()
                                .unwrap() as u64,
                            CLOCK
                        );
                        let fields = match mode {
                            0 => vec![("timingProtocol", plist::Value::String("PTP".into()))],
                            1 => vec![("timingProtocol", plist::Value::String("NTP".into()))],
                            2 => vec![("timingProtocol", plist::Value::Integer(1_u64.into()))],
                            _ => vec![],
                        };
                        response.body = binary_plist(dictionary(fields)).unwrap();
                    }
                    1 => {
                        assert!(accept);
                        assert_eq!(request.method, "SETPEERS");
                        assert_eq!(
                            request.headers.get("Content-Type"),
                            Some("/peer-list-changed")
                        );
                        let peers =
                            plist::Value::from_reader(std::io::Cursor::new(&request.body)).unwrap();
                        assert_eq!(peers.as_array().unwrap().len(), 1);
                    }
                    2 => {
                        assert_eq!(request.method, "SETUP");
                        response.body = binary_plist(dictionary(vec![(
                            "streams",
                            plist::Value::Array(vec![dictionary(vec![
                                ("dataPort", plist::Value::Integer((data_port as u64).into())),
                                (
                                    "controlPort",
                                    plist::Value::Integer((control_port as u64).into()),
                                ),
                                ("latencyMin", plist::Value::Integer(11_025_u64.into())),
                            ])]),
                        )]))
                        .unwrap();
                    }
                    3 => assert_eq!(request.method, "RECORD"),
                    4 => assert_eq!(request.method, "FLUSH"),
                    _ => panic!("unexpected request: {}", request.method),
                }
                stage += 1;
                let mut bytes = Vec::new();
                response.encode(&mut bytes);
                conn.send(&bytes).await.unwrap();
            }
            assert_eq!(stage, if accept { 5 } else { 1 });
            if accept {
                let mut bytes = [0; 2048];
                let (size, _) = control.recv_from(&mut bytes).await.unwrap();
                assert_eq!(size, 28);
                assert_eq!(bytes[1], 0xd7);
                assert_eq!(u64::from_be_bytes(bytes[20..28].try_into().unwrap()), CLOCK);
                assert_eq!(
                    u64::from_be_bytes(bytes[8..16].try_into().unwrap()),
                    7_000_000_000
                );
                let (size, _) = data.recv_from(&mut bytes).await.unwrap();
                assert!(size > PACKET_BYTES);
                assert_eq!(bytes[1] & 0x7f, 96);
            }
        });
        let mut config = SessionConfig::new(endpoint, Identity::new("sender", "test"));
        config.pairing = Some(credentials);
        config.ptp = Some(plan);
        config.preserve_receiver_volume = true;
        config.latency_frames = 22_050;
        let session = Session::connect(config).await;
        if accept {
            let session = session.unwrap();
            assert!(session._timing_task.is_none());
            let (audio, packets) = broadcast::channel(4);
            let (_gain, gains) = watch::channel(Volume::MUTE);
            let (_stop, stopped) = watch::channel(false);
            let frame = MediaClock::now()
                .packet_with_ptp(0, &[0; PACKET_BYTES], domain, |_| {
                    Ok(domain.reading(7_000_000_000))
                })
                .unwrap();
            audio.send(Arc::new(frame)).unwrap();
            drop(audio);
            let stats = timeout(Duration::from_secs(3), session.run(packets, gains, stopped))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stats.packets_sent, 1);
        } else {
            assert!(session.err().unwrap().to_string().contains("PTP timing"));
        }
        timeout(Duration::from_secs(3), peer)
            .await
            .unwrap()
            .unwrap();
    }
}
