//! Подключение buffered-потока (type 103) к сокету приёмника.
//!
//! [`BufferedSender`](super::buffered::BufferedSender) — чистый транспорт над
//! `AsyncWrite`; здесь появляется то, что нужно вызывающему для настоящей
//! сессии: разбор ответа SETUP и TCP-соединение с `dataPort`, которое
//! приёмник назвал в этом ответе (наш собственный RX отдаёт `dataPort`,
//! `controlPort` и `audioBufferSize`, см. `services/media/src/rx.rs`).
//!
//! Границы: RTSP-запросы (`ANNOUNCE`/`SETUP`/`RECORD`/`FLUSH`/`TEARDOWN`)
//! по-прежнему отправляет вызывающий — этот слой не владеет управляющим
//! соединением и не переподключается сам. Аппаратной проверки нет; тесты
//! идут по loopback против собственного разбора `rtp::split_buffered`.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use plist::{Dictionary, Value};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::timeout;

use super::buffered::{BufferedSender, STREAM_TYPE_BUFFERED, StreamSetup, adts_frames};

/// Разобранный ответ на SETUP со стримами.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamReply {
    pub stream_type: u64,
    /// TCP-порт, к которому подключается отправитель.
    pub data_port: u16,
    /// Порт управляющего канала, если приёмник его назвал.
    pub control_port: Option<u16>,
    /// Глубина буфера приёмника в байтах (как её называет наш RX).
    pub audio_buffer_size: Option<u64>,
}

fn uint(d: &Dictionary, key: &str) -> Option<u64> {
    d.get(key).and_then(Value::as_unsigned_integer)
}

impl StreamReply {
    /// Достаёт первый стрим из тела ответа `{streams: [...]}`. Занумерованные
    /// поля проверяются: `dataPort` вне `u16` — ошибка, а не усечение.
    pub fn from_setup_reply(body: &Dictionary) -> Result<Self> {
        let streams = body
            .get("streams")
            .and_then(Value::as_array)
            .context("в ответе SETUP нет streams")?;
        let first = streams
            .first()
            .and_then(Value::as_dictionary)
            .context("в ответе SETUP пустой список streams")?;
        let stream_type = uint(first, "type").context("в ответе SETUP нет type")?;
        let data_port = uint(first, "dataPort").context("в ответе SETUP нет dataPort")?;
        if data_port == 0 || data_port > u16::MAX as u64 {
            bail!("dataPort вне диапазона TCP-порта: {data_port}");
        }
        let control_port = match uint(first, "controlPort") {
            None => None,
            Some(0) => None,
            Some(port) if port <= u16::MAX as u64 => Some(port as u16),
            Some(port) => bail!("controlPort вне диапазона TCP-порта: {port}"),
        };
        Ok(Self {
            stream_type,
            data_port: data_port as u16,
            control_port,
            audio_buffer_size: uint(first, "audioBufferSize"),
        })
    }

    /// Приёмник подтвердил именно buffered-аудио, а не другой тип стрима.
    pub fn require_buffered(&self) -> Result<()> {
        if self.stream_type != STREAM_TYPE_BUFFERED {
            bail!(
                "приёмник подтвердил стрим типа {}, ожидался {STREAM_TYPE_BUFFERED}",
                self.stream_type
            );
        }
        Ok(())
    }
}

/// TCP-соединение с `dataPort` приёмника и отправка кадров в него.
pub struct BufferedLink {
    sender: BufferedSender<TcpStream>,
    remote: SocketAddr,
    frames_sent: u64,
}

impl BufferedLink {
    /// Подключается к `addr` (обычно `ip приёмника : dataPort` из
    /// [`StreamReply`]) и готов отправлять кадры с заданными `ssrc`,
    /// `seq`/`timestamp`.
    pub async fn connect(
        addr: SocketAddr,
        setup: &StreamSetup,
        ssrc: u32,
        start_seq: u16,
        start_timestamp: u32,
        deadline: Duration,
    ) -> Result<Self> {
        if deadline.is_zero() {
            bail!("нулевой срок подключения к data-порту");
        }
        let stream = timeout(deadline, TcpStream::connect(addr))
            .await
            .with_context(|| format!("подключение к data-порту {addr}"))??;
        stream.set_nodelay(true)?;
        Ok(Self {
            sender: BufferedSender::new(stream, &setup.shk, ssrc, start_seq, start_timestamp),
            remote: addr,
            frames_sent: 0,
        })
    }

    /// Адрес приёмника, к которому открыто соединение.
    pub fn remote(&self) -> SocketAddr {
        self.remote
    }

    /// Сколько кадров успешно ушло в сокет.
    pub fn frames_sent(&self) -> u64 {
        self.frames_sent
    }

    pub fn next_sequence(&self) -> u16 {
        self.sender.next_sequence()
    }

    pub fn next_timestamp(&self) -> u32 {
        self.sender.next_timestamp()
    }

    /// Один уже закодированный кадр AAC-LC.
    pub async fn send_frame(&mut self, frame: &[u8]) -> Result<()> {
        self.sender
            .send_frame(frame)
            .await
            .context("кадр buffered-аудио")?;
        self.frames_sent += 1;
        Ok(())
    }

    /// Пачка кадров; при ошибке счётчик отражает только дошедшее.
    pub async fn send_frames(&mut self, frames: &[&[u8]]) -> Result<usize> {
        for frame in frames {
            self.send_frame(frame).await?;
        }
        Ok(frames.len())
    }

    /// Режет ADTS и отправляет все кадры. Возвращает число отправленных.
    pub async fn send_adts(&mut self, data: &[u8]) -> Result<usize> {
        let frames = adts_frames(data).map_err(|e| anyhow::anyhow!("разбор ADTS: {e:?}"))?;
        self.send_frames(&frames).await
    }

    /// Закрывает запись и дожидается, пока данные уйдут в сокет.
    pub async fn close(self) -> Result<()> {
        self.sender.into_inner().shutdown().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtp::AudioCipher;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    fn reply(items: &[(&str, u64)]) -> Dictionary {
        let mut d = Dictionary::new();
        for (k, v) in items {
            d.insert((*k).into(), Value::Integer((*v).into()));
        }
        let mut body = Dictionary::new();
        body.insert("streams".into(), Value::Array(vec![Value::Dictionary(d)]));
        body
    }

    #[test]
    fn reply_keeps_buffered_ports() {
        let body = reply(&[
            ("type", 103),
            ("dataPort", 7100),
            ("controlPort", 7101),
            ("audioBufferSize", 16384),
        ]);
        let reply = StreamReply::from_setup_reply(&body).unwrap();
        assert_eq!(
            reply,
            StreamReply {
                stream_type: 103,
                data_port: 7100,
                control_port: Some(7101),
                audio_buffer_size: Some(16384),
            }
        );
        reply.require_buffered().unwrap();
    }

    #[test]
    fn reply_without_ports_or_wrong_type_is_refused() {
        let missing = reply(&[("type", 103)]);
        assert!(StreamReply::from_setup_reply(&missing).is_err());
        let zero = reply(&[("type", 103), ("dataPort", 0)]);
        assert!(StreamReply::from_setup_reply(&zero).is_err());
        let huge = reply(&[("type", 103), ("dataPort", 70000)]);
        assert!(StreamReply::from_setup_reply(&huge).is_err());
        let empty = Dictionary::new();
        assert!(StreamReply::from_setup_reply(&empty).is_err());

        let realtime = reply(&[("type", 96), ("dataPort", 7100)]);
        let reply = StreamReply::from_setup_reply(&realtime).unwrap();
        assert!(reply.require_buffered().is_err());
    }

    #[test]
    fn zero_control_port_means_absent() {
        let body = reply(&[("type", 103), ("dataPort", 7100), ("controlPort", 0)]);
        assert_eq!(
            StreamReply::from_setup_reply(&body).unwrap().control_port,
            None
        );
    }

    fn setup() -> StreamSetup {
        StreamSetup::aac_44100(7101, [5; 32], 7)
    }

    #[tokio::test]
    async fn link_sends_decryptable_frames_to_the_data_port() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shk = [5u8; 32];
        let receiver = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut wire = Vec::new();
            socket.read_to_end(&mut wire).await.unwrap();
            wire
        });

        let mut link = BufferedLink::connect(
            addr,
            &setup(),
            0x1600_0000,
            10,
            352_000,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        link.send_frames(&[b"one", b"two"]).await.unwrap();
        assert_eq!(link.frames_sent(), 2);
        link.close().await.unwrap();

        let wire = receiver.await.unwrap();
        let cipher = AudioCipher::new(&shk);
        let mut buf = wire;
        let mut seen = Vec::new();
        while let Some(mut packet) = rtp::split_buffered(&mut buf) {
            let (header, payload) = cipher.open(&mut packet).unwrap();
            seen.push((header.seq, payload.to_vec()));
        }
        assert!(buf.is_empty());
        assert_eq!(seen, vec![(10, b"one".to_vec()), (11, b"two".to_vec())]);
    }

    #[tokio::test]
    async fn link_sends_adts_frames() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let receiver = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut wire = Vec::new();
            socket.read_to_end(&mut wire).await.unwrap();
            wire
        });

        let mut link =
            BufferedLink::connect(addr, &setup(), 0x1600_0000, 0, 0, Duration::from_secs(5))
                .await
                .unwrap();
        // Два ADTS-кадра без CRC.
        let mut data = Vec::new();
        for payload in [&b"aaa"[..], &b"bb"[..]] {
            let length = 7 + payload.len();
            data.extend_from_slice(&[
                0xFF,
                0xF1,
                0x50,
                0x80 | (((length >> 11) as u8) & 0x03),
                ((length >> 3) & 0xFF) as u8,
                (((length & 0x07) as u8) << 5) | 0x1F,
                0xFC,
            ]);
            data.extend_from_slice(payload);
        }
        assert_eq!(link.send_adts(&data).await.unwrap(), 2);
        assert_eq!(link.frames_sent(), 2);
        link.close().await.unwrap();
        assert!(!receiver.await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn link_reports_an_unreachable_data_port() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let error = BufferedLink::connect(
            addr,
            &setup(),
            0x1600_0000,
            0,
            0,
            Duration::from_millis(500),
        )
        .await
        .err()
        .expect("подключение к закрытому порту не должно удаться");
        assert!(error.to_string().contains("data-порт"), "{error}");
    }
}
