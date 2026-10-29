//! Приём видеопотока: одно TCP-подключение, пакеты из 128 байт заголовка и
//! тела. Тело кадров зашифровано AES-128-CTR одним потоком на всё
//! подключение.

use std::time::Duration;

use crypto::ctr;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use crate::{Codec, VideoSink};

pub const HEADER_LEN: usize = 128;
const MAX_PAYLOAD: usize = 32 * 1024 * 1024;
/// Отправитель шлёт хотя бы пульс раз в секунду; дольше тишины — обрыв.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketKind {
    /// Кадр (зашифрован).
    Frame,
    /// `avcC` или `hvcC`.
    Config,
    /// Пульс, тело пустое или служебное.
    Heartbeat,
    /// Служебный plist.
    Plist,
    Other(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub payload_len: usize,
    pub kind: PacketKind,
    /// Время показа: NTP 32.32 по часам отправителя.
    pub ntp_time: u64,
}

impl Header {
    pub fn parse(h: &[u8; HEADER_LEN]) -> Header {
        let payload_len = u32::from_le_bytes(h[0..4].try_into().unwrap()) as usize;
        let kind = match u16::from_le_bytes([h[4], h[5]]) {
            0 | 0x1000 => PacketKind::Frame,
            1 => PacketKind::Config,
            2 => PacketKind::Heartbeat,
            5 => PacketKind::Plist,
            other => PacketKind::Other(other),
        };
        let ntp_time = u64::from_le_bytes(h[8..16].try_into().unwrap());
        Header {
            payload_len,
            kind,
            ntp_time,
        }
    }

    /// Время NTP 32.32 в наносекунды.
    pub fn pts_ns(&self) -> u64 {
        let secs = self.ntp_time >> 32;
        let frac = self.ntp_time & 0xFFFF_FFFF;
        secs * 1_000_000_000 + ((frac * 1_000_000_000) >> 32)
    }
}

/// Принимает одно подключение на `listener` и отдаёт видео в `sink`.
pub async fn receive(
    listener: TcpListener,
    key: [u8; 16],
    iv: [u8; 16],
    mut sink: Box<dyn VideoSink>,
) {
    let mut conn = match listener.accept().await {
        Ok((conn, peer)) => {
            info!(%peer, "видеопоток подключён");
            conn
        }
        Err(e) => {
            warn!("видеопоток: {e}");
            return;
        }
    };
    let _ = conn.set_nodelay(true);
    let mut cipher = ctr::aes128_ctr(&key, &iv);
    let mut header = [0u8; HEADER_LEN];
    let mut payload = Vec::new();
    let mut frames = 0u64;
    loop {
        if !read(&mut conn, &mut header).await {
            break;
        }
        let h = Header::parse(&header);
        if h.payload_len > MAX_PAYLOAD {
            warn!(len = h.payload_len, "видеопакет слишком большой");
            break;
        }
        payload.resize(h.payload_len, 0);
        if !read(&mut conn, &mut payload).await {
            break;
        }
        match h.kind {
            PacketKind::Frame => {
                ctr::apply(&mut cipher, &mut payload);
                sink.frame(h.pts_ns(), &payload);
                frames += 1;
            }
            PacketKind::Config => {
                let codec = if payload.get(4..8) == Some(b"hvc1") {
                    Codec::H265
                } else {
                    Codec::H264
                };
                info!(?codec, len = payload.len(), "параметры видео");
                sink.config(codec, &payload);
            }
            other => debug!(?other, len = payload.len(), "видеопакет"),
        }
    }
    info!(frames, "видеопоток закончился");
    sink.end();
}

async fn read(conn: &mut tokio::net::TcpStream, buf: &mut [u8]) -> bool {
    match tokio::time::timeout(READ_TIMEOUT, conn.read_exact(buf)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => {
            debug!("видеопоток: {e}");
            false
        }
        Err(_) => {
            debug!("видеопоток: тишина {READ_TIMEOUT:?}");
            false
        }
    }
}
