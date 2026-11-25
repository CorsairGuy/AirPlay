//! Минимальный клиент Companion Link без сети: собирает кадры, копит
//! принятые байты и разбирает OPACK. Транспорт (TCP) подключает вызывающий —
//! так же, как это делают `services/remote` и тесты.

use crate::Error;
use crate::commands;
use crate::frame::{Codec, Frame, FrameType};
use crate::opack::{self, Value};

pub struct Link {
    codec: Codec,
    next_xid: u32,
}

impl Default for Link {
    fn default() -> Self {
        Self::plain()
    }
}

impl Link {
    pub fn plain() -> Self {
        Self {
            codec: Codec::plain(),
            next_xid: 1,
        }
    }

    /// `write`/`read` — ключи из [`crate::auth::PairVerify::keys`].
    pub fn encrypted(write: [u8; 32], read: [u8; 32]) -> Self {
        Self {
            codec: Codec::encrypted(write, read),
            next_xid: 1,
        }
    }

    pub fn is_encrypted(&self) -> bool {
        self.codec.is_encrypted()
    }

    /// Следующий идентификатор обмена.
    pub fn next_xid(&mut self) -> u32 {
        let xid = self.next_xid;
        self.next_xid = self.next_xid.wrapping_add(1);
        xid
    }

    /// Собирает OPACK-кадр.
    pub fn send(&mut self, frame_type: FrameType, value: &Value) -> Result<Vec<u8>, Error> {
        let payload = opack::encode(value)?;
        self.codec.encode(frame_type, &payload)
    }

    /// Запрос `_i`/`_c` с новым `_x` и кадром `E_OPACK`.
    pub fn request(&mut self, identifier: &str, content: Value) -> Result<(u32, Vec<u8>), Error> {
        let xid = self.next_xid();
        let value = commands::request(identifier, content, xid);
        Ok((xid, self.send(FrameType::Encrypted, &value)?))
    }

    /// Событие касания `_hidT` с новым `_x`: `_t = 1`, но не запрос, поэтому
    /// ответа не ждём. `ns` — наносекунды с начала соединения.
    pub fn touch(
        &mut self,
        x: i64,
        y: i64,
        phase: crate::commands::TouchPhase,
        ns: u64,
    ) -> Result<(u32, Vec<u8>), Error> {
        let xid = self.next_xid();
        let value = commands::touch(x, y, phase, ns, xid);
        Ok((xid, self.send(FrameType::Encrypted, &value)?))
    }

    pub fn feed(&mut self, data: &[u8]) {
        self.codec.feed(data);
    }

    pub fn poll(&mut self) -> Result<Option<Frame>, Error> {
        self.codec.next_frame()
    }

    /// Разбирает OPACK из кадра; для не-OPACK кадров — ошибка.
    pub fn payload(frame: &Frame) -> Result<Value, Error> {
        if !frame.frame_type.is_opack() {
            return Err(Error::Frame("кадр не несёт OPACK"));
        }
        opack::unpack(&frame.payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{HidCommand, hid};

    #[test]
    fn request_is_one_encrypted_frame_with_matching_xid() {
        let mut link = Link::plain();
        let (xid, wire) = link
            .request(
                "_hidC",
                Value::object(vec![
                    ("_hBtS", Value::Int(1)),
                    ("_hidC", Value::Int(HidCommand::Menu as u8 as i64)),
                ]),
            )
            .unwrap();
        assert_eq!(wire[0], FrameType::Encrypted.byte());
        let length = u32::from_be_bytes([0, wire[1], wire[2], wire[3]]) as usize;
        assert_eq!(length, wire.len() - 4);

        link.feed(&wire);
        let frame = link.poll().unwrap().unwrap();
        let value = Link::payload(&frame).unwrap();
        assert_eq!(value.get("_x").and_then(Value::as_int), Some(xid as i64));
        assert_eq!(value.get("_i").and_then(Value::as_str), Some("_hidC"));
        assert!(link.poll().unwrap().is_none());
    }

    #[test]
    fn touch_goes_out_as_one_encrypted_event_frame() {
        use crate::commands::TouchPhase;
        let mut link = Link::plain();
        let (xid, wire) = link.touch(500, 250, TouchPhase::Press, 42).unwrap();
        assert_eq!(wire[0], FrameType::Encrypted.byte());
        let length = u32::from_be_bytes([0, wire[1], wire[2], wire[3]]) as usize;
        assert_eq!(length, wire.len() - 4);
        link.feed(&wire);
        let frame = link.poll().unwrap().unwrap();
        let value = Link::payload(&frame).unwrap();
        assert_eq!(value.get("_x").and_then(Value::as_int), Some(xid as i64));
        assert_eq!(value.get("_i").and_then(Value::as_str), Some("_hidT"));
        assert_eq!(
            value.get("_t").and_then(Value::as_int),
            Some(commands::EVENT)
        );
        assert!(link.poll().unwrap().is_none());
    }

    #[test]
    fn encrypted_link_roundtrips_through_two_instances() {
        let mut sender = Link::encrypted([1; 32], [2; 32]);
        let mut receiver = Link::encrypted([2; 32], [1; 32]);
        let (_, wire) = sender
            .request(
                "_sessionStart",
                Value::object(vec![("_srvT", Value::Str("x".into()))]),
            )
            .unwrap();
        receiver.feed(&wire);
        let frame = receiver.poll().unwrap().unwrap();
        let value = Link::payload(&frame).unwrap();
        assert_eq!(
            value.get("_i").and_then(Value::as_str),
            Some("_sessionStart")
        );
        assert!(sender.is_encrypted());
    }

    #[test]
    fn ids_increase_and_skip_zero() {
        let mut link = Link::plain();
        let first = link.next_xid();
        let second = link.next_xid();
        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(
            hid(true, HidCommand::Home, first)
                .get("_x")
                .and_then(Value::as_int),
            Some(1)
        );
    }

    #[test]
    fn non_opack_frames_are_not_decoded_as_opack() {
        let frame = Frame {
            frame_type: FrameType::SessionData,
            raw_type: FrameType::SessionData.byte(),
            payload: vec![0x08],
        };
        assert!(Link::payload(&frame).is_err());
    }
}
