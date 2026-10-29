//! Шифрованный канал HAP поверх TCP: кадры до 1024 байт,
//! 2 байта длины (LE) служат AAD, nonce — счётчик кадров.

use crate::Error;
use crate::aead::{self, TAG_LEN};
use crate::kdf::ControlKeys;

pub const MAX_FRAME: usize = 1024;

pub struct Channel {
    keys: ControlKeys,
    read_counter: u64,
    write_counter: u64,
}

impl Channel {
    pub fn new(keys: ControlKeys) -> Self {
        Self {
            keys,
            read_counter: 0,
            write_counter: 0,
        }
    }

    /// Шифрует данные и дописывает кадры в `out`.
    pub fn seal(&mut self, mut data: &[u8], out: &mut Vec<u8>) {
        while !data.is_empty() {
            let (frame, rest) = data.split_at(data.len().min(MAX_FRAME));
            let len = (frame.len() as u16).to_le_bytes();
            let sealed = aead::seal(
                &self.keys.write,
                &self.write_counter.to_le_bytes(),
                &len,
                frame,
            );
            self.write_counter += 1;
            out.extend_from_slice(&len);
            out.extend_from_slice(&sealed);
            data = rest;
        }
    }

    /// Расшифровывает все полные кадры из начала `buf`, удаляет их из буфера
    /// и дописывает открытый текст в `out`. Неполный кадр остаётся в `buf`.
    pub fn open(&mut self, buf: &mut Vec<u8>, out: &mut Vec<u8>) -> Result<(), Error> {
        let mut pos = 0;
        while buf.len() - pos >= 2 {
            let len = u16::from_le_bytes([buf[pos], buf[pos + 1]]) as usize;
            if len > MAX_FRAME {
                return Err(Error::FrameTooLong(len));
            }
            let end = pos + 2 + len + TAG_LEN;
            if buf.len() < end {
                break;
            }
            let aad = [buf[pos], buf[pos + 1]];
            let plain = aead::open(
                &self.keys.read,
                &self.read_counter.to_le_bytes(),
                &aad,
                &buf[pos + 2..end],
            )?;
            self.read_counter += 1;
            out.extend_from_slice(&plain);
            pos = end;
        }
        buf.drain(..pos);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_across_frames_and_partial_reads() {
        let shared = [7u8; 32];
        let mut acc = Channel::new(ControlKeys::accessory(&shared));
        let mut ctl = Channel::new(ControlKeys::controller(&shared));

        let msg: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
        let mut wire = Vec::new();
        ctl.seal(&msg, &mut wire);
        assert_eq!(wire.len(), msg.len() + 3 * (2 + TAG_LEN));

        // Подаём по кускам, как из сокета.
        let mut buf = Vec::new();
        let mut plain = Vec::new();
        for chunk in wire.chunks(100) {
            buf.extend_from_slice(chunk);
            acc.open(&mut buf, &mut plain).unwrap();
        }
        assert!(buf.is_empty());
        assert_eq!(plain, msg);
    }

    #[test]
    fn tampered_frame_is_rejected() {
        let shared = [1u8; 32];
        let mut acc = Channel::new(ControlKeys::accessory(&shared));
        let mut ctl = Channel::new(ControlKeys::controller(&shared));
        let mut wire = Vec::new();
        ctl.seal(b"GET /info RTSP/1.0\r\n\r\n", &mut wire);
        wire[5] ^= 1;
        assert!(acc.open(&mut wire, &mut Vec::new()).is_err());
    }
}
