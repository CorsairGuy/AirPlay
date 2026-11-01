//! Same backend/data over TCP bridge versus in-process duplex; localhost only.
use std::{sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener,TcpStream}};
use crate::{card::Card, remote::{self, MrpService, Remote}};
use crypto::{Channel, kdf::ControlKeys};
use mrp::{frame::{Decoder, Frame}, messages::Device, wire::Node};

async fn probe(service: MrpService, fragmented: bool) -> Node {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr=listener.local_addr().unwrap();
    let server=tokio::spawn(remote::bridge(listener,addr.ip(),ControlKeys::accessory_data(&[42;32],7),service));
    let mut stream=TcpStream::connect(addr).await.unwrap();
    let mut channel=Channel::new(ControlKeys::controller_data(&[42;32],7));
    let mut request=Node::default();
    request.int(1,15).text(2,"same-request").node(20,&Node::default());
    let mut wire=Vec::new();
    channel.seal(&mrp::frame::sync(9,&[request]),&mut wire);
    if fragmented {
        for part in wire.chunks(3) { stream.write_all(part).await.unwrap(); }
    } else { stream.write_all(&wire).await.unwrap(); }
    let result=tokio::time::timeout(Duration::from_secs(3),async {
        let mut encrypted=Vec::new(); let mut frames=Decoder::new(); let mut ack=false;
        loop {
            let mut chunk=[0;4096]; let n=stream.read(&mut chunk).await.unwrap(); assert!(n>0);
            encrypted.extend_from_slice(&chunk[..n]); let mut plain=Vec::new();
            while encrypted.len() >= 2 {
                let size = u16::from_le_bytes(encrypted[..2].try_into().unwrap()) as usize;
                let total = size + 18;
                if encrypted.len() < total { break; }
                let mut record: Vec<u8> = encrypted.drain(..total).collect();
                let mut decoded = Vec::new();
                channel.open(&mut record, &mut decoded).unwrap();
                // These bootstrap frames are below the encryption record limit.
                // An ACK and DeviceInfo must not be coalesced into one record.
                assert!(decoded.len() >= 32);
                assert_eq!(u32::from_be_bytes(decoded[..4].try_into().unwrap()) as usize, decoded.len());
                plain.extend_from_slice(&decoded);
            }
            frames.push(&plain);
            while let Some(frame)=frames.next_frame().unwrap() {
                match frame {
                    Frame::Reply {seq:9} => ack=true,
                    Frame::Sync {mut messages,..} if !messages.is_empty() => {
                        assert!(ack,"request ACK precedes the response");
                        let mut response=messages.remove(0);
                        assert_eq!(response.number(1),Some(15));
                        assert_eq!(response.string(2),Some("same-request"));
                        let unique=mrp::schema::field::ProtocolMessage::uniqueIdentifier;
                        response.0.retain(|(tag,_)|*tag!=unique);
                        return response;
                    },
                    _=>{}
                }
            }
        }
    }).await.expect("MRP bootstrap deadline");
    drop(stream); tokio::time::timeout(Duration::from_secs(1),server).await.unwrap().unwrap();
    result
}

#[tokio::test]
async fn same_card_device_info_survives_bridge_and_duplex_paths() {
    let card=Card::new(Device {
        name:"Isolated MRP test".into(),model:"AudioAccessory5,1".into(),
        localized_model:"HomePod mini".into(),uid:"test-device".into(),
        group:"test-group".into(),system_app:"mini2.native".into(),
    },Remote::new(),Arc::new(crate::events::NoEvents));
    for fragmented in [false,true] {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr=listener.local_addr().unwrap(); let backend=card.clone();
        let serving=tokio::spawn(async move {
            let (socket,_)=listener.accept().await.unwrap(); backend.serve(socket).await.unwrap();
        });
        let tcp=probe(MrpService::Bridge(addr),fragmented).await;
        let direct=probe(MrpService::Card(card.clone()),fragmented).await;
        assert_eq!(tcp,direct,"same identity/feed differ by backend transport");
        tokio::time::timeout(Duration::from_secs(1),serving).await.unwrap().unwrap();
    }
}
