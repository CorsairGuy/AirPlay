//! Сокеты потока: TCP для буферизованного, UDP (данные и управление) для
//! realtime. Каждый поток — пара задач tokio; вывод идёт в [`Playout`].

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
/// Не чаще этого шлём запросы пересылки: shairport-sync `7ce90d8`
/// (`rtp.c`, `rtp_time_of_last_resend_request_error_ns`) так же гасит их
/// шторм, когда предыдущий `sendto` не ушёл.
const RESEND_BACKOFF: Duration = Duration::from_millis(300);
/// Больше этого за раз не просим: живой отправитель пересылает несколько
/// десятков пакетов, а больший разрыв — обрыв потока, не потеря.
const MAX_RESEND: u16 = 32;

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

/// Запросы пересылки потерянных пакетов AirPlay 1. Отправитель отвечает
/// пакетами `0x80 0x56` в этот же управляющий порт.
#[derive(Clone)]
struct Resend {
    socket: Arc<UdpSocket>,
    /// `control_port` из `Transport` при SETUP: порт отправителя, не наш.
    target: SocketAddr,
}

impl Resend {
    /// `0x80 0xd5`, номер запроса `1`, первый пропущенный seq, счётчик.
    /// Формат shairport-sync `7ce90d8` `rtp.c::rtp_request_resend`.
    fn request(first: u16, count: u16) -> [u8; 8] {
        let mut req = [0x80, 0xd5, 0, 1, 0, 0, 0, 0];
        req[4..6].copy_from_slice(&first.to_be_bytes());
        req[6..8].copy_from_slice(&count.to_be_bytes());
        req
    }

    async fn ask(&self, first: u16, count: u16) {
        if let Err(e) = self
            .socket
            .send_to(&Self::request(first, count), self.target)
            .await
        {
            debug!(addr = %self.target, first, count, "запрос пересылки: {e}");
        }
    }
}

/// Ожидаемый номер следующего пакета звука.
#[derive(Default)]
struct Missing {
    expected: Option<u16>,
    last: Option<Instant>,
}

impl Missing {
    /// Первый пропущенный номер и сколько запросить; `None` — потерь нет,
    /// разрыв слишком велик или запрос недавно уже уходил.
    fn observe(&mut self, packet: &[u8]) -> Option<(u16, u16)> {
        if packet.len() <= HEADER_LEN
            || packet[0] >> 6 != 2
            || rtp::Header::parse(packet).is_none_or(|header| header.payload_type != 0x60)
        {
            return None;
        }
        // Шапка RTP у AirPlay 1 открыта и при шифрованном теле.
        let seq = u16::from_be_bytes([packet[2], packet[3]]);
        let Some(previous) = self.expected else {
            self.expected = Some(seq.wrapping_add(1));
            return None;
        };
        let missing = seq.wrapping_sub(previous);
        if missing >= 0x8000 {
            return None;
        }
        self.expected = Some(seq.wrapping_add(1));
        if missing == 0 || missing > MAX_RESEND {
            return None;
        }
        if self.last.is_some_and(|at| at.elapsed() < RESEND_BACKOFF) {
            return None;
        }
        self.last = Some(Instant::now());
        Some((previous, missing))
    }
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
                None,
                None,
            )),
            tokio::spawn(receive_control(
                Arc::new(control),
                cipher,
                playout.clone(),
                None,
            )),
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
    /// запросов времени. `sender_control` — `control_port` из `Transport`;
    /// timing-сервис передаётся хостом, чтобы не дублировать движок часов.
    pub fn raop<F>(
        cfg: Config,
        key: StreamKey,
        format: Format,
        sender_control: Option<u16>,
        sender: IpAddr,
        start_timing: F,
    ) -> io::Result<Stream>
    where
        F: FnOnce() -> io::Result<(u16, Arc<dyn TimeSource>, JoinHandle<()>)>,
    {
        let data = bind_udp()?;
        let control = Arc::new(bind_udp()?);
        let data_port = data.local_addr()?.port();
        let control_port = control.local_addr()?.port();
        let (timing_port, time, timing_task) = start_timing()?;
        let resend = sender_control.map(|port| Resend {
            socket: control.clone(),
            target: SocketAddr::new(sender, port),
        });
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
                resend,
                Some(sender),
            )),
            tokio::spawn(receive_control(
                control,
                cipher,
                playout.clone(),
                Some(sender),
            )),
            timing_task,
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
    resend: Option<Resend>,
    allowed_peer: Option<IpAddr>,
) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    let mut failures = 0u64;
    let mut missing = Missing::default();
    loop {
        // `recv_from`, а не `recv`: по источнику первого пакета узнаём
        // адрес отправителя для запросов пересылки.
        let (n, source): (usize, SocketAddr) = match socket.recv_from(&mut buf).await {
            Ok(r) => {
                failures = 0;
                r
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
        if allowed_peer.is_some_and(|peer| peer.to_canonical() != source.ip().to_canonical()) {
            continue;
        }
        if let Some(resend) = &resend {
            if let Some((first, count)) = missing.observe(&buf[..n]) {
                resend.ask(first, count).await;
            }
        }
        // Пересланный пакет приходит на порт звука с четырёхбайтовой
        // приставкой `0x80 0x56` (shairport-sync `7ce90d8` `rtp.c`).
        let resent = resend.is_some() && n > 4 && buf[1] & 0x7f == 0x56;
        let from = if resent { 4 } else { 0 };
        if let Some(ts) = push(&cipher, &playout, &mut buf[from..n]) {
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
async fn receive_control(
    socket: Arc<UdpSocket>,
    cipher: Arc<Decrypt>,
    playout: Arc<Playout>,
    allowed_peer: Option<IpAddr>,
) {
    let mut buf = vec![0u8; MAX_DATAGRAM];
    let mut failures = 0u64;
    loop {
        let (n, from): (usize, SocketAddr) = match socket.recv_from(&mut buf).await {
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
        if allowed_peer.is_some_and(|peer| peer.to_canonical() != from.ip().to_canonical()) {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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

    async fn accepted_from(allowed: IpAddr) -> bool {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        let format = codec::Format {
            codec: codec::Codec::Pcm {
                frames_per_packet: 1024,
            },
            sample_rate: 44_100,
            channels: 2,
        };
        let factory = audio::SinkFactory::new(|_, _| {
            Ok(Box::new(audio::Null::new(44_100)) as Box<dyn audio::Sink>)
        });
        let playout = Arc::new(Playout::start(
            Config {
                device: "null".into(),
                sink: Some(factory),
                ..Default::default()
            },
            Arc::new(clock::NoTime),
            Some(format),
        ));
        let (started_tx, mut started_rx) = watch::channel(false);
        let task = tokio::spawn(receive_realtime(
            socket,
            Arc::new(Decrypt::Plain),
            playout.clone(),
            started_tx,
            None,
            Some(allowed),
        ));
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut packet = Header {
            marker: true,
            payload_type: 96,
            seq: 1,
            timestamp: 0,
            ssrc: 1,
        }
        .encode()
        .to_vec();
        packet.extend_from_slice(&[0; 4]);
        sender.send_to(&packet, address).await.unwrap();
        let accepted = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if *started_rx.borrow_and_update() {
                    return true;
                }
                if started_rx.changed().await.is_err() {
                    return false;
                }
            }
        })
        .await
        .unwrap_or(false);
        task.abort();
        accepted
    }

    /// Пакет с чужого адреса не запускает воспроизведение, даже если порт
    /// совпал; подтверждённый адрес принимается.
    #[tokio::test]
    async fn realtime_audio_is_accepted_only_from_the_expected_source() {
        assert!(
            !accepted_from(IpAddr::from([127, 0, 0, 2])).await,
            "чужой источник принят"
        );
        assert!(
            accepted_from(IpAddr::from([127, 0, 0, 1])).await,
            "подтверждённый источник отвергнут"
        );
    }
}
#[cfg(test)]
mod resend_tests {
    use super::{Missing, Resend};

    fn audio(seq: u16) -> Vec<u8> {
        let mut packet = vec![0u8; 13];
        packet[0] = 0x80;
        packet[1] = 0x60;
        packet[2..4].copy_from_slice(&seq.to_be_bytes());
        packet
    }

    #[test]
    fn request_matches_the_wire_format() {
        // shairport-sync `7ce90d8` `rtp.c::rtp_request_resend`: 0x80 0xd5,
        // номер запроса 1, первый пропущенный seq, счётчик.
        assert_eq!(
            Resend::request(1000, 2),
            [0x80, 0xd5, 0, 1, 0x03, 0xe8, 0, 2]
        );
    }

    #[test]
    fn a_missed_packet_is_requested_once() {
        let mut missing = Missing::default();
        assert_eq!(missing.observe(&audio(1)), None);
        assert_eq!(missing.observe(&audio(2)), None);
        assert_eq!(missing.observe(&audio(4)), Some((3, 1)));
        // Повтор того же разрыва не должен слать второй запрос.
        assert_eq!(missing.observe(&audio(5)), None);
    }

    #[test]
    fn late_packets_do_not_rewind_sequence_tracking() {
        let mut missing = Missing::default();
        assert_eq!(missing.observe(&audio(1)), None);
        assert_eq!(missing.observe(&audio(2)), None);
        assert_eq!(missing.observe(&audio(1)), None);
        assert_eq!(missing.observe(&audio(3)), None);
        assert_eq!(missing.observe(&audio(5)), Some((4, 1)));
    }

    #[test]
    fn sequence_wraps_and_big_gaps_are_ignored() {
        let mut missing = Missing::default();
        assert_eq!(missing.observe(&audio(0xfffe)), None);
        assert_eq!(missing.observe(&audio(0)), Some((0xffff, 1)));
        // Разрыв в сотни пакетов — обрыв потока, а не потеря в сети.
        let mut other = Missing::default();
        assert_eq!(other.observe(&audio(0)), None);
        assert_eq!(other.observe(&audio(900)), None);
    }

    #[test]
    fn a_resent_packet_carries_a_four_byte_prefix() {
        // `0x80 0x56` + RTP-шапка: seq лежит по смещению 4 + 2.
        let mut packet = vec![0x80, 0x56, 0, 0];
        packet.extend_from_slice(&audio(9));
        let stripped = &packet[4..];
        assert_eq!(u16::from_be_bytes([stripped[2], stripped[3]]), 9);
    }

    #[test]
    fn control_packets_and_short_datagrams_are_not_tracked() {
        let mut missing = Missing::default();
        let mut sync = audio(7);
        sync[1] = 0xd4;
        assert_eq!(missing.observe(&sync), None);
        assert_eq!(missing.observe(&[0x80, 0x60]), None);
    }
}
