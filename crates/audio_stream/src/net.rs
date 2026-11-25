//! Сокеты потока: TCP для буферизованного, UDP (данные и управление) для
//! realtime. Каждый поток — пара задач tokio; вывод идёт в [`Playout`].

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clock::{Anchor, TimeSource};
use codec::Format;
use crypto::cbc::AudioCbc;
use rtp::{AudioCipher, HEADER_LEN, Header};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::playout::{Config, Playout};

/// Самая большая UDP-датаграмма. Realtime-пакет ALAC 24 бита по 352 кадра —
/// это 2112 байт данных плюс заголовок RTP и тег ChaCha; в буфер на 2048 он
/// не влезает, а `recv` молча обрезает датаграмму, после чего расшифровка
/// не проходит и наступает тишина.
const MAX_DATAGRAM: usize = 65_535;
/// Предел незавершённого буферизованного пакета: испорченная длина не должна
/// съедать память.
const MAX_BUFFERED_BYTES: usize = 4 * 1024 * 1024;
/// Сколько подряд ошибок `accept` терпим, прежде чем закрыть приём.
const MAX_ACCEPT_FAILURES: u32 = 10;

/// Ключ realtime-потока.
#[derive(Clone, Copy)]
pub enum StreamKey {
    /// AirPlay 2: `shk` из SETUP, ChaCha20-Poly1305.
    ChaCha([u8; 32]),
    /// AirPlay 1 и звук зеркалирования: ключ сессии и `eiv`, AES-128-CBC.
    AesCbc { key: [u8; 16], iv: [u8; 16] },
    /// AirPlay 1 без шифрования (`et:0`, без `rsaaeskey`): payload открыт.
    Plain,
}

enum Decrypt {
    ChaCha(AudioCipher),
    Cbc(Box<AudioCbc>),
    Plain,
}

impl Decrypt {
    fn open<'a>(&self, packet: &'a mut [u8]) -> Option<(Header, &'a [u8])> {
        match self {
            Decrypt::ChaCha(c) => c.open(packet),
            Decrypt::Cbc(c) => {
                let header = Header::parse(packet)?;
                let body = &mut packet[HEADER_LEN..];
                c.decrypt(body);
                Some((header, body))
            }
            Decrypt::Plain => {
                let header = Header::parse(packet)?;
                Some((header, &packet[HEADER_LEN..]))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// Тип 96: ALAC/AAC по UDP, играется сразу, с малой задержкой.
    Realtime,
    /// Тип 103: AAC по TCP, присылается заранее, время — по SETRATEANCHORTIME.
    Buffered,
}

/// Текущее TCP-подключение буферизованного потока. Отправитель открывает
/// новое подключение, не всегда закрывая прежнее, поэтому его нужно уметь
/// заменить, пока старое ещё читается.
type Connection = Arc<Mutex<Option<JoinHandle<()>>>>;

pub struct Stream {
    pub kind: StreamKind,
    pub data_port: u16,
    pub control_port: u16,
    /// Порт запросов времени RAOP; 0 — не открыт.
    pub timing_port: u16,
    playout: Arc<Playout>,
    tasks: Vec<JoinHandle<()>>,
    started: Option<watch::Receiver<bool>>,
    connection: Option<Connection>,
}

impl Stream {
    /// Сокеты занимаются сразу, задачи приёма запускаются в текущем
    /// рантайме tokio — вызывать изнутри него.
    pub fn buffered(cfg: Config, shk: &[u8; 32], time: Arc<dyn TimeSource>) -> io::Result<Stream> {
        let listener = bind_tcp()?;
        let control = bind_udp()?;
        let data_port = listener.local_addr()?.port();
        let control_port = control.local_addr()?.port();
        let playout = Arc::new(Playout::start(cfg, time, None));
        let cipher = Arc::new(Decrypt::ChaCha(AudioCipher::new(shk)));
        let connection: Connection = Arc::new(Mutex::new(None));
        let tasks = vec![
            tokio::spawn(accept_buffered(
                listener,
                cipher,
                playout.clone(),
                connection.clone(),
            )),
            // Отправитель шлёт сюда редко и ничего важного; читаем, чтобы не копилось.
            tokio::spawn(drain(control)),
        ];
        Ok(Stream {
            started: None,
            kind: StreamKind::Buffered,
            data_port,
            control_port,
            timing_port: 0,
            playout,
            tasks,
            connection: Some(connection),
        })
    }

    pub fn realtime(
        cfg: Config,
        key: StreamKey,
        format: Format,
        time: Arc<dyn TimeSource>,
    ) -> io::Result<Stream> {
        let data = bind_udp()?;
        let control = bind_udp()?;
        let data_port = data.local_addr()?.port();
        let control_port = control.local_addr()?.port();
        let playout = Arc::new(Playout::start(cfg, time, Some(format)));
        let cipher = Arc::new(match key {
            StreamKey::ChaCha(shk) => Decrypt::ChaCha(AudioCipher::new(&shk)),
            StreamKey::AesCbc { key, iv } => Decrypt::Cbc(Box::new(AudioCbc::new(&key, &iv))),
            StreamKey::Plain => Decrypt::Plain,
        });
        let (started_tx, started) = watch::channel(false);
        let tasks = vec![
            tokio::spawn(receive_realtime(
                data,
                cipher.clone(),
                playout.clone(),
                started_tx,
            )),
            tokio::spawn(receive_control(control, cipher, playout.clone())),
        ];
        Ok(Stream {
            started: Some(started),
            kind: StreamKind::Realtime,
            data_port,
            control_port,
            timing_port: 0,
            playout,
            tasks,
            connection: None,
        })
    }

    /// AirPlay 1 (RAOP): звук по UDP, управляющий канал и отдельный порт
    /// запросов времени. Часов отправителя у нас нет: воспроизведение
    /// начинается с первого пакета по своим часам.
    pub fn raop(cfg: Config, key: StreamKey, format: Format) -> io::Result<Stream> {
        let data = bind_udp()?;
        let control = bind_udp()?;
        let timing = bind_udp()?;
        let data_port = data.local_addr()?.port();
        let control_port = control.local_addr()?.port();
        let timing_port = timing.local_addr()?.port();
        let playout = Arc::new(Playout::start(cfg, Arc::new(clock::NoTime), Some(format)));
        let cipher = Arc::new(match key {
            StreamKey::ChaCha(shk) => Decrypt::ChaCha(AudioCipher::new(&shk)),
            StreamKey::AesCbc { key, iv } => Decrypt::Cbc(Box::new(AudioCbc::new(&key, &iv))),
            StreamKey::Plain => Decrypt::Plain,
        });
        let (started_tx, started) = watch::channel(false);
        let tasks = vec![
            tokio::spawn(receive_realtime(
                data,
                cipher.clone(),
                playout.clone(),
                started_tx,
            )),
            tokio::spawn(receive_control(control, cipher, playout.clone())),
            tokio::spawn(receive_timing(timing)),
        ];
        Ok(Stream {
            started: Some(started),
            kind: StreamKind::Realtime,
            data_port,
            control_port,
            timing_port,
            playout,
            tasks,
            connection: None,
        })
    }

    /// Notify only after the first accepted realtime audio packet, not SETUP.
    /// The observer is cancelled with this stream.
    pub fn on_realtime_started(&mut self, callback: impl FnOnce() + Send + 'static) {
        let Some(mut started) = self.started.clone() else {
            return;
        };
        self.tasks.push(tokio::spawn(async move {
            loop {
                if *started.borrow_and_update() {
                    callback();
                    return;
                }
                if started.changed().await.is_err() {
                    return;
                }
            }
        }));
    }

    pub fn playout(&self) -> &Playout {
        &self.playout
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // Читатель подключения живёт отдельной задачей: его надо закрыть
        // вместе с потоком, иначе он продолжит принимать пакеты в мёртвый вывод.
        if let Some(connection) = &self.connection
            && let Some(reader) = connection
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take()
        {
            reader.abort();
        }
        for t in &self.tasks {
            t.abort();
        }
        self.playout.stop();
    }
}

/// TCP на случайном порту для IPv4 и IPv6 сразу. `IPV6_V6ONLY` снимаем
/// явно: на Windows он включён по умолчанию и сокет `[::]` не принимает IPv4.
pub fn bind_tcp() -> io::Result<TcpListener> {
    let s = dual_stack(Type::STREAM, Protocol::TCP)?;
    s.listen(16)?;
    TcpListener::from_std(s.into())
}

fn bind_udp() -> io::Result<UdpSocket> {
    UdpSocket::from_std(dual_stack(Type::DGRAM, Protocol::UDP)?.into())
}

fn dual_stack(ty: Type, proto: Protocol) -> io::Result<Socket> {
    let v6 = Socket::new(Domain::IPV6, ty, Some(proto)).and_then(|s| {
        s.set_only_v6(false)?;
        s.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)).into())?;
        Ok(s)
    });
    let s = match v6 {
        Ok(s) => s,
        // Без IPv6 в системе.
        Err(_) => {
            let s = Socket::new(Domain::IPV4, ty, Some(proto))?;
            s.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into())?;
            s
        }
    };
    s.set_nonblocking(true)?;
    Ok(s)
}

async fn accept_buffered(
    listener: TcpListener,
    cipher: Arc<Decrypt>,
    playout: Arc<Playout>,
    connection: Connection,
) {
    let mut failures = 0u32;
    let (verified_tx, mut verified_rx) = tokio::sync::mpsc::channel(1);
    let mut candidate: Option<JoinHandle<()>> = None;
    loop {
        tokio::select! {
        Some((conn, buffered, timestamp, ssrc, payload)) = verified_rx.recv() => {
            if let Some(previous) = connection.lock().unwrap_or_else(|error| error.into_inner()).take() {
                previous.abort();
            }
            playout.push(timestamp, ssrc, payload);
            let cipher = cipher.clone();
            let playout = playout.clone();
            let reader = tokio::spawn(async move {
                read_buffered(conn, &cipher, &playout, buffered).await;
            });
            *connection.lock().unwrap_or_else(|error| error.into_inner()) = Some(reader);
        }
        accepted = listener.accept() => match accepted {
            Ok((conn, peer)) => {
                failures = 0;
                // Keep the current stream until an encrypted packet passes
                // authentication. This does not itself prevent packet replay.
                if let Some(previous) = candidate.take() { previous.abort(); }
                let cipher = cipher.clone();
                let verified_tx = verified_tx.clone();
                candidate = Some(tokio::spawn(async move {
                    if let Ok(Some(verified)) = tokio::time::timeout(
                        Duration::from_secs(5), verify_buffered(conn, &cipher)
                    ).await {
                        debug!(%peer, "буферизованный поток подтверждён");
                        let _ = verified_tx.send(verified).await;
                    }
                }));
            }
            Err(e) => {
                // Временная ошибка (кончились дескрипторы, сбой сети) не должна
                // убивать приём навсегда.
                failures += 1;
                warn!(failures, "приём аудиопотока: {e}");
                if failures >= MAX_ACCEPT_FAILURES {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }}
    }
}

type Verified = (TcpStream, Vec<u8>, u32, u32, Vec<u8>);

async fn verify_buffered(mut conn: TcpStream, cipher: &Decrypt) -> Option<Verified> {
    let mut buffered = Vec::new();
    let mut chunk = [0u8; 32 * 1024];
    loop {
        let n = conn.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buffered.extend_from_slice(&chunk[..n]);
        while let Some(mut packet) = rtp::split_buffered(&mut buffered) {
            if let Some((header, payload)) = cipher.open(&mut packet) {
                return Some((
                    conn,
                    buffered,
                    header.timestamp,
                    header.ssrc,
                    payload.to_vec(),
                ));
            }
        }
        if buffered.len() > MAX_BUFFERED_BYTES {
            return None;
        }
    }
}

async fn read_buffered(mut conn: TcpStream, cipher: &Decrypt, playout: &Playout, mut buf: Vec<u8>) {
    let _ = conn.set_nodelay(true);
    let mut chunk = vec![0u8; 32 * 1024];
    loop {
        while let Some(mut packet) = rtp::split_buffered(&mut buf) {
            push(cipher, playout, &mut packet);
        }
        let n = match conn.read(&mut chunk).await {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) => {
                debug!("аудиопоток: {e}");
                return;
            }
        };
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_BUFFERED_BYTES {
            // Длина пакета заведомо испорчена: ждать её конца бессмысленно.
            warn!(
                bytes = buf.len(),
                "буферизованный поток: пакет больше предела, подключение закрыто"
            );
            return;
        }
    }
}

fn push(cipher: &Decrypt, playout: &Playout, packet: &mut [u8]) -> Option<u32> {
    match cipher.open(packet) {
        Some((h, payload)) => {
            playout.push(h.timestamp, h.ssrc, payload.to_vec());
            Some(h.timestamp)
        }
        None => {
            debug!(len = packet.len(), "пакет не расшифрован");
            None
        }
    }
}

async fn receive_realtime(
    socket: UdpSocket,
    cipher: Arc<Decrypt>,
    playout: Arc<Playout>,
    started: watch::Sender<bool>,
) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    let mut failures = 0u64;
    loop {
        let n = match socket.recv(&mut buf).await {
            Ok(n) => {
                failures = 0;
                n
            }
            Err(e) => {
                // На Windows ICMP «порт недоступен» приходит как ошибка recv;
                // без паузы цикл крутится вхолостую и жжёт процессор.
                failures += 1;
                if failures % 100 == 1 {
                    debug!(failures, "realtime: {e}");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };
        if let Some(ts) = push(&cipher, &playout, &mut buf[..n]) {
            playout.play_from(ts);
            started.send_if_modified(|value| {
                if *value {
                    false
                } else {
                    *value = true;
                    true
                }
            });
        }
    }
}

/// Управляющий канал realtime: синхропакеты с PTP-временем (0xD7) и
/// повторно присланные пакеты (0xD6).
async fn receive_control(socket: UdpSocket, cipher: Arc<Decrypt>, playout: Arc<Playout>) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    let mut failures = 0u64;
    loop {
        let (n, _from): (usize, SocketAddr) = match socket.recv_from(&mut buf).await {
            Ok(r) => {
                failures = 0;
                r
            }
            Err(e) => {
                failures += 1;
                if failures % 100 == 1 {
                    debug!(failures, "управление: {e}");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
        };
        let packet = &mut buf[..n];
        if n < 4 {
            continue;
        }
        match packet[1] & 0x7F {
            0x57 if n >= 28 => {
                // [4..8] — метка, которая звучит в момент [8..16] (нс по
                // часам [20..28]); задержка отправителя уже учтена.
                let rtp = u32::from_be_bytes(packet[4..8].try_into().unwrap());
                let network_ns = u64::from_be_bytes(packet[8..16].try_into().unwrap());
                let timeline = u64::from_be_bytes(packet[20..28].try_into().unwrap());
                playout.set_anchor(Anchor {
                    rtp,
                    network_ns,
                    timeline,
                    playing: true,
                });
            }
            0x54 if n >= 20 => {
                // AirPlay 1: [4..8] звучит в момент [8..16] (NTP по часам
                // отправителя), [16..20] — метка, отправляемая сейчас.
                let rtp = u32::from_be_bytes(packet[4..8].try_into().unwrap());
                let ntp = u64::from_be_bytes(packet[8..16].try_into().unwrap());
                playout.set_anchor(Anchor {
                    rtp,
                    network_ns: clock::ntp_to_ns(ntp),
                    timeline: clock::NTP_TIMELINE,
                    playing: true,
                });
            }
            0x56 => {
                push(&cipher, &playout, &mut packet[4..]);
            }
            other => debug!(kind = other, "управляющий пакет"),
        }
    }
}

async fn drain(socket: UdpSocket) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    while socket.recv(&mut buf).await.is_ok() {}
}

/// Порт времени AirPlay 1: отправитель шлёт 32-байтный запрос `0xD2` и
/// ждёт ответ `0xD3` с его transmit-меткой как origin и нашим временем.
/// Без ответа отправитель не выставляет часы и не начинает звук.
async fn receive_timing(socket: UdpSocket) {
    let mut buf = [0u8; 128];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf).await else {
            tokio::time::sleep(Duration::from_millis(10)).await;
            continue;
        };
        if let Some(reply) = timing_reply(&buf[..n])
            && let Err(e) = socket.send_to(&reply, from).await
        {
            debug!("ответ времени: {e}");
        }
    }
}

fn timing_reply(request: &[u8]) -> Option<[u8; 32]> {
    if request.len() < 32 || request[1] & 0x7F != 0x52 {
        return None;
    }
    let mut reply = [0u8; 32];
    reply.copy_from_slice(&request[..32]);
    reply[0] = 0x80;
    reply[1] = 0xD3;
    // Origin — transmit-метка запроса, затем наши receive/transmit.
    reply[8..16].copy_from_slice(&request[24..32]);
    let now = ntp_now();
    reply[16..24].copy_from_slice(&now.to_be_bytes());
    reply[24..32].copy_from_slice(&now.to_be_bytes());
    Some(reply)
}

/// NTP-время от 1900 года, 32.32: отправитель сверяет по нему часы.
fn ntp_now() -> u64 {
    let since_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since_unix.as_secs() + 2_208_988_800;
    let frac = ((since_unix.subsec_nanos() as u64) << 32) / 1_000_000_000;
    (secs << 32) | frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_reply_echoes_origin_and_stamps_now() {
        let mut request = [0u8; 32];
        request[0] = 0x80;
        request[1] = 0xD2;
        request[4..8].copy_from_slice(&[0, 0, 0, 7]);
        request[8..16].copy_from_slice(&[1; 8]);
        request[24..32].copy_from_slice(&[2; 8]);
        let reply = timing_reply(&request).unwrap();
        assert_eq!(reply[1], 0xD3);
        assert_eq!(reply[4..8], request[4..8]);
        assert_eq!(reply[8..16], request[24..32]);
        assert_eq!(reply[16..24], reply[24..32]);
        assert_ne!(reply[16..24], [0; 8]);
        assert!(timing_reply(&request[..31]).is_none());
        let mut sync = request;
        sync[1] = 0xD4;
        assert!(timing_reply(&sync).is_none());
    }

    #[test]
    fn plain_audio_is_not_decrypted() {
        let header = Header {
            marker: true,
            payload_type: 96,
            seq: 1,
            timestamp: 352,
            ssrc: 0x1234,
        };
        let mut packet = header.encode().to_vec();
        packet.extend_from_slice(b"alac");
        let cipher = Decrypt::Plain;
        let (h, body) = cipher.open(&mut packet).unwrap();
        assert_eq!(h, header);
        assert_eq!(body, b"alac");
    }
}
