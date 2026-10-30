//! Служба MediaRemote (аналог mediaremoted): карточка «Сейчас играет»,
//! пульт и громкость для iPhone.
//!
//! [`serve`] ведёт одно соединение MRP поверх расшифрованного канала
//! потока 130: отвечает на запросы iPhone и рассылает изменения
//! [`NowPlaying`]. Откуда берётся трек и куда уходят команды, решает
//! программа: она реализует [`Control`] и обновляет `watch` с карточкой.
//! Приёмник `airplay` делает это сам (метаданные и пульт отправителя), на
//! станции — airplay_mrp из своих источников.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use mrp::frame::{self, Decoder, Frame};
use mrp::messages::{self, Device, NowPlaying, same_uid};
use mrp::schema::{field as f, kind, value};
use mrp::wire::Node;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

pub use mrp::messages::{Artwork, Route};

/// Команда из карточки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Позиция в секундах от начала трека.
    Seek(f64),
}

pub type Reply<'a> = Pin<Box<dyn Future<Output = bool> + Send + 'a>>;

/// Куда уходят команды карточки. `true` — источник принял.
pub trait Control: Send + Sync {
    fn command(&self, command: Command) -> Reply<'_>;
    /// Громкость 0..1. Ползунок шлёт пачку значений: служба отвечает iPhone
    /// сразу и передаёт сюда только последнее, по одному вызову за раз.
    fn volume(&self, volume: f32) -> Reply<'_>;
}

/// Не дольше этого пишем один кадр: iPhone, который не читает, — пропал.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Сдвиг громкости меньше этого не рассылается.
const VOLUME_STEP: f32 = 0.004;

/// Одно соединение MRP до его закрытия.
pub async fn serve<S>(
    stream: S,
    device: Device,
    mut updates: watch::Receiver<NowPlaying>,
    control: Arc<dyn Control>,
) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let (mut read, write) = tokio::io::split(stream);
    let (tx, rx) = mpsc::channel(64);
    let writer = tokio::spawn(writer(write, rx));
    let volume = volume_worker(control.clone());

    let mut session = Session {
        device,
        control,
        volume,
        subscribed: false,
    };
    let mut decoder = Decoder::new();
    let mut chunk = vec![0u8; 16 * 1024];
    let mut previous = updates.borrow().clone();
    info!("MRP: соединение");
    let result = async {
        loop {
            tokio::select! {
                n = read.read(&mut chunk) => {
                    let n = n?;
                    if n == 0 {
                        return Ok(());
                    }
                    decoder.push(&chunk[..n]);
                    loop {
                        let frame = match decoder.next_frame() {
                            Ok(Some(f)) => f,
                            Ok(None) => break,
                            Err(mrp::Error::FrameSize) => {
                                return Err(invalid("MRP: неверная длина кадра"));
                            }
                            Err(e) => {
                                warn!("MRP: кадр пропущен: {e}");
                                continue;
                            }
                        };
                        let Frame::Sync { seq, messages } = frame else {
                            continue;
                        };
                        send(&tx, Out::Ack(seq)).await?;
                        for message in messages {
                            let now = updates.borrow().clone();
                            let out = session.handle(&message, &now).await;
                            if !out.is_empty() {
                                send(&tx, Out::Messages(out)).await?;
                            }
                        }
                    }
                }
                changed = updates.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                    let now = updates.borrow_and_update().clone();
                    if session.subscribed {
                        let out = session.changes(&previous, &now);
                        if !out.is_empty() {
                            send(&tx, Out::Messages(out)).await?;
                        }
                    }
                    previous = now;
                }
            }
        }
    }
    .await;
    drop(tx);
    let _ = writer.await;
    match &result {
        Ok(()) => info!("MRP: соединение закрыто"),
        Err(e) => info!("MRP: соединение закрыто: {e}"),
    }
    result
}

fn invalid(msg: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

enum Out {
    Ack(u64),
    Messages(Vec<Node>),
}

async fn send(tx: &mpsc::Sender<Out>, out: Out) -> std::io::Result<()> {
    tx.send(out)
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "MRP: запись закрыта"))
}

async fn writer<W: AsyncWrite + Unpin>(mut write: W, mut rx: mpsc::Receiver<Out>) {
    let mut seq: u64 = rand::random();
    while let Some(out) = rx.recv().await {
        let bytes = match out {
            Out::Ack(n) => frame::reply(n),
            Out::Messages(m) => {
                seq = seq.wrapping_add(1);
                frame::sync(seq, &m)
            }
        };
        match tokio::time::timeout(WRITE_TIMEOUT, write.write_all(&bytes)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                debug!("MRP: запись: {e}");
                break;
            }
            Err(_) => {
                warn!("MRP: iPhone не читает канал");
                break;
            }
        }
    }
    let _ = write.shutdown().await;
}

/// Громкость с ползунка: в `control` уходит последнее значение.
fn volume_worker(control: Arc<dyn Control>) -> watch::Sender<Option<f32>> {
    let (tx, mut rx) = watch::channel(None::<f32>);
    tokio::spawn(async move {
        while rx.changed().await.is_ok() {
            let value = *rx.borrow_and_update();
            if let Some(v) = value
                && !control.volume(v).await
            {
                debug!(volume = v, "MRP: громкость не применена");
            }
        }
    });
    tx
}

struct Session {
    device: Device,
    control: Arc<dyn Control>,
    volume: watch::Sender<Option<f32>>,
    /// iPhone прислал CLIENT_UPDATES_CONFIG: ему нужны изменения.
    subscribed: bool,
}

impl Session {
    async fn handle(&mut self, message: &Node, now: &NowPlaying) -> Vec<Node> {
        use f::ProtocolMessage as p;
        let typ = message.number(p::r#type).unwrap_or(0) as u32;
        let id = message.string(p::identifier);
        let body = match mrp::schema::extension(typ).map(|tag| message.nested(tag)) {
            Some(Ok(b)) => b,
            None => Node::default(),
            Some(Err(e)) => {
                warn!(typ, "MRP: тело сообщения: {e}");
                return reply_if(id, value::ErrorCode::UnknownError);
            }
        };
        debug!(typ, request = id.is_some(), "MRP");
        let d = &self.device;
        match typ {
            kind::DEVICE_INFO_MESSAGE => vec![messages::device_info(d, now, id)],
            kind::CLIENT_UPDATES_CONFIG_MESSAGE => {
                self.subscribed = true;
                let mut out = messages::topology(d, now);
                out.extend(messages::announcement(d, now));
                out.extend(id.map(|id| messages::error_reply(Some(id), 0)));
                out
            }
            kind::PLAYBACK_QUEUE_REQUEST_MESSAGE => {
                vec![messages::state(d, now, Some(&body), id)]
            }
            kind::GET_VOLUME_MESSAGE => {
                let mut b = Node::default();
                b.float(f::GetVolumeResultMessage::volume, now.volume);
                vec![messages::envelope(kind::GET_VOLUME_RESULT_MESSAGE, &b, id)]
            }
            kind::SET_VOLUME_MESSAGE => self.set_volume(&body, now, id),
            kind::SEND_COMMAND_MESSAGE => {
                let error = self.command(&body).await;
                vec![messages::command_reply(id, error)]
            }
            kind::GENERIC_MESSAGE => {
                vec![messages::envelope(
                    kind::GENERIC_MESSAGE,
                    &Node::default(),
                    id,
                )]
            }
            kind::MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE => {
                vec![messages::error_reply(id, self.modify_output_context(&body))]
            }
            kind::DEVICE_INFO_UPDATE_MESSAGE | kind::SET_CONNECTION_STATE_MESSAGE => Vec::new(),
            _ => reply_if(id, value::ErrorCode::UnsupportedOperation),
        }
    }

    fn set_volume(&self, body: &Node, now: &NowPlaying, id: Option<&str>) -> Vec<Node> {
        use f::SetVolumeMessage as m;
        if let Some(uid) = body.string(m::outputDeviceUID) {
            let d = &self.device;
            if !same_uid(uid, &d.uid) && !same_uid(uid, &d.group) && !same_uid(uid, now.group(d)) {
                return vec![messages::error_reply(
                    id,
                    value::ErrorCode::CouldNotFindTheGivenOutputdevice,
                )];
            }
        }
        let v = body.real(m::volume).unwrap_or(f64::NAN) as f32;
        if !(0.0..=1.0).contains(&v) {
            return vec![messages::error_reply(
                id,
                value::ErrorCode::VolumeValueIsOutOfRange,
            )];
        }
        self.volume.send_replace(Some(v));
        let mut now = now.clone();
        now.volume = v;
        vec![
            messages::error_reply(id, 0),
            messages::volume(&self.device, &now),
        ]
    }

    /// `value::SendError` для ответа.
    async fn command(&self, body: &Node) -> u64 {
        use value::Command as c;
        use value::SendError as e;
        let options = body
            .nested(f::SendCommandMessage::options)
            .unwrap_or_default();
        let command = match body.number(f::SendCommandMessage::command).unwrap_or(0) {
            c::Play => Command::Play,
            c::Pause => Command::Pause,
            c::TogglePlayPause => Command::PlayPause,
            c::Stop => Command::Stop,
            c::NextTrack => Command::Next,
            c::PreviousTrack => Command::Previous,
            c::SeekToPlaybackPosition => match options.real(f::CommandOptions::playbackPosition) {
                Some(p) if p.is_finite() && p >= 0.0 => Command::Seek(p),
                _ => return e::InvalidOptions,
            },
            _ => return e::NotSupported,
        };
        if self.control.command(command).await {
            e::NoError
        } else {
            e::ConnectionFailed
        }
    }

    /// Группы собирает отправитель (SETPEERS), а не MRP: принимаем только
    /// себя.
    fn modify_output_context(&self, body: &Node) -> u64 {
        use f::ModifyOutputContextRequestMessage as m;
        let d = &self.device;
        let own = [
            m::addingDevices,
            m::removingDevices,
            m::settingDevices,
            m::clusterAwareAddingDevices,
            m::clusterAwareRemovingDevices,
            m::clusterAwareSettingDevices,
        ]
        .into_iter()
        .flat_map(|field| body.strings(field))
        .all(|uid| same_uid(uid, &d.uid) || same_uid(uid, &d.group));
        if own {
            0
        } else {
            value::ErrorCode::OneOrMoreSpecifiedOutputDevicesAreNotGroupable
        }
    }

    /// Что разослать подписанному iPhone после изменения карточки.
    fn changes(&self, old: &NowPlaying, now: &NowPlaying) -> Vec<Node> {
        let d = &self.device;
        let mut out = Vec::new();
        if now.app != old.app {
            out.extend(messages::announcement(d, now));
        } else if now.title != old.title
            || now.artist != old.artist
            || now.album != old.album
            || now.content_id != old.content_id
            || now.artwork != old.artwork
            || now.duration != old.duration
            || now.control_available != old.control_available
            || now.can_seek != old.can_seek
            || now.playing != old.playing
            || (now.position - old.position).abs() >= 1.0
        {
            out.push(messages::state(d, now, None, None));
        }
        if now.route != old.route {
            out.push(messages::device_info_update(d, now));
        }
        if now.control_available != old.control_available || now.route != old.route {
            out.extend(messages::topology(d, now));
        } else if (now.volume - old.volume).abs() > VOLUME_STEP {
            out.push(messages::volume(d, now));
        }
        out
    }
}

fn reply_if(id: Option<&str>, error: u64) -> Vec<Node> {
    id.map(|id| vec![messages::error_reply(Some(id), error)])
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
