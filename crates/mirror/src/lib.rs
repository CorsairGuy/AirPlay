//! Зеркалирование экрана (тип 110): H.264/H.265 поверх TCP, AES-CTR, SPS/PPS.
//!
//! Схема — как в UxPlay и shairplay-rust: ключ потока выводится из ключа
//! FairPlay (`ekey` из SETUP), каждый пакет — 128 байт заголовка и тело.
//! Видео отдаётся наружу в [`VideoSink`] без декодирования; декодер — в
//! `ap_video`, показ зависит от железа и живёт в программе, которая
//! встраивает приёмник.

mod receiver;

use sha2::{Digest, Sha512};

pub use receiver::{HEADER_LEN, Header, PacketKind, receive};

use std::fmt;
use std::sync::Arc;

use clock::TimeSource;

/// Что известно о видеопотоке при его открытии.
#[derive(Clone)]
pub struct StreamInfo {
    /// Часы отправителя (NTP, шкала [`clock::NTP_TIMELINE`]): по ним
    /// `pts_ns` кадров переводится в локальное время [`clock::now_ns`].
    pub time: Arc<dyn TimeSource>,
}

/// Создаёт [`VideoSink`] на каждый новый видеопоток.
#[derive(Clone)]
pub struct VideoSinkFactory(Arc<OpenFn>);

type OpenFn = dyn Fn(&StreamInfo) -> Box<dyn VideoSink> + Send + Sync;

impl VideoSinkFactory {
    pub fn new(f: impl Fn(&StreamInfo) -> Box<dyn VideoSink> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn open(&self, info: &StreamInfo) -> Box<dyn VideoSink> {
        (self.0)(info)
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
    /// Параметры декодера как прислал отправитель: запись `avcC` (H.264)
    /// или описание `hvc1` с `hvcC` внутри (H.265); [`parameter_sets`]
    /// достаёт из них SPS/PPS. Приходит в начале и при смене ориентации или
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

/// Наборы параметров (VPS, SPS, PPS) из пакета настроек в виде Annex B:
/// годятся и в начало дампа, и в `extradata` декодера.
pub fn parameter_sets(codec: Codec, record: &[u8]) -> Option<Vec<u8>> {
    match codec {
        Codec::H264 => avc_config_to_annex_b(record),
        Codec::H265 => hevc_config_to_annex_b(hevc_record(record)?),
    }
}

/// SPS и PPS из записи `avcC` в виде Annex B.
pub fn avc_config_to_annex_b(record: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut pos = 5;
    let sps_count = *record.get(pos)? & 0x1F;
    pos += 1;
    for _ in 0..sps_count {
        take_nal(record, &mut pos, &mut out)?;
    }
    let pps_count = *record.get(pos)?;
    pos += 1;
    for _ in 0..pps_count {
        take_nal(record, &mut pos, &mut out)?;
    }
    Some(out)
}

/// NAL с 2-байтовой длиной из записи настроек → стартовый код и NAL.
fn take_nal(record: &[u8], pos: &mut usize, out: &mut Vec<u8>) -> Option<()> {
    let len = u16::from_be_bytes([*record.get(*pos)?, *record.get(*pos + 1)?]) as usize;
    *pos += 2;
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(record.get(*pos..*pos + len)?);
    *pos += len;
    Some(())
}

/// Запись `hvcC`. iOS присылает описание сэмпла `hvc1` (как в `stsd`
/// QuickTime), а запись лежит в его вложенном блоке `hvcC`; голую запись
/// (версия 1 в первом байте) принимаем как есть.
fn hevc_record(payload: &[u8]) -> Option<&[u8]> {
    match payload.windows(4).position(|w| w == b"hvcC") {
        Some(tag) if tag >= 4 => {
            let size = u32::from_be_bytes(payload[tag - 4..tag].try_into().unwrap()) as usize;
            payload.get(tag + 4..(tag - 4).checked_add(size)?)
        }
        _ => (payload.first() == Some(&1)).then_some(payload),
    }
}

/// VPS, SPS и PPS из записи `hvcC` в виде Annex B.
pub fn hevc_config_to_annex_b(record: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let arrays = *record.get(22)?;
    let mut pos = 23;
    for _ in 0..arrays {
        // Тип NAL в младших 6 битах; дальше число NAL в массиве.
        pos += 1;
        let count = u16::from_be_bytes([*record.get(pos)?, *record.get(pos + 1)?]);
        pos += 2;
        for _ in 0..count {
            take_nal(record, &mut pos, &mut out)?;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Пишет поток H.264 или H.265 в Annex B (файл, канал в ffplay или gst).
/// Для проверки и отладки: `ffplay -f h264 файл` (`-f hevc` для H.265).
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
        match parameter_sets(codec, record) {
            Some(ps) => {
                let _ = self.out.write_all(&ps);
            }
            None => tracing::warn!(?codec, "не разобрать параметры видео"),
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

    #[test]
    fn hevc_parameter_sets_from_sample_description() {
        // hvcC: 22 байта общих полей и 2 массива, VPS и SPS, по одному NAL
        // с 2-байтовой длиной.
        let mut record = vec![1u8];
        record.extend_from_slice(&[0; 21]);
        record.extend_from_slice(&[2, 0xA0, 0, 1, 0, 2, 0x40, 0x01, 0xA1, 0, 1, 0, 1, 0x42]);
        let expected = [0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x42];
        assert_eq!(parameter_sets(Codec::H265, &record).unwrap(), expected);

        // Как шлёт iOS: описание сэмпла `hvc1`, запись — в блоке `hvcC`.
        let mut entry = vec![0, 0, 0, 0];
        entry.extend_from_slice(b"hvc1");
        entry.extend_from_slice(&[0; 78]);
        entry.extend_from_slice(&((record.len() + 8) as u32).to_be_bytes());
        entry.extend_from_slice(b"hvcC");
        entry.extend_from_slice(&record);
        entry.extend_from_slice(&[0, 0, 0, 8, b'c', b'o', b'l', b'r']);
        assert_eq!(parameter_sets(Codec::H265, &entry).unwrap(), expected);

        assert_eq!(parameter_sets(Codec::H265, &entry[..40]), None);
    }
}
