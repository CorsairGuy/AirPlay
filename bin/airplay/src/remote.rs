//! Удалённое управление AirPlay 2.
//!
//! - Команды отправителю ([`Remote`]): play/pause/next/previous, перемотка
//!   и громкость уходят plist-ом `sendMediaRemoteCommand` по каналу событий
//!   звукового соединения. DACP для AirPlay 2 не работает: iOS не
//!   присылает DACP-ID и не публикует `_dacp._tcp`.
//! - Канал MediaRemote (поток 130): шифрованный канал данных с протоколом
//!   MRP внутри ([`bridge`]). Открытый текст идёт в карточку самого
//!   приёмника (`card`) или в отдельную службу по TCP.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};

use crypto::Channel;
use crypto::kdf::ControlKeys;
use plist::{Dictionary, Value};
use rtsp::Request;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::card::Card;
use crate::session::BPLIST;

/// Кто разбирает MRP из потока 130.
#[derive(Clone)]
pub enum MrpService {
    /// Служба по TCP, например airplay_mrp на станции.
    Bridge(SocketAddr),
    /// Карточка самого приёмника.
    Card(Arc<Card>),
}

/// Команда отправителю.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Перемотка на позицию, секунды от начала трека.
    Seek(f64),
    /// Громкость 0..1: iPhone двигает свой ползунок, звук здесь меняется
    /// сразу, не дожидаясь `SET_PARAMETER` в ответ.
    Volume(f32),
}

impl Command {
    /// Тело `POST /command`.
    fn plist(self) -> Dictionary {
        let mut d = Dictionary::new();
        d.insert("type".into(), "sendMediaRemoteCommand".into());
        // Номера MediaRemote: 0 play, 1 pause, 2 toggle, 3 stop, 4 next,
        // 5 previous, 24 перемотка.
        let modern = match self {
            Command::Play => 0,
            Command::Pause => 1,
            Command::PlayPause => 2,
            Command::Stop => 3,
            Command::Next => 4,
            Command::Previous => 5,
            Command::Seek(secs) => {
                let mut params = Dictionary::new();
                params.insert(
                    "kMRMediaRemoteOptionPlaybackPosition".into(),
                    Value::Real(secs.max(0.0)),
                );
                d.insert("params".into(), params.into());
                24
            }
            Command::Volume(v) => {
                let v = f64::from(v.clamp(0.0, 1.0));
                let mut params = Dictionary::new();
                params.insert("volume".into(), Value::Real(v));
                d.insert("value".into(), "dvlc".into());
                d.insert("volume".into(), Value::Real(v));
                d.insert("params".into(), params.into());
                return d;
            }
        };
        d.insert(
            "modernMediaRemoteCommand".into(),
            Value::Integer(modern.into()),
        );
        d
    }

    /// Громкость 0..1 в дБ AirPlay: 0 — тишина (-144), иначе -30..0.
    pub fn volume_db(unit: f32) -> f32 {
        let unit = unit.clamp(0.0, 1.0);
        if unit == 0.0 {
            -144.0
        } else {
            (unit - 1.0) * 30.0
        }
    }
}

/// Ручка для программы, которая встраивает приёмник: команды текущему
/// отправителю. Клонируется; передаётся в `serve::Options::remote`.
#[derive(Clone, Default)]
pub struct Remote {
    active: Arc<Mutex<Option<Active>>>,
}

struct Active {
    owner: u64,
    tx: mpsc::UnboundedSender<Command>,
}

impl Remote {
    pub fn new() -> Self {
        Self::default()
    }

    /// Отправить команду. `false` — никто не играет (канала событий нет).
    pub fn send(&self, command: Command) -> bool {
        let active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        match active.as_ref() {
            Some(a) => a.tx.send(command).is_ok(),
            None => false,
        }
    }

    /// Подключён ли отправитель со звуком.
    pub fn connected(&self) -> bool {
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|a| !a.tx.is_closed())
    }

    /// Канал событий соединения `owner` открыт: команды теперь идут туда.
    pub(crate) fn attach(&self, owner: u64) -> mpsc::UnboundedReceiver<Command> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.active.lock().unwrap_or_else(|e| e.into_inner()) = Some(Active { owner, tx });
        rx
    }

    pub(crate) fn detach(&self, owner: u64) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if active.as_ref().is_some_and(|a| a.owner == owner) {
            *active = None;
        }
    }
}

/// Запрос `POST /command` в канал событий.
pub(crate) fn command_request(body: Dictionary, cseq: u32) -> Vec<u8> {
    let mut plist = Vec::new();
    Value::Dictionary(body)
        .to_writer_binary(&mut plist)
        .expect("запись plist в память не может завершиться ошибкой");
    let mut req = Request::new("POST", "/command", "RTSP/1.0");
    req.headers.insert("CSeq", cseq.to_string());
    req.headers.insert("Content-Type", BPLIST);
    req.body = plist;
    let mut out = Vec::new();
    req.encode(&mut out);
    out
}

/// Тело команды для канала событий.
pub(crate) fn command_body(command: Command) -> Dictionary {
    command.plist()
}

/// Канал данных потока 130: принимает одно подключение с адреса
/// отправителя, расшифровывает и отдаёт в `service`, ответы — обратно.
pub async fn bridge(listener: TcpListener, sender: IpAddr, keys: ControlKeys, service: MrpService) {
    let (client, peer) = match listener.accept().await {
        Ok(c) => c,
        Err(e) => {
            warn!("MRP: {e}");
            return;
        }
    };
    if peer.ip().to_canonical() != sender {
        warn!(%peer, "MRP: подключение не от отправителя");
        return;
    }
    let _ = client.set_nodelay(true);
    let channel = Channel::new(keys);
    let result = match service {
        MrpService::Bridge(backend) => {
            let conn = match TcpStream::connect(backend).await {
                Ok(b) => b,
                Err(e) => {
                    warn!(%backend, "MRP: служба недоступна: {e}");
                    return;
                }
            };
            info!(%backend, "канал MRP подключён");
            let _ = conn.set_nodelay(true);
            forward(client, conn, channel).await
        }
        MrpService::Card(card) => {
            info!("канал MRP: карточка приёмника");
            let (ours, theirs) = tokio::io::duplex(64 * 1024);
            let serving = tokio::spawn(card.serve(theirs));
            let result = forward(client, ours, channel).await;
            serving.abort();
            result
        }
    };
    match result {
        Ok(()) => info!("канал MRP закрыт"),
        Err(e) => info!("канал MRP закрыт: {e}"),
    }
}

async fn forward<B: AsyncRead + AsyncWrite + Unpin>(
    mut client: TcpStream,
    mut backend: B,
    mut channel: Channel,
) -> std::io::Result<()> {
    let mut from_client = vec![0u8; 16 * 1024];
    let mut from_backend = vec![0u8; 16 * 1024];
    let mut sealed = Vec::new();
    let mut plain = Vec::new();
    loop {
        tokio::select! {
            n = client.read(&mut from_client) => {
                let n = n?;
                if n == 0 {
                    return Ok(());
                }
                sealed.extend_from_slice(&from_client[..n]);
                plain.clear();
                channel
                    .open(&mut sealed, &mut plain)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
                backend.write_all(&plain).await?;
            }
            n = backend.read(&mut from_backend) => {
                let n = n?;
                if n == 0 {
                    return Ok(());
                }
                let mut out = Vec::with_capacity(n + 64);
                channel.seal(&from_backend[..n], &mut out);
                client.write_all(&out).await?;
            }
        }
        debug!("MRP: пересылка");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_plists() {
        let next = Command::Next.plist();
        assert_eq!(next["type"].as_string(), Some("sendMediaRemoteCommand"));
        assert_eq!(
            next["modernMediaRemoteCommand"].as_unsigned_integer(),
            Some(4)
        );

        let seek = Command::Seek(12.5).plist();
        assert_eq!(
            seek["modernMediaRemoteCommand"].as_unsigned_integer(),
            Some(24)
        );
        let params = seek["params"].as_dictionary().unwrap();
        assert_eq!(
            params["kMRMediaRemoteOptionPlaybackPosition"].as_real(),
            Some(12.5)
        );

        let vol = Command::Volume(0.5).plist();
        assert_eq!(vol["value"].as_string(), Some("dvlc"));
        assert_eq!(vol["volume"].as_real(), Some(0.5));
        assert!(!vol.contains_key("modernMediaRemoteCommand"));

        assert_eq!(Command::volume_db(0.0), -144.0);
        assert_eq!(Command::volume_db(1.0), 0.0);
        assert_eq!(Command::volume_db(0.5), -15.0);
    }

    #[test]
    fn remote_without_sender() {
        let r = Remote::new();
        assert!(!r.send(Command::Play));
        assert!(!r.connected());
        let mut rx = r.attach(1);
        assert!(r.connected());
        assert!(r.send(Command::Pause));
        assert_eq!(rx.try_recv().unwrap(), Command::Pause);
        r.detach(2);
        assert!(r.connected(), "чужое соединение не отключает");
        r.detach(1);
        assert!(!r.send(Command::Play));
    }
}
