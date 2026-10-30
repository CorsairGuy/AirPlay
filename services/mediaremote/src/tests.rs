use std::sync::Mutex;

use mrp::frame;
use mrp::schema::{field as f, kind, value};
use mrp::wire::Node;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use super::*;

#[derive(Default)]
struct Log(Mutex<Vec<String>>);

impl Control for Log {
    fn command(&self, command: Command) -> Reply<'_> {
        self.0.lock().unwrap().push(format!("{command:?}"));
        Box::pin(async { true })
    }

    fn volume(&self, volume: f32) -> Reply<'_> {
        self.0.lock().unwrap().push(format!("volume {volume}"));
        Box::pin(async { true })
    }
}

fn device() -> Device {
    Device {
        name: "Кухня".into(),
        model: "AudioAccessory5,1".into(),
        localized_model: "HomePod mini".into(),
        uid: "12345678-1234-1234-1234-123456789ABC".into(),
        group: "87654321-4321-4321-4321-CBA987654321".into(),
        system_app: "com.example.receiver".into(),
    }
}

fn request(kind: u32, body: &Node, id: &str) -> Node {
    messages::envelope(kind, body, Some(id))
}

/// iPhone по ту сторону канала.
struct Phone {
    io: DuplexStream,
    decoder: Decoder,
    seq: u64,
}

impl Phone {
    async fn send(&mut self, messages: &[Node]) {
        self.seq += 1;
        self.io
            .write_all(&frame::sync(self.seq, messages))
            .await
            .unwrap();
    }

    /// Следующие сообщения (подтверждения пропускаются).
    async fn recv(&mut self) -> Vec<Node> {
        let mut buf = [0u8; 4096];
        loop {
            while let Some(f) = self.decoder.next_frame().unwrap() {
                if let Frame::Sync { messages, .. } = f {
                    return messages;
                }
            }
            let n = self.io.read(&mut buf).await.unwrap();
            assert!(n > 0, "канал закрыт");
            self.decoder.push(&buf[..n]);
        }
    }

    /// Ответ на запрос `id` (рассылки по дороге пропускаются).
    async fn answer(&mut self, id: &str) -> Vec<Node> {
        loop {
            let m = self.recv().await;
            if m.iter()
                .any(|m| m.string(f::ProtocolMessage::identifier) == Some(id))
            {
                return m;
            }
        }
    }
}

fn kinds(messages: &[Node]) -> Vec<u32> {
    messages
        .iter()
        .map(|m| m.number(f::ProtocolMessage::r#type).unwrap() as u32)
        .collect()
}

#[tokio::test]
async fn card_session() {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let (tx, rx) = watch::channel(NowPlaying {
        control_available: true,
        ..Default::default()
    });
    let log = Arc::new(Log::default());
    let server = tokio::spawn(serve(ours, device(), rx, log.clone()));
    let mut phone = Phone {
        io: theirs,
        decoder: Decoder::new(),
        seq: 0,
    };

    // Описание устройства.
    phone
        .send(&[request(kind::DEVICE_INFO_MESSAGE, &Node::default(), "1")])
        .await;
    let m = phone.answer("1").await;
    assert_eq!(kinds(&m), [kind::DEVICE_INFO_MESSAGE]);

    // Подписка: выход, громкость, клиент и карточка, ответ.
    phone
        .send(&[request(
            kind::CLIENT_UPDATES_CONFIG_MESSAGE,
            &Node::default(),
            "2",
        )])
        .await;
    let m = phone.answer("2").await;
    assert_eq!(kinds(&m)[0], kind::UPDATE_OUTPUT_DEVICE_MESSAGE);
    assert!(kinds(&m).contains(&kind::SET_STATE_MESSAGE));

    // Команды.
    let mut next = Node::default();
    next.int(f::SendCommandMessage::command, value::Command::NextTrack);
    phone
        .send(&[request(kind::SEND_COMMAND_MESSAGE, &next, "3")])
        .await;
    let m = phone.answer("3").await;
    let result = m[0]
        .nested(f::ProtocolMessage::sendCommandResultMessage)
        .unwrap();
    assert_eq!(
        result.number(f::SendCommandResultMessage::sendError),
        Some(0)
    );

    let mut seek = Node::default();
    let mut options = Node::default();
    options.double(f::CommandOptions::playbackPosition, 42.5);
    seek.int(
        f::SendCommandMessage::command,
        value::Command::SeekToPlaybackPosition,
    )
    .node(f::SendCommandMessage::options, &options);
    let mut like = Node::default();
    like.int(f::SendCommandMessage::command, value::Command::LikeTrack);
    phone
        .send(&[
            request(kind::SEND_COMMAND_MESSAGE, &seek, "4"),
            request(kind::SEND_COMMAND_MESSAGE, &like, "5"),
        ])
        .await;
    let m = phone.answer("5").await;
    let result = m
        .last()
        .unwrap()
        .nested(f::ProtocolMessage::sendCommandResultMessage)
        .unwrap();
    assert_eq!(
        result.number(f::SendCommandResultMessage::sendError),
        Some(value::SendError::NotSupported)
    );

    // Громкость: ответ сразу, в Control — последнее значение.
    let mut vol = Node::default();
    vol.float(f::SetVolumeMessage::volume, 0.25).text(
        f::SetVolumeMessage::outputDeviceUID,
        device().uid.to_lowercase(),
    );
    let mut bad = Node::default();
    bad.float(f::SetVolumeMessage::volume, 2.0);
    phone
        .send(&[
            request(kind::SET_VOLUME_MESSAGE, &vol, "6"),
            request(kind::SET_VOLUME_MESSAGE, &bad, "7"),
        ])
        .await;
    let m = phone.answer("7").await;
    assert_eq!(
        m.last().unwrap().number(f::ProtocolMessage::errorCode),
        Some(value::ErrorCode::VolumeValueIsOutOfRange)
    );

    // Новый трек — новая карточка.
    tx.send_modify(|now| {
        now.title = "Песня".into();
        now.content_id = "1".into();
        now.playing = true;
    });
    let m = phone.recv().await;
    assert_eq!(kinds(&m), [kind::SET_STATE_MESSAGE]);

    // Неизвестный запрос — ошибка, соединение живо.
    phone
        .send(&[request(kind::GET_STATE_MESSAGE, &Node::default(), "8")])
        .await;
    let m = phone.answer("8").await;
    assert_eq!(
        m[0].number(f::ProtocolMessage::errorCode),
        Some(value::ErrorCode::UnsupportedOperation)
    );

    // Громкость применяется в своей задаче.
    for _ in 0..100 {
        if log.0.lock().unwrap().len() == 3 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        *log.0.lock().unwrap(),
        ["Next", "Seek(42.5)", "volume 0.25"]
    );

    drop(phone);
    server.await.unwrap().unwrap();
}
