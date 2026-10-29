//! RTP для аудиопотоков AirPlay: заголовок, расшифровка ChaCha20-Poly1305
//! (AirPlay 2) и кадрирование буферизованного потока по TCP.

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce, Tag};

pub const HEADER_LEN: usize = 12;
const TAG_LEN: usize = 16;
const NONCE_LEN: usize = 8;

/// Фиксированный заголовок RTP (RFC 3550), без CSRC и расширений —
/// AirPlay их не использует.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub marker: bool,
    pub payload_type: u8,
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
}

impl Header {
    pub fn parse(packet: &[u8]) -> Option<Header> {
        let h = packet.get(..HEADER_LEN)?;
        if h[0] >> 6 != 2 {
            return None;
        }
        Some(Header {
            marker: h[1] & 0x80 != 0,
            payload_type: h[1] & 0x7f,
            seq: u16::from_be_bytes([h[2], h[3]]),
            timestamp: u32::from_be_bytes([h[4], h[5], h[6], h[7]]),
            ssrc: u32::from_be_bytes([h[8], h[9], h[10], h[11]]),
        })
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0] = 0x80;
        h[1] = self.payload_type | if self.marker { 0x80 } else { 0 };
        h[2..4].copy_from_slice(&self.seq.to_be_bytes());
        h[4..8].copy_from_slice(&self.timestamp.to_be_bytes());
        h[8..12].copy_from_slice(&self.ssrc.to_be_bytes());
        h
    }
}

/// Шифр аудиопакетов AirPlay 2 с ключом `shk` из SETUP.
///
/// Пакет: `[заголовок 12][шифротекст][тег 16][nonce 8]`. Nonce AEAD — 4 нуля
/// и 8 байт из хвоста, AAD — байты заголовка 4..12 (время и SSRC).
pub struct AudioCipher(ChaCha20Poly1305);

impl AudioCipher {
    pub fn new(shk: &[u8; 32]) -> Self {
        Self(ChaCha20Poly1305::new(shk.into()))
    }

    /// Расшифровывает пакет на месте и возвращает заголовок и полезную нагрузку.
    pub fn open<'a>(&self, packet: &'a mut [u8]) -> Option<(Header, &'a [u8])> {
        let header = Header::parse(packet)?;
        let len = packet.len().checked_sub(HEADER_LEN + TAG_LEN + NONCE_LEN)?;
        let (head, rest) = packet.split_at_mut(HEADER_LEN);
        let (body, trailer) = rest.split_at_mut(len);
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&trailer[TAG_LEN..]);
        let tag = Tag::clone_from_slice(&trailer[..TAG_LEN]);
        self.0
            .decrypt_in_place_detached(Nonce::from_slice(&nonce), &head[4..12], body, &tag)
            .ok()?;
        Some((header, body))
    }

    /// Шифрует пакет так же, как отправитель (для тестов и `airplay send`).
    pub fn seal(&self, header: &Header, payload: &[u8], nonce: u64) -> Vec<u8> {
        let head = header.encode();
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[4..].copy_from_slice(&nonce.to_le_bytes());
        let mut body = payload.to_vec();
        let tag = self
            .0
            .encrypt_in_place_detached(Nonce::from_slice(&nonce_bytes), &head[4..12], &mut body)
            .expect("размер пакета RTP в пределах ChaCha20");
        let mut out = Vec::with_capacity(HEADER_LEN + body.len() + TAG_LEN + NONCE_LEN);
        out.extend_from_slice(&head);
        out.extend_from_slice(&body);
        out.extend_from_slice(&tag);
        out.extend_from_slice(&nonce_bytes[4..]);
        out
    }
}

/// Буферизованный поток (тип 103) идёт по TCP: каждый пакет предварён
/// двумя байтами длины (big endian), длина включает сами эти два байта.
pub fn split_buffered(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    if buf.len() < 2 {
        return None;
    }
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    if len < 2 {
        // Испорченный поток: выбрасываем два байта, чтобы не зациклиться.
        buf.drain(..2);
        return None;
    }
    if buf.len() < len {
        return None;
    }
    let packet = buf[2..len].to_vec();
    buf.drain(..len);
    Some(packet)
}

pub fn frame_buffered(packet: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&((packet.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(packet);
}

/// Разница между двумя метками времени RTP с учётом переполнения.
pub fn ts_diff(a: u32, b: u32) -> i32 {
    a.wrapping_sub(b) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header {
            marker: false,
            payload_type: 0x60,
            seq: 7,
            timestamp: 352_000,
            ssrc: 0x1600_0000,
        }
    }

    #[test]
    fn header_roundtrip() {
        let h = header();
        assert_eq!(Header::parse(&h.encode()), Some(h));
        assert_eq!(Header::parse(&[0u8; 12]), None, "версия RTP не 2");
    }

    #[test]
    fn cipher_roundtrip_and_tamper() {
        let c = AudioCipher::new(&[3u8; 32]);
        let mut pkt = c.seal(&header(), b"aac frame", 42);
        let (h, body) = c.open(&mut pkt).unwrap();
        assert_eq!(h, header());
        assert_eq!(body, b"aac frame");

        let mut pkt = c.seal(&header(), b"aac frame", 42);
        pkt[5] ^= 1; // время входит в AAD
        assert!(c.open(&mut pkt).is_none());
        assert!(c.open(&mut [0x80u8; 20]).is_none());
    }

    #[test]
    fn buffered_framing() {
        let mut wire = Vec::new();
        frame_buffered(b"one", &mut wire);
        frame_buffered(b"second", &mut wire);
        let mut partial = wire[..4].to_vec();
        assert_eq!(split_buffered(&mut partial), None);
        assert_eq!(split_buffered(&mut wire).as_deref(), Some(&b"one"[..]));
        assert_eq!(split_buffered(&mut wire).as_deref(), Some(&b"second"[..]));
        assert!(wire.is_empty());
    }

    #[test]
    fn wrapping_diff() {
        assert_eq!(ts_diff(5, u32::MAX - 4), 10);
        assert_eq!(ts_diff(0, 352), -352);
    }
}
