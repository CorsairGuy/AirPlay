//! Сокеты потока: TCP для буферизованного, UDP (данные и управление) для
//! realtime. Каждый поток — пара задач tokio; вывод идёт в [`Playout`].

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use clock::{Anchor, TimeSource};
use codec::Format;
use rtp::AudioCipher;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::playout::{Config, Playout};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// Тип 96: ALAC/AAC по UDP, играется сразу, с малой задержкой.
    Realtime,
    /// Тип 103: AAC по TCP, присылается заранее, время — по SETRATEANCHORTIME.
    Buffered,
}

pub struct Stream {
    pub kind: StreamKind,
    pub data_port: u16,
    pub control_port: u16,
    playout: Arc<Playout>,
    tasks: Vec<JoinHandle<()>>,
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
        let cipher = Arc::new(AudioCipher::new(shk));
        let tasks = vec![
            tokio::spawn(accept_buffered(listener, cipher, playout.clone())),
            // Отправитель шлёт сюда редко и ничего важного; читаем, чтобы не копилось.
            tokio::spawn(drain(control)),
        ];
        Ok(Stream {
            kind: StreamKind::Buffered,
            data_port,
            control_port,
            playout,
            tasks,
        })
    }

    pub fn realtime(
        cfg: Config,
        shk: &[u8; 32],
        format: Format,
        time: Arc<dyn TimeSource>,
    ) -> io::Result<Stream> {
        let data = bind_udp()?;
        let control = bind_udp()?;
        let data_port = data.local_addr()?.port();
        let control_port = control.local_addr()?.port();
        let playout = Arc::new(Playout::start(cfg, time, Some(format)));
        let cipher = Arc::new(AudioCipher::new(shk));
        let tasks = vec![
            tokio::spawn(receive_realtime(data, cipher.clone(), playout.clone())),
            tokio::spawn(receive_control(control, cipher, playout.clone())),
        ];
        Ok(Stream {
            kind: StreamKind::Realtime,
            data_port,
            control_port,
            playout,
            tasks,
        })
    }

    pub fn playout(&self) -> &Playout {
        &self.playout
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        self.playout.stop();
    }
}

/// TCP на случайном порту: `[::]` (принимает и IPv4), иначе `0.0.0.0`.
pub fn bind_tcp() -> io::Result<TcpListener> {
    let l = std::net::TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0))
        .or_else(|_| std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
    l.set_nonblocking(true)?;
    TcpListener::from_std(l)
}

fn bind_udp() -> io::Result<UdpSocket> {
    let s = std::net::UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0))
        .or_else(|_| std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)))?;
    s.set_nonblocking(true)?;
    UdpSocket::from_std(s)
}

async fn accept_buffered(listener: TcpListener, cipher: Arc<AudioCipher>, playout: Arc<Playout>) {
    loop {
        match listener.accept().await {
            Ok((conn, peer)) => {
                debug!(%peer, "буферизованный поток подключён");
                // Новое подключение заменяет старое: iPhone переподключается сам.
                read_buffered(conn, &cipher, &playout).await;
            }
            Err(e) => {
                warn!("приём аудиопотока: {e}");
                return;
            }
        }
    }
}

async fn read_buffered(mut conn: TcpStream, cipher: &AudioCipher, playout: &Playout) {
    let _ = conn.set_nodelay(true);
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut chunk = vec![0u8; 32 * 1024];
    loop {
        let n = match conn.read(&mut chunk).await {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) => {
                debug!("аудиопоток: {e}");
                return;
            }
        };
        buf.extend_from_slice(&chunk[..n]);
        while let Some(mut packet) = rtp::split_buffered(&mut buf) {
            push(cipher, playout, &mut packet);
        }
    }
}

fn push(cipher: &AudioCipher, playout: &Playout, packet: &mut [u8]) -> Option<u32> {
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

async fn receive_realtime(socket: UdpSocket, cipher: Arc<AudioCipher>, playout: Arc<Playout>) {
    let mut buf = vec![0u8; 2048];
    loop {
        let n = match socket.recv(&mut buf).await {
            Ok(n) => n,
            Err(e) => {
                debug!("realtime: {e}");
                continue;
            }
        };
        if let Some(ts) = push(&cipher, &playout, &mut buf[..n]) {
            playout.play_from(ts);
        }
    }
}

/// Управляющий канал realtime: синхропакеты с PTP-временем (0xD7) и
/// повторно присланные пакеты (0xD6).
async fn receive_control(socket: UdpSocket, cipher: Arc<AudioCipher>, playout: Arc<Playout>) {
    let mut buf = vec![0u8; 2048];
    loop {
        let (n, _from): (usize, SocketAddr) = match socket.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                debug!("управление: {e}");
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
            0x56 => {
                push(&cipher, &playout, &mut packet[4..]);
            }
            other => debug!(kind = other, "управляющий пакет"),
        }
    }
}

async fn drain(socket: UdpSocket) {
    let mut buf = vec![0u8; 2048];
    while socket.recv(&mut buf).await.is_ok() {}
}
