//! Отправка buffered-аудио (тип 103).
//!
//! Канал — TCP: `[длина:u16BE][пакет RTP:длина−2]`, длина включает сами два
//! байта (`rtp::frame_buffered`). Внутри — обычный пакет AirPlay 2:
//! 12-байтовый RTP-заголовок, шифротекст, тег и 8-байтовый nonce
//! (`rtp::AudioCipher`). Такой поток принимает наш же приёмник
//! (`audio_stream` → `Stream::buffered`), поэтому формат не выдуман: он
//! проверен на разборе собственного RX и по §C-RTP/§C-NET wire-spec.
//!
//! Кодек здесь не кодируется. `BufferedSender` принимает уже
//! закодированные кадры AAC-LC; разбор ADTS-файла (`adts_frames`) даёт их
//! из `.aac` без внешнего кодировщика. AAC-энкод из PCM в этот модуль не
//! входит: libavcodec-энкодер не входит в offline-сборку.
//!
//! Аппаратной проверки нет: поля SETUP для type 103 спецификация называет
//! «sender-dependent», и набор ниже — минимальный, который принимает наш RX.

use anyhow::{Context, Result, bail};
use rtp::{AudioCipher, Header};
use tokio::io::{AsyncWrite, AsyncWriteExt};

/// AAC-LC: 1024 сэмпла на кадр.
pub const AAC_FRAMES_PER_PACKET: u32 = 1024;
/// Тип полезной нагрузки RTP для AAC в AirPlay 2.
pub const AAC_PAYLOAD_TYPE: u8 = 0x60;
/// `ct` для AAC-LC в SETUP.
pub const COMPRESSION_AAC_LC: u64 = 4;
/// Тип потока buffered audio.
pub const STREAM_TYPE_BUFFERED: u64 = 103;

/// Отправитель buffered-потока поверх любого `AsyncWrite` (в бою — TCP).
pub struct BufferedSender<W> {
    out: W,
    cipher: AudioCipher,
    ssrc: u32,
    seq: u16,
    timestamp: u32,
    nonce: u64,
    failed: bool,
}

impl<W: AsyncWrite + Unpin> BufferedSender<W> {
    /// `seq`/`timestamp` — начальные значения; `ssrc` для AAC-профиля
    /// (например `0x1600_0000` — AAC 44.1 кГц стерео).
    pub fn new(out: W, shk: &[u8; 32], ssrc: u32, seq: u16, timestamp: u32) -> Self {
        Self {
            out,
            cipher: AudioCipher::new(shk),
            ssrc,
            seq,
            timestamp,
            nonce: 0,
            failed: false,
        }
    }

    /// Метка времени следующего кадра.
    pub fn next_timestamp(&self) -> u32 {
        self.timestamp
    }

    pub fn next_sequence(&self) -> u16 {
        self.seq
    }

    /// Отправляет один уже закодированный кадр AAC-LC.
    pub async fn send_frame(&mut self, frame: &[u8]) -> Result<()> {
        if self.failed {
            bail!("buffered stream is unusable after an interrupted write");
        }
        if frame.is_empty() {
            bail!("пустой кадр AAC");
        }
        if frame.len() > u16::MAX as usize - 2 - 12 - 16 - 8 {
            bail!("пакет buffered-аудио больше 65533 байт");
        }
        let header = Header {
            marker: false,
            payload_type: AAC_PAYLOAD_TYPE,
            seq: self.seq,
            timestamp: self.timestamp,
            ssrc: self.ssrc,
        };
        let next_nonce = self
            .nonce
            .checked_add(1)
            .context("buffered audio nonce exhausted")?;
        let packet = self.cipher.seal(&header, frame, self.nonce);
        // Длина в префиксе — u16 вместе с двумя байтами длины.
        if packet.len() + 2 > u16::MAX as usize {
            bail!("пакет buffered-аудио больше 65533 байт");
        }
        let mut wire = Vec::with_capacity(packet.len() + 2);
        rtp::frame_buffered(&packet, &mut wire);
        // Once I/O starts, an error or cancellation can leave a partial frame
        // on the wire. Never allow this keyed stream to be reused in that case.
        self.failed = true;
        self.out
            .write_all(&wire)
            .await
            .context("отправка buffered-аудио")?;
        self.out.flush().await?;
        self.seq = self.seq.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(AAC_FRAMES_PER_PACKET);
        self.nonce = next_nonce;
        self.failed = false;
        Ok(())
    }

    /// Отправляет пачку кадров по порядку.
    pub async fn send_frames(&mut self, frames: &[&[u8]]) -> Result<()> {
        for frame in frames {
            self.send_frame(frame).await?;
        }
        Ok(())
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

/// Параметры стрима для SETUP (type 103).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamSetup {
    /// Порт управляющего канала, который открыл отправитель.
    pub control_port: u16,
    /// `shk` — ключ аудиопотока (32 байта).
    pub shk: [u8; 32],
    /// `streamConnectionID` — идентификатор соединения.
    pub stream_connection_id: u64,
    pub sample_rate: u32,
    pub frames_per_packet: u32,
}

impl StreamSetup {
    pub fn aac_44100(control_port: u16, shk: [u8; 32], stream_connection_id: u64) -> Self {
        Self {
            control_port,
            shk,
            stream_connection_id,
            sample_rate: 44_100,
            frames_per_packet: AAC_FRAMES_PER_PACKET,
        }
    }

    /// Словарь стрима. Набор полей минимальный: `type`, `ct`, `shk`,
    /// `isMedia`, `sr`, `spf`, `streamConnectionID`, `supportsDynamicStreamID`.
    /// Wire-spec называет состав полей для 103 зависящим от отправителя,
    /// поэтому расширять его без захвата нельзя.
    pub fn plist(&self) -> plist::Dictionary {
        let mut d = plist::Dictionary::new();
        d.insert(
            "type".into(),
            plist::Value::Integer(STREAM_TYPE_BUFFERED.into()),
        );
        d.insert(
            "ct".into(),
            plist::Value::Integer(COMPRESSION_AAC_LC.into()),
        );
        d.insert("isMedia".into(), plist::Value::Boolean(true));
        d.insert("shk".into(), plist::Value::Data(self.shk.to_vec()));
        d.insert(
            "sr".into(),
            plist::Value::Integer((self.sample_rate as u64).into()),
        );
        d.insert(
            "spf".into(),
            plist::Value::Integer((self.frames_per_packet as u64).into()),
        );
        d.insert(
            "controlPort".into(),
            plist::Value::Integer((self.control_port as u64).into()),
        );
        d.insert(
            "streamConnectionID".into(),
            plist::Value::Integer(self.stream_connection_id.into()),
        );
        d.insert(
            "supportsDynamicStreamID".into(),
            plist::Value::Boolean(false),
        );
        d
    }

    /// Тело SETUP: `{streams: [<словарь стрима>]}`.
    pub fn setup_plist(&self) -> plist::Dictionary {
        let mut d = plist::Dictionary::new();
        d.insert(
            "streams".into(),
            plist::Value::Array(vec![plist::Value::Dictionary(self.plist())]),
        );
        d
    }
}

/// Ошибка разбора ADTS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdtsError {
    /// Поток кончился посреди заголовка или кадра.
    Truncated,
    /// Нет синхроса `0xFFF`.
    BadSync,
    /// Длина кадра меньше заголовка.
    BadLength,
    /// The sender's SETUP is limited to one AAC-LC, 44.1 kHz stereo block.
    UnsupportedConfiguration,
}

/// Режет ADTS-поток на полезные нагрузки AAC (без заголовков ADTS).
///
/// Заголовок — 7 байт, с CRC — 9. Длина кадра — 13 бит в заголовке и
/// включает сам заголовок. Пустой вход — пустой список.
pub fn adts_frames(data: &[u8]) -> Result<Vec<&[u8]>, AdtsError> {
    let mut frames = Vec::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let head = data.get(pos..pos + 7).ok_or(AdtsError::Truncated)?;
        if head[0] != 0xFF || head[1] & 0xF0 != 0xF0 {
            return Err(AdtsError::BadSync);
        }
        let profile = head[2] >> 6;
        let sample_rate_index = (head[2] >> 2) & 0x0f;
        let channels = ((head[2] & 1) << 2) | (head[3] >> 6);
        if head[1] & 0x06 != 0
            || profile != 1
            || sample_rate_index != 4
            || channels != 2
            || head[6] & 3 != 0
        {
            return Err(AdtsError::UnsupportedConfiguration);
        }
        let protection_absent = head[1] & 0x01 != 0;
        let header_len = if protection_absent { 7 } else { 9 };
        let length = (((head[3] & 0x03) as usize) << 11)
            | ((head[4] as usize) << 3)
            | ((head[5] as usize) >> 5);
        if length < header_len {
            return Err(AdtsError::BadLength);
        }
        let frame = data.get(pos..pos + length).ok_or(AdtsError::Truncated)?;
        frames.push(&frame[header_len..]);
        pos += length;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn cancelled_partial_write_makes_stream_unusable() {
        let (writer, mut reader) = tokio::io::duplex(1);
        let mut sender = BufferedSender::new(writer, &[7; 32], 1, 0, 0);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(30),
                sender.send_frame(b"test")
            )
            .await
            .is_err()
        );
        let mut first = [0u8; 1];
        reader.read_exact(&mut first).await.unwrap();
        assert!(sender.send_frame(b"retry").await.is_err());
    }

    fn adts_frame(payload: &[u8], crc: bool) -> Vec<u8> {
        let header = if crc { 9 } else { 7 };
        let length = header + payload.len();
        let mut frame = vec![0u8; header];
        frame[0] = 0xFF;
        frame[1] = 0xF0 | if crc { 0 } else { 1 };
        frame[2] = 0x50;
        frame[3] = 0x80 | (((length >> 11) as u8) & 0x03);
        frame[4] = ((length >> 3) & 0xFF) as u8;
        frame[5] = (((length & 0x07) as u8) << 5) | 0x1F;
        frame[6] = 0xFC;
        frame.extend_from_slice(payload);
        frame
    }

    #[test]
    fn adts_splits_payloads() {
        let mut data = adts_frame(b"first", false);
        data.extend(adts_frame(b"second", false));
        data.extend(adts_frame(b"crc", true));
        assert_eq!(
            adts_frames(&data).unwrap(),
            vec![&b"first"[..], &b"second"[..], &b"crc"[..]]
        );
        assert!(adts_frames(&[]).unwrap().is_empty());
    }

    #[test]
    fn adts_rejects_broken_input() {
        assert_eq!(adts_frames(&[0xFF, 0xF1, 0x40]), Err(AdtsError::Truncated));
        assert_eq!(adts_frames(&[0x00; 8]), Err(AdtsError::BadSync));

        let mut data = adts_frame(b"x", false);
        data.truncate(data.len() - 1);
        assert_eq!(adts_frames(&data), Err(AdtsError::Truncated));

        // Длина меньше заголовка.
        let mut bad = adts_frame(b"x", false);
        bad[3] = 0x80;
        bad[4] = 0;
        bad[5] = 0;
        assert_eq!(adts_frames(&bad), Err(AdtsError::BadLength));
    }

    #[test]
    fn adts_rejects_profiles_not_matching_setup() {
        let valid = adts_frame(b"x", false);
        for (index, replacement) in [(2, 0x4c), (3, 0x40), (6, 0xfd)] {
            let mut invalid = valid.clone();
            invalid[index] = replacement;
            assert_eq!(
                adts_frames(&invalid),
                Err(AdtsError::UnsupportedConfiguration)
            );
        }
    }

    #[test]
    fn setup_plist_has_the_minimal_buffered_fields() {
        let setup = StreamSetup::aac_44100(7001, [9; 32], 0x0102_0304);
        let d = setup.plist();
        assert_eq!(
            d.get("type").and_then(plist::Value::as_unsigned_integer),
            Some(103)
        );
        assert_eq!(
            d.get("ct").and_then(plist::Value::as_unsigned_integer),
            Some(4)
        );
        assert_eq!(
            d.get("isMedia").and_then(plist::Value::as_boolean),
            Some(true)
        );
        assert_eq!(
            d.get("shk").and_then(plist::Value::as_data),
            Some(&[9u8; 32][..])
        );
        assert_eq!(
            d.get("spf").and_then(plist::Value::as_unsigned_integer),
            Some(1024)
        );
        assert_eq!(
            d.get("controlPort")
                .and_then(plist::Value::as_unsigned_integer),
            Some(7001)
        );
        let Value::Array(streams) = setup
            .setup_plist()
            .get("streams")
            .cloned()
            .expect("streams")
        else {
            panic!("ожидался массив streams");
        };
        assert_eq!(streams.len(), 1);
    }

    use plist::Value;

    async fn wire_frames(frames: Vec<Vec<u8>>) -> (Vec<u8>, [u8; 32], u16, u32) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let shk = [7u8; 32];
        let seq = 1000;
        let timestamp = 352_000;
        let writer = tokio::spawn(async move {
            let stream = TcpStream::connect(address).await.unwrap();
            let mut sender = BufferedSender::new(stream, &shk, 0x1600_0000, seq, timestamp);
            let borrowed: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
            sender.send_frames(&borrowed).await.unwrap();
            sender.into_inner().shutdown().await.unwrap();
        });
        let (mut server, _) = listener.accept().await.unwrap();
        let mut wire = Vec::new();
        server.read_to_end(&mut wire).await.unwrap();
        writer.await.unwrap();
        (wire, shk, seq, timestamp)
    }

    #[tokio::test]
    async fn frames_are_framed_encrypted_and_timestamped() {
        let (wire, shk, seq, timestamp) =
            wire_frames(vec![b"aac-one".to_vec(), b"aac-two".to_vec()]).await;
        let cipher = AudioCipher::new(&shk);
        let mut buf = wire;
        let mut seen = Vec::new();
        while let Some(mut packet) = rtp::split_buffered(&mut buf) {
            let (header, payload) = cipher.open(&mut packet).expect("пакет расшифрован");
            seen.push((header.seq, header.timestamp, header.ssrc, payload.to_vec()));
        }
        assert!(buf.is_empty());
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], (seq, timestamp, 0x1600_0000, b"aac-one".to_vec()));
        assert_eq!(
            seen[1],
            (
                seq.wrapping_add(1),
                timestamp.wrapping_add(AAC_FRAMES_PER_PACKET),
                0x1600_0000,
                b"aac-two".to_vec()
            )
        );
    }

    #[tokio::test]
    async fn empty_and_oversized_frames_are_refused() {
        let (client, _server) = tokio::io::duplex(64);
        let mut sender = BufferedSender::new(client, &[1; 32], 1, 0, 0);
        assert!(sender.send_frame(b"").await.is_err());
        let huge = vec![0u8; u16::MAX as usize];
        assert!(sender.send_frame(&huge).await.is_err());
        assert_eq!(sender.next_sequence(), 0, "отказ не двигает счётчики");
        assert_eq!(sender.next_timestamp(), 0);
    }

    #[tokio::test]
    async fn sequence_and_timestamp_wrap() {
        let (client, _server) = tokio::io::duplex(4096);
        let mut sender = BufferedSender::new(client, &[2; 32], 1, u16::MAX, u32::MAX - 10);
        sender.send_frame(b"x").await.unwrap();
        assert_eq!(sender.next_sequence(), 0);
        assert_eq!(sender.next_timestamp(), AAC_FRAMES_PER_PACKET - 11);
    }

    /// Одинаковые кадры шифруются разными nonce; счётчик идёт 0,1,2, а
    /// шифротекст не совпадает с открытым текстом.
    #[tokio::test]
    async fn nonce_increments_and_identical_frames_differ() {
        let (wire, shk, _, _) = wire_frames(vec![b"same".to_vec(); 3]).await;
        let cipher = AudioCipher::new(&shk);
        let mut buf = wire;
        let mut nonces = Vec::new();
        let mut bodies = Vec::new();
        while let Some(mut packet) = rtp::split_buffered(&mut buf) {
            let nonce = u64::from_le_bytes(packet[packet.len() - 8..].try_into().unwrap());
            nonces.push(nonce);
            bodies.push(packet[12..packet.len() - 24].to_vec());
            let (_, payload) = cipher.open(&mut packet).expect("пакет расшифрован");
            assert_eq!(payload, b"same");
        }
        assert_eq!(nonces, [0, 1, 2]);
        assert_ne!(bodies[0], bodies[1]);
        assert_ne!(bodies[1], bodies[2]);
        assert!(bodies.iter().all(|body| body != b"same"));
    }

    /// Отказ на пустом или слишком большом кадре не отравляет поток:
    /// следующий корректный кадр уходит с прежними счётчиками.
    #[tokio::test]
    async fn rejected_frame_does_not_poison_the_stream() {
        let (client, _server) = tokio::io::duplex(4096);
        let mut sender = BufferedSender::new(client, &[3; 32], 1, 0, 0);
        assert!(sender.send_frame(b"").await.is_err());
        assert!(sender.send_frame(&vec![0u8; u16::MAX as usize]).await.is_err());
        assert_eq!(sender.next_sequence(), 0);
        sender.send_frame(b"ok").await.unwrap();
        assert_eq!(sender.next_sequence(), 1);
        assert_eq!(sender.next_timestamp(), AAC_FRAMES_PER_PACKET);
    }

}
