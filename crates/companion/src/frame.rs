//! Кадры Companion Link: заголовок 4 байта (вид + длина в 3 байта BE),
//! затем полезная нагрузка. После pair-verify нагрузка шифруется
//! ChaCha20-Poly1305: nonce — счётчик кадра в 12 байтах little-endian,
//! AAD — сам заголовок, длина в заголовке уже включает тег (16 байт).

use crypto::aead;

use crate::Error;

pub const HEADER: usize = 4;
const TAG: usize = 16;
/// Кадр больше этого считаем мусором.
const MAX_PAYLOAD: usize = 4 * 1024 * 1024;

/// Вид кадра. Числа совпадают с референсными значениями.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    Unknown,
    NoOp,
    PairSetupStart,
    PairSetupNext,
    PairVerifyStart,
    PairVerifyNext,
    Unencrypted,
    Encrypted,
    Plain,
    PairingRequest,
    PairingResponse,
    SessionStartRequest,
    SessionStartResponse,
    SessionData,
    FamilyIdentityRequest,
    FamilyIdentityResponse,
    FamilyIdentityUpdate,
}

impl FrameType {
    pub fn byte(self) -> u8 {
        match self {
            FrameType::Unknown => 0,
            FrameType::NoOp => 1,
            FrameType::PairSetupStart => 3,
            FrameType::PairSetupNext => 4,
            FrameType::PairVerifyStart => 5,
            FrameType::PairVerifyNext => 6,
            FrameType::Unencrypted => 7,
            FrameType::Encrypted => 8,
            FrameType::Plain => 9,
            FrameType::PairingRequest => 10,
            FrameType::PairingResponse => 11,
            FrameType::SessionStartRequest => 16,
            FrameType::SessionStartResponse => 17,
            FrameType::SessionData => 18,
            FrameType::FamilyIdentityRequest => 32,
            FrameType::FamilyIdentityResponse => 33,
            FrameType::FamilyIdentityUpdate => 34,
        }
    }

    pub fn from_byte(byte: u8) -> Self {
        match byte {
            0 => FrameType::Unknown,
            1 => FrameType::NoOp,
            3 => FrameType::PairSetupStart,
            4 => FrameType::PairSetupNext,
            5 => FrameType::PairVerifyStart,
            6 => FrameType::PairVerifyNext,
            7 => FrameType::Unencrypted,
            8 => FrameType::Encrypted,
            9 => FrameType::Plain,
            10 => FrameType::PairingRequest,
            11 => FrameType::PairingResponse,
            16 => FrameType::SessionStartRequest,
            17 => FrameType::SessionStartResponse,
            18 => FrameType::SessionData,
            32 => FrameType::FamilyIdentityRequest,
            33 => FrameType::FamilyIdentityResponse,
            34 => FrameType::FamilyIdentityUpdate,
            _ => FrameType::Unknown,
        }
    }

    /// Кадры, которые несут OPACK.
    pub fn is_opack(self) -> bool {
        matches!(
            self,
            FrameType::Unencrypted | FrameType::Encrypted | FrameType::Plain
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: FrameType,
    /// Исходный байт вида: неизвестные виды не теряются при разборе.
    pub raw_type: u8,
    pub payload: Vec<u8>,
}

/// Сборка/разбор кадров с необязательным шифрованием.
#[derive(Default)]
pub struct Codec {
    write_key: Option<[u8; 32]>,
    read_key: Option<[u8; 32]>,
    write_counter: u64,
    read_counter: u64,
    buffer: Vec<u8>,
    failed: bool,
}

impl Codec {
    pub fn plain() -> Self {
        Self::default()
    }

    pub fn encrypted(write: [u8; 32], read: [u8; 32]) -> Self {
        Self {
            write_key: Some(write),
            read_key: Some(read),
            ..Self::default()
        }
    }

    pub fn is_encrypted(&self) -> bool {
        self.write_key.is_some()
    }

    /// Собирает кадр; счётчик записи увеличивается только для шифрованных
    /// непустых нагрузок (пустые кадры идут открытым текстом).
    pub fn encode(&mut self, frame_type: FrameType, payload: &[u8]) -> Result<Vec<u8>, Error> {
        if self.failed { return Err(Error::Frame("канал закрыт после ошибки")); }
        if payload.len() > MAX_PAYLOAD {
            return Err(Error::Frame("слишком большой кадр"));
        }
        let encrypted = self.write_key.is_some() && !payload.is_empty();
        let length = if encrypted {
            payload.len() + TAG
        } else {
            payload.len()
        };
        let header = header(frame_type.byte(), length)?;
        let mut out = header.to_vec();
        match (&self.write_key, encrypted) {
            (Some(key), true) => {
                let mut nonce = [0u8; 8];
                nonce.copy_from_slice(&self.write_counter.to_le_bytes());
                self.write_counter = self.write_counter.checked_add(1).ok_or(Error::Frame("исчерпан счётчик записи"))?;
                out.extend_from_slice(&aead::seal(key, &nonce, &header, payload));
            }
            _ => out.extend_from_slice(payload),
        }
        Ok(out)
    }

    /// Добавляет принятые байты в буфер.
    pub fn feed(&mut self, data: &[u8]) -> Result<(), Error> {
        if self.failed { return Err(Error::Frame("канал закрыт после ошибки")); }
        if data.len() > MAX_PAYLOAD + TAG + HEADER || self.buffer.len().saturating_add(data.len()) > MAX_PAYLOAD + TAG + HEADER {
            self.failed = true;
            return Err(Error::Frame("буфер кадров переполнен"));
        }
        self.buffer.extend_from_slice(data);
        Ok(())
    }

    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Следующий целый кадр. `Ok(None)` — нужно больше байт.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, Error> {
        if self.failed { return Err(Error::Frame("канал закрыт после ошибки")); }
        if self.buffer.len() < HEADER {
            return Ok(None);
        }
        let raw_type = self.buffer[0];
        let length =
            u32::from_be_bytes([0, self.buffer[1], self.buffer[2], self.buffer[3]]) as usize;
        if length > MAX_PAYLOAD + TAG {
            return Err(Error::Frame("длина кадра вне допустимого"));
        }
        if self.buffer.len() < HEADER + length {
            return Ok(None);
        }
        let header: [u8; 4] = self.buffer[..HEADER].try_into().expect("длина проверена");
        let body: Vec<u8> = self.buffer[HEADER..HEADER + length].to_vec();
        self.buffer.drain(..HEADER + length);

        let payload = match (&self.read_key, body.is_empty()) {
            (Some(key), false) => {
                if body.len() < TAG {
                    return Err(Error::Frame("шифрованный кадр короче тега"));
                }
                let mut nonce = [0u8; 8];
                nonce.copy_from_slice(&self.read_counter.to_le_bytes());
                let next = self.read_counter.checked_add(1).ok_or(Error::Frame("исчерпан счётчик чтения"))?;
                match aead::open(key, &nonce, &header, &body) {
                    Ok(payload) => { self.read_counter = next; payload }
                    Err(error) => { self.failed = true; return Err(error.into()); }
                }
            }
            _ => body,
        };
        Ok(Some(Frame {
            frame_type: FrameType::from_byte(raw_type),
            raw_type,
            payload,
        }))
    }
}

fn header(frame_type: u8, length: usize) -> Result<[u8; 4], Error> {
    let length = u32::try_from(length).map_err(|_| Error::Frame("длина не влезает в 3 байта"))?;
    if length > 0xFF_FFFF {
        return Err(Error::Frame("длина не влезает в 3 байта"));
    }
    let bytes = length.to_be_bytes();
    Ok([frame_type, bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout() {
        let mut codec = Codec::plain();
        let frame = codec.encode(FrameType::Encrypted, b"ab").unwrap();
        assert_eq!(frame, vec![8, 0, 0, 2, b'a', b'b']);
    }

    #[test]
    fn plain_frames_split_at_arbitrary_boundaries() {
        let mut codec = Codec::plain();
        let mut wire = Vec::new();
        wire.extend(codec.encode(FrameType::Encrypted, b"first").unwrap());
        wire.extend(codec.encode(FrameType::NoOp, b"").unwrap());
        wire.extend(codec.encode(FrameType::Encrypted, b"second").unwrap());
        // 4+5, 4+0, 4+6
        assert_eq!(wire.len(), 9 + 4 + 10);

        let mut decoder = Codec::plain();
        let mut frames = Vec::new();
        for chunk in wire.chunks(3) {
            decoder.feed(chunk).unwrap();
            while let Some(frame) = decoder.next_frame().unwrap() {
                frames.push((frame.frame_type, frame.payload));
            }
        }
        assert_eq!(
            frames,
            vec![
                (FrameType::Encrypted, b"first".to_vec()),
                (FrameType::NoOp, Vec::new()),
                (FrameType::Encrypted, b"second".to_vec()),
            ]
        );
    }

    #[test]
    fn encryption_roundtrip_uses_counters_and_header_aad() {
        let mut sender = Codec::encrypted([1; 32], [2; 32]);
        let mut receiver = Codec::encrypted([2; 32], [1; 32]);
        let mut wire = Vec::new();
        wire.extend(sender.encode(FrameType::Encrypted, b"hello").unwrap());
        wire.extend(sender.encode(FrameType::NoOp, b"").unwrap());
        wire.extend(sender.encode(FrameType::Encrypted, b"world").unwrap());
        // Длина в заголовке включает тег.
        assert_eq!(&wire[..4], &[8, 0, 0, 21]);

        receiver.feed(&wire).unwrap();
        let first = receiver.next_frame().unwrap().unwrap();
        assert_eq!(first.payload, b"hello");
        let second = receiver.next_frame().unwrap().unwrap();
        assert_eq!(second.frame_type, FrameType::NoOp);
        let third = receiver.next_frame().unwrap().unwrap();
        assert_eq!(third.payload, b"world");
        assert!(receiver.next_frame().unwrap().is_none());
    }

    #[test]
    fn tampered_frame_is_rejected() {
        let mut sender = Codec::encrypted([3; 32], [4; 32]);
        let mut receiver = Codec::encrypted([4; 32], [3; 32]);
        let mut wire = sender.encode(FrameType::Encrypted, b"payload").unwrap();
        let last = wire.len() - 1;
        wire[last] ^= 1;
        receiver.feed(&wire).unwrap();
        assert!(receiver.next_frame().is_err());
        assert!(receiver.feed(&[]).is_err());
    }

    #[test]
    fn counters_do_not_wrap_and_input_buffer_is_bounded() {
        let mut writer = Codec::encrypted([1; 32], [2; 32]);
        writer.write_counter = u64::MAX;
        assert!(writer.encode(FrameType::Encrypted, b"x").is_err());
        let mut reader = Codec::plain();
        assert!(reader.feed(&vec![0; MAX_PAYLOAD + TAG + HEADER + 1]).is_err());
        assert!(reader.next_frame().is_err());
    }

    #[test]
    fn unknown_frame_type_keeps_raw_byte() {
        let mut codec = Codec::plain();
        codec.feed(&[0xFE, 0, 0, 1, 0x08]).unwrap();
        let frame = codec.next_frame().unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::Unknown);
        assert_eq!(frame.raw_type, 0xFE);
    }

    #[test]
    fn oversized_length_is_rejected_without_buffering() {
        let mut codec = Codec::plain();
        codec.feed(&[8, 0xFF, 0xFF, 0xFF]).unwrap();
        assert!(codec.next_frame().is_err());
    }
}
