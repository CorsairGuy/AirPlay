//! Зеркалирование экрана (тип 110): H.264/H.265 поверх TCP, AES-CTR, SPS/PPS.
//!
//! Схема — как в UxPlay и shairplay-rust: ключ потока выводится из ключа
//! FairPlay (`ekey` из SETUP), каждый пакет — 128 байт заголовка и тело.
//! Видео отдаётся наружу в [`VideoSink`] без декодирования: декодер и показ
//! зависят от железа и живут в программе, которая встраивает приёмник.

mod receiver;

use sha2::{Digest, Sha512};

pub use receiver::{HEADER_LEN, Header, PacketKind, receive};

use std::fmt;
use std::sync::Arc;

/// Создаёт [`VideoSink`] на каждый новый видеопоток.
#[derive(Clone)]
pub struct VideoSinkFactory(Arc<OpenFn>);

type OpenFn = dyn Fn() -> Box<dyn VideoSink> + Send + Sync;

impl VideoSinkFactory {
    pub fn new(f: impl Fn() -> Box<dyn VideoSink> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn open(&self) -> Box<dyn VideoSink> {
        (self.0)()
    }
}

impl fmt::Debug for VideoSinkFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VideoSinkFactory")
    }
}

/// Только считает кадры: когда показывать некуда.
pub struct CountingSink(pub u64);

impl VideoSink for CountingSink {
    fn config(&mut self, _codec: Codec, _record: &[u8]) {}
    fn frame(&mut self, _pts_ns: u64, _nals: &[u8]) {
        self.0 += 1;
    }
}

fn sha512_16(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = Sha512::new();
    for p in parts {
        h.update(p);
    }
    h.finalize()[..16]
        .try_into()
        .expect("SHA-512 длиннее 16 байт")
}

/// Ключ AES сессии: расшифрованный FairPlay `ekey`, а если отправитель
/// прошёл legacy pair-verify, ещё и смешанный с его общим секретом.
pub fn session_key(fairplay_key: &[u8; 16], pair_verify_shared: Option<&[u8; 32]>) -> [u8; 16] {
    match pair_verify_shared {
        Some(shared) => sha512_16(&[fairplay_key, shared]),
        None => *fairplay_key,
    }
}

/// Ключ и IV AES-CTR для потока `streamConnectionID` из SETUP.
pub fn stream_key_iv(session_key: &[u8; 16], connection_id: u64) -> ([u8; 16], [u8; 16]) {
    let key = sha512_16(&[
        format!("AirPlayStreamKey{connection_id}").as_bytes(),
        session_key,
    ]);
    let iv = sha512_16(&[
        format!("AirPlayStreamIV{connection_id}").as_bytes(),
        session_key,
    ]);
    (key, iv)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    H265,
}

/// Куда идёт видео. Вызовы приходят из задачи приёма, по порядку.
pub trait VideoSink: Send {
    /// Параметры декодера: запись `avcC` (H.264) или `hvcC` (H.265) как
    /// прислал отправитель. Приходит в начале и при смене ориентации или
    /// разрешения.
    fn config(&mut self, codec: Codec, record: &[u8]);
    /// Кадр: NAL-блоки с 4-байтовой длиной (формат AVCC), уже расшифрованные.
    /// `pts_ns` — время показа по часам отправителя (NTP), нс.
    fn frame(&mut self, pts_ns: u64, nals: &[u8]);
    /// Поток закончился.
    fn end(&mut self) {}
}

/// NAL-блоки AVCC → Annex B (стартовые коды вместо длин), на месте:
/// длина и стартовый код занимают по 4 байта.
pub fn avcc_to_annex_b(nals: &mut [u8]) {
    let mut pos = 0;
    while pos + 4 <= nals.len() {
        let len = u32::from_be_bytes(nals[pos..pos + 4].try_into().unwrap()) as usize;
        nals[pos..pos + 4].copy_from_slice(&[0, 0, 0, 1]);
        pos += 4 + len;
    }
}

/// SPS и PPS из записи `avcC` в виде Annex B.
pub fn avc_config_to_annex_b(record: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 5;
    let sps_count = *record.get(pos)? & 0x1F;
    pos += 1;
    let take = |pos: &mut usize, out: &mut Vec<u8>| -> Option<()> {
        let len = u16::from_be_bytes([*record.get(*pos)?, *record.get(*pos + 1)?]) as usize;
        *pos += 2;
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(record.get(*pos..*pos + len)?);
        *pos += len;
        Some(())
    };
    for _ in 0..sps_count {
        take(&mut pos, &mut out)?;
    }
    let pps_count = *record.get(pos)?;
    pos += 1;
    for _ in 0..pps_count {
        take(&mut pos, &mut out)?;
    }
    Some(out)
}

/// Пишет поток H.264 в Annex B (файл, канал в ffplay или gst). Для проверки
/// и отладки: `ffplay -f h264 файл`.
pub struct AnnexBWriter<W: std::io::Write + Send> {
    out: W,
}

impl<W: std::io::Write + Send> AnnexBWriter<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: std::io::Write + Send> VideoSink for AnnexBWriter<W> {
    fn config(&mut self, codec: Codec, record: &[u8]) {
        if codec != Codec::H264 {
            tracing::warn!("H.265 в Annex B пока не пишется");
            return;
        }
        if let Some(ps) = avc_config_to_annex_b(record) {
            let _ = self.out.write_all(&ps);
        }
    }

    fn frame(&mut self, _pts_ns: u64, nals: &[u8]) {
        let mut data = nals.to_vec();
        avcc_to_annex_b(&mut data);
        let _ = self.out.write_all(&data);
    }

    fn end(&mut self) {
        let _ = self.out.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_key_matches_formula() {
        let (key, iv) = stream_key_iv(&[1; 16], 42);
        assert_eq!(key, sha512_16(&[b"AirPlayStreamKey42", &[1; 16]]));
        assert_eq!(iv, sha512_16(&[b"AirPlayStreamIV42", &[1; 16]]));
        assert_ne!(key, iv);
        assert_eq!(session_key(&[3; 16], None), [3; 16]);
        assert_ne!(session_key(&[3; 16], Some(&[4; 32])), [3; 16]);
    }

    #[test]
    fn annex_b_conversion() {
        let mut nals = vec![0, 0, 0, 2, 0x65, 0xAA, 0, 0, 0, 1, 0x41];
        avcc_to_annex_b(&mut nals);
        assert_eq!(nals, [0, 0, 0, 1, 0x65, 0xAA, 0, 0, 0, 1, 0x41]);

        // avcC: версия, профиль, совместимость, уровень, 0xFF, 1 SPS, 1 PPS.
        let record = [
            1, 0x64, 0, 0x28, 0xFF, 0xE1, 0, 2, 0x67, 0x64, 1, 0, 1, 0x68,
        ];
        assert_eq!(
            avc_config_to_annex_b(&record).unwrap(),
            [0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68]
        );
    }
}
