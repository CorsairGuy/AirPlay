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
    read_failed: bool,
    write_failed: bool,
}

impl Channel {
    pub fn new(keys: ControlKeys) -> Self {
        Self {
            keys,
            read_counter: 0,
            write_counter: 0,
            read_failed: false,
            write_failed: false,
        }
    }

    /// Шифрует данные и дописывает кадры в `out`.
    ///
    /// Операция атомарна: при исчерпании счётчика записей (2^64 кадров)
    /// возвращается [`Error::CounterExhausted`], `out` не меняется и ни один
    /// nonce не повторяется. После ошибки канал закрыт для записи.
    pub fn seal(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
        if self.write_failed {
            return Err(Error::CounterExhausted);
        }
        if data.is_empty() {
            return Ok(());
        }
        let frames = data.len().div_ceil(MAX_FRAME) as u64;
        // Последний допустимый nonce — u64::MAX; его используем, но следующий
        // вызов обязан отказать, а не обнулить счётчик.
        let Some(last) = self.write_counter.checked_add(frames - 1) else {
            self.write_failed = true;
            return Err(Error::CounterExhausted);
        };
        let mut rest = data;
        let mut counter = self.write_counter;
        while !rest.is_empty() {
            let (frame, tail) = rest.split_at(rest.len().min(MAX_FRAME));
            let len = (frame.len() as u16).to_le_bytes();
            let sealed = aead::seal(&self.keys.write, &counter.to_le_bytes(), &len, frame);
            if counter < last {
                counter += 1;
            }
            out.extend_from_slice(&len);
            out.extend_from_slice(&sealed);
            rest = tail;
        }
        self.write_counter = last.saturating_add(1);
        if last == u64::MAX {
            self.write_failed = true;
        }
        Ok(())
    }

    /// Расшифровывает все полные кадры из начала `buf`, удаляет их из буфера
    /// и дописывает открытый текст в `out`. Неполный кадр остаётся в `buf`.
    pub fn open(&mut self, buf: &mut Vec<u8>, out: &mut Vec<u8>) -> Result<(), Error> {
        if self.read_failed {
            return Err(Error::Aead);
        }
        let mut pos = 0;
        let mut counter = self.read_counter;
        let mut decoded = Vec::new();
        while buf.len() - pos >= 2 {
            // We send at most MAX_FRAME, but iOS sends longer frames on the MRP
            // data stream (seen 1087 and 1125 bytes); any u16 length is valid.
            let len = u16::from_le_bytes([buf[pos], buf[pos + 1]]) as usize;
            let end = pos + 2 + len + TAG_LEN;
            if buf.len() < end {
                break;
            }
            let aad = [buf[pos], buf[pos + 1]];
            let             Some(next_counter) = counter.checked_add(1) else {
                self.read_failed = true;
                return Err(Error::CounterExhausted);
            };
            let plain = match aead::open(
                &self.keys.read,
                &counter.to_le_bytes(),
                &aad,
                &buf[pos + 2..end],
            ) {
                Ok(plain) => plain,
                Err(error) => {
                    self.read_failed = true;
                    return Err(error);
                }
            };
            counter = next_counter;
            decoded.extend_from_slice(&plain);
            pos = end;
        }
        self.read_counter = counter;
        out.extend_from_slice(&decoded);
        buf.drain(..pos);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_exhaustion_is_atomic_and_terminal() {
        let shared = [3u8; 32];
        let mut sender = Channel::new(ControlKeys::controller(&shared));
        sender.write_counter = u64::MAX;
        let mut wire = Vec::new();
        // один кадр при счётчике MAX допустим
        sender.seal(b"a", &mut wire).unwrap();
        let mut second = Vec::new();
        assert!(matches!(
            sender.seal(b"b", &mut second),
            Err(Error::CounterExhausted)
        ));
        assert!(second.is_empty(), "частичная запись недопустима");
        // После ошибки запись закрыта, счётчик не обнуляется.
        assert!(sender.seal(b"c", &mut second).is_err());
        assert!(second.is_empty());
    }

    #[test]
    fn seal_of_empty_input_does_not_touch_the_counter() {
        let shared = [4u8; 32];
        let mut sender = Channel::new(ControlKeys::controller(&shared));
        sender.write_counter = u64::MAX;
        let mut wire = Vec::new();
        sender.seal(&[], &mut wire).unwrap();
        assert!(wire.is_empty());
        sender.seal(b"x", &mut wire).unwrap();
        assert!(!wire.is_empty());
    }

    #[test]
    fn roundtrip_across_frames_and_partial_reads() {
        let shared = [7u8; 32];
        let mut acc = Channel::new(ControlKeys::accessory(&shared));
        let mut ctl = Channel::new(ControlKeys::controller(&shared));

        let msg: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
        let mut wire = Vec::new();
        ctl.seal(&msg, &mut wire).unwrap();
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
        ctl.seal(b"GET /info RTSP/1.0\r\n\r\n", &mut wire).unwrap();
        wire[5] ^= 1;
        assert!(acc.open(&mut wire, &mut Vec::new()).is_err());
    }

    #[test]
    fn exhausted_read_counter_fails_without_wrapping() {
        let shared = [9u8; 32];
        let mut receiver = Channel::new(ControlKeys::accessory(&shared));
        let mut sender = Channel::new(ControlKeys::controller(&shared));
        receiver.read_counter = u64::MAX;
        sender.write_counter = u64::MAX;
        let len = [1u8, 0];
        let mut wire = len.to_vec();
        wire.extend_from_slice(&aead::seal(&sender.keys.write, &u64::MAX.to_le_bytes(), &len, b"x"));
        let original = wire.clone();
        let mut out = Vec::new();
        assert!(receiver.open(&mut wire, &mut out).is_err());
        assert!(out.is_empty());
        assert_eq!(wire, original);
        assert!(receiver.open(&mut wire, &mut out).is_err());
    }
}
