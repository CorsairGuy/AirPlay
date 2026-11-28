//! Зеркалирование экрана (тип 110): H.264/H.265 поверх TCP, SPS/PPS.
//!
//! Схема — как в UxPlay и shairplay-rust: ключ потока выводится из ключа
//! FairPlay (`ekey` из SETUP) для AES-CTR либо из HAP для ChaCha20-Poly1305;
//! каждый пакет содержит 128 байт заголовка и тело.
//! Видео отдаётся наружу в [`VideoSink`] без декодирования; декодер — в
//! `video`, показ зависит от железа и живёт в программе, которая
//! встраивает приёмник.

mod fanout;
mod receiver;

use sha2::{Digest, Sha512};

pub use fanout::FanOutSink;
pub use receiver::{HEADER_LEN, Header, PacketKind, receive, receive_authenticated};

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

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// R19: KDF посчитан независимо через Python hashlib.sha512, а не той же
    /// формулой из тестируемого кода. Ключ и IV для streamConnectionID 42.
    #[test]
    fn stream_key_and_iv_match_independent_sha512_vectors() {
        let (key, iv) = stream_key_iv(&[1; 16], 42);
        assert_eq!(hex(&key), "b42e3732de9be31a3845440d33ebf2e0");
        assert_eq!(hex(&iv), "05c598d26b031d812e8d16986d834d8d");
    }

    /// R19: NIST SP 800-38A F.5.1, AES-128-CTR, полный блок счётчика.
    #[test]
    fn aes128_ctr_matches_the_nist_vector() {
        let key: [u8; 16] = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let iv: [u8; 16] = [
            0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd,
            0xfe, 0xff,
        ];
        let plain = [
            0x6bu8, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a, 0xae, 0x2d, 0x8a, 0x57, 0x1e, 0x03, 0xac, 0x9c, 0x9e, 0xb7, 0x6f, 0xac,
            0x45, 0xaf, 0x8e, 0x51, 0x30, 0xc8, 0x1c, 0x46, 0xa3, 0x5c, 0xe4, 0x11, 0xe5, 0xfb,
            0xc1, 0x19, 0x1a, 0x0a, 0x52, 0xef, 0xf6, 0x9f, 0x24, 0x45, 0xdf, 0x4f, 0x9b, 0x17,
            0xad, 0x2b, 0x41, 0x7b, 0xe6, 0x6c, 0x37, 0x10,
        ];
        let expected = "874d6191b620e3261bef6864990db6ce\
9806f66b7970fdff8617187bb9fffdff\
5ae4df3edbd5d35e5b4f09020db03eab\
1e031dda2fbe03d1792170a0f3009cee";
        let mut data = plain;
        let mut cipher = crypto::ctr::aes128_ctr(&key, &iv);
        crypto::ctr::apply(&mut cipher, &mut data);
        assert_eq!(hex(&data), expected.replace('\n', ""));
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
