//! Кадры канала данных MediaRemote (поток 130).
//!
//! Заголовок 32 байта: длина кадра вместе с заголовком (u32 BE), вид
//! (`sync` — сообщения, `rply` — подтверждение), 8 нулей, команда (`cmnd`
//! у `sync`), номер (u64 BE, байты 20..28), 4 нуля. Тело `sync` — бинарный
//! plist `{params: {data: <пакет protobuf>}}`. На каждый `sync` отвечают
//! пустым `rply` с тем же номером.

use plist::{Dictionary, Value};

use crate::Error;
use crate::wire::{self, Node};

pub const HEADER: usize = 32;
/// Кадр больше этого — мусор.
const MAX_FRAME: usize = 4 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub enum Frame {
    /// Сообщения от другой стороны; ответить [`reply`] с этим `seq`.
    Sync { seq: u64, messages: Vec<Node> },
    /// Подтверждение нашего кадра.
    Reply { seq: u64 },
    /// Кадр другого вида или `sync` без сообщений.
    Other { seq: u64 },
}

/// Собирает кадры из потока байт.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Следующий кадр. `Ok(None)` — нужно больше байт. [`Error::FrameSize`]
    /// — поток сломан; другая ошибка — кадр с битым телом уже вынут, можно
    /// читать дальше.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, Error> {
        if self.buf.len() < HEADER {
            return Ok(None);
        }
        let size = u32::from_be_bytes(self.buf[..4].try_into().unwrap()) as usize;
        if !(HEADER..=MAX_FRAME).contains(&size) {
            return Err(Error::FrameSize);
        }
        if self.buf.len() < size {
            return Ok(None);
        }
        let frame: Vec<u8> = self.buf.drain(..size).collect();
        let seq = u64::from_be_bytes(frame[20..28].try_into().unwrap());
        Ok(Some(match &frame[4..8] {
            b"rply" => Frame::Reply { seq },
            b"sync" if size > HEADER => Frame::Sync {
                seq,
                messages: decode_body(&frame[HEADER..])?,
            },
            _ => Frame::Other { seq },
        }))
    }
}

fn decode_body(body: &[u8]) -> Result<Vec<Node>, Error> {
    let value = Value::from_reader(std::io::Cursor::new(body)).map_err(|e| {
        Error::Frame(if e.is_eof() {
            "обрезанный plist"
        } else {
            "не plist"
        })
    })?;
    let data = value
        .as_dictionary()
        .and_then(|d| d.get("params"))
        .and_then(Value::as_dictionary)
        .and_then(|d| d.get("data"))
        .and_then(Value::as_data);
    match data {
        Some(data) => wire::decode_packet(data),
        None => Ok(Vec::new()),
    }
}

fn header(out: &mut Vec<u8>, kind: &[u8; 4], seq: u64, body: usize) {
    out.extend(((HEADER + body) as u32).to_be_bytes());
    out.extend(kind);
    out.extend([0u8; 8]);
    out.extend(if kind == b"sync" {
        b"cmnd"
    } else {
        b"\0\0\0\0"
    });
    out.extend(seq.to_be_bytes());
    out.extend([0u8; 4]);
}

/// Подтверждение кадра `seq`.
pub fn reply(seq: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER);
    header(&mut out, b"rply", seq, 0);
    out
}

/// Кадр `sync` с сообщениями.
pub fn sync(seq: u64, messages: &[Node]) -> Vec<u8> {
    let mut params = Dictionary::new();
    params.insert("data".into(), Value::Data(wire::encode_packet(messages)));
    let mut root = Dictionary::new();
    root.insert("params".into(), params.into());
    let mut body = Vec::new();
    Value::Dictionary(root)
        .to_writer_binary(&mut body)
        .expect("запись plist в память не может завершиться ошибкой");
    let mut out = Vec::with_capacity(HEADER + body.len());
    header(&mut out, b"sync", seq, body.len());
    out.extend_from_slice(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip_in_pieces() {
        let mut m = Node::default();
        m.int(1, 15).text(2, "запрос");
        let mut stream = sync(7, std::slice::from_ref(&m));
        stream.extend(reply(7));
        assert_eq!(&stream[20..28], &7u64.to_be_bytes());
        assert_eq!(reply(1).len(), HEADER);

        let mut d = Decoder::new();
        let mut got = Vec::new();
        for chunk in stream.chunks(5) {
            d.push(chunk);
            while let Some(f) = d.next_frame().unwrap() {
                got.push(f);
            }
        }
        assert_eq!(
            got,
            vec![
                Frame::Sync {
                    seq: 7,
                    messages: vec![m]
                },
                Frame::Reply { seq: 7 }
            ]
        );
    }

    #[test]
    fn bad_frames() {
        let mut d = Decoder::new();
        d.push(&[0, 0, 0, 4]);
        d.push(&[0; 28]);
        assert!(d.next_frame().is_err());

        // Битый plist: ошибка, но кадр вынут, следующий читается.
        let mut d = Decoder::new();
        let mut f = sync(1, &[]);
        let n = f.len();
        f[n - 3] ^= 0xff;
        f[HEADER] = b'x';
        d.push(&f);
        d.push(&reply(2));
        assert!(d.next_frame().is_err());
        assert_eq!(d.next_frame().unwrap(), Some(Frame::Reply { seq: 2 }));
    }
}
