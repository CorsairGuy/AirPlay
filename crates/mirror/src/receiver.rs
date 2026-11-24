//! Приём видеопотока: одно TCP-подключение, пакеты из 128 байт заголовка и
//! тела. Поддерживаются legacy AES-128-CTR и аутентифицированные кадры
//! ChaCha20-Poly1305 с ключом, полученным из HAP-сессии.

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
        let kind = match h[4] {
            0 => PacketKind::Frame,
            1 => PacketKind::Config,
            2 => PacketKind::Heartbeat,
            5 => PacketKind::Plist,
            other => PacketKind::Other(u16::from(other)),
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
pub async fn receive(listener: TcpListener, key: [u8; 16], iv: [u8; 16], sink: Box<dyn VideoSink>) {
    receive_stream(listener, Some((key, iv)), None, sink).await;
}

/// HAP-derived video: authenticate each frame against its complete wire header.
pub async fn receive_authenticated(listener: TcpListener, key: [u8; 32], sink: Box<dyn VideoSink>) {
    receive_stream(listener, None, Some(key), sink).await;
}

async fn receive_stream(
    listener: TcpListener,
    legacy: Option<([u8; 16], [u8; 16])>,
    authenticated: Option<[u8; 32]>,
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
    let mut cipher = legacy.map(|(key, iv)| ctr::aes128_ctr(&key, &iv));
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
                if let Some(cipher) = cipher.as_mut() {
                    ctr::apply(cipher, &mut payload);
                } else if let Some(key) = authenticated.as_ref() {
                    match crypto::aead::open(key, &frames.to_le_bytes(), &header, &payload) {
                        Ok(plain) => payload = plain,
                        Err(_) => {
                            warn!(frames, "video frame authentication failed");
                            break;
                        }
                    }
                } else {
                    break;
                }
                sink.frame(h.pts_ns(), &payload);
                let Some(next) = frames.checked_add(1) else {
                    break;
                };
                frames = next;
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

#[cfg(test)]
mod authenticated_tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::{io::AsyncWriteExt, net::TcpStream};

    struct Sink(Arc<Mutex<Vec<Vec<u8>>>>);
    impl VideoSink for Sink {
        fn config(&mut self, _: Codec, _: &[u8]) {}
        fn frame(&mut self, _: u64, bytes: &[u8]) {
            self.0.lock().unwrap().push(bytes.to_vec());
        }
    }

    #[tokio::test]
    async fn authenticated_frames_and_tampered_header() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let frames = Arc::new(Mutex::new(Vec::new()));
        let key = [7; 32];
        let task = tokio::spawn(receive_authenticated(
            listener,
            key,
            Box::new(Sink(frames.clone())),
        ));
        let mut client = TcpStream::connect(address).await.unwrap();
        for counter in 0u64..3 {
            let mut header = [0u8; HEADER_LEN];
            header[..4].copy_from_slice(&(19u32).to_le_bytes());
            let data = crypto::aead::seal(&key, &counter.to_le_bytes(), &header, b"abc");
            if counter == 2 {
                header[8] = 1;
            }
            client.write_all(&header).await.unwrap();
            client.write_all(&data).await.unwrap();
        }
        drop(client);
        task.await.unwrap();
        assert_eq!(
            *frames.lock().unwrap(),
            vec![b"abc".to_vec(), b"abc".to_vec()]
        );
    }
}
