//! Protobuf без кодогенерации: сообщение — список полей `(номер, значение)`.
//! Номера берутся из [`crate::schema`], [`Node::validate`] сверяет типы полей
//! со схемой. Так не нужны ни protoc, ни сгенерированные структуры на сотни
//! сообщений, из которых приёмнику нужен десяток.

use crate::Error;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(u64),
    Fixed64(u64),
    Bytes(Vec<u8>),
    Fixed32(u32),
}

impl Value {
    fn wire_type(&self) -> u8 {
        match self {
            Value::Int(_) => 0,
            Value::Fixed64(_) => 1,
            Value::Bytes(_) => 2,
            Value::Fixed32(_) => 5,
        }
    }
}

/// Сообщение protobuf. Повторяющееся поле — несколько записей с одним номером.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Node(pub Vec<(u32, Value)>);

/// Больше полей в одном сообщении не бывает: защита от мусора.
const MAX_FIELDS: usize = 8192;
/// Больше сообщений в одном пакете не бывает.
const MAX_MESSAGES: usize = 128;
const MAX_FIELD: usize = 4 * 1024 * 1024;

fn varint(data: &[u8], at: &mut usize) -> Result<u64, Error> {
    let mut n = 0u64;
    for shift in (0..70).step_by(7) {
        let b = *data.get(*at).ok_or(Error::Protobuf("короткий varint"))?;
        *at += 1;
        if shift == 63 && b > 1 {
            return Err(Error::Protobuf("varint больше 64 бит"));
        }
        n |= ((b & 127) as u64) << shift;
        if b < 128 {
            return Ok(n);
        }
    }
    Err(Error::Protobuf("длинный varint"))
}

fn put_varint(mut n: u64, out: &mut Vec<u8>) {
    while n > 127 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}

impl Node {
    pub fn parse(data: &[u8]) -> Result<Self, Error> {
        let mut at = 0;
        let mut fields = Vec::new();
        while at < data.len() {
            if fields.len() > MAX_FIELDS {
                return Err(Error::Protobuf("слишком много полей"));
            }
            let tag = varint(data, &mut at)?;
            let id = (tag >> 3) as u32;
            if id == 0 {
                return Err(Error::Protobuf("поле с номером 0"));
            }
            let value = match tag & 7 {
                0 => Value::Int(varint(data, &mut at)?),
                1 => {
                    let b = data
                        .get(at..at + 8)
                        .ok_or(Error::Protobuf("короткое fixed64"))?;
                    at += 8;
                    Value::Fixed64(u64::from_le_bytes(b.try_into().unwrap()))
                }
                2 => {
                    let len = varint(data, &mut at)? as usize;
                    if len > MAX_FIELD || len > data.len() - at {
                        return Err(Error::Protobuf("неверная длина поля"));
                    }
                    let value = data[at..at + len].to_vec();
                    at += len;
                    Value::Bytes(value)
                }
                5 => {
                    let b = data
                        .get(at..at + 4)
                        .ok_or(Error::Protobuf("короткое fixed32"))?;
                    at += 4;
                    Value::Fixed32(u32::from_le_bytes(b.try_into().unwrap()))
                }
                _ => return Err(Error::Protobuf("неизвестный тип поля")),
            };
            fields.push((id, value));
        }
        Ok(Self(fields))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_into(&mut out);
        out
    }

    fn encode_into(&self, out: &mut Vec<u8>) {
        for (id, value) in &self.0 {
            put_varint(u64::from(*id) * 8 + u64::from(value.wire_type()), out);
            match value {
                Value::Int(n) => put_varint(*n, out),
                Value::Fixed64(n) => out.extend(n.to_le_bytes()),
                Value::Fixed32(n) => out.extend(n.to_le_bytes()),
                Value::Bytes(b) => {
                    put_varint(b.len() as u64, out);
                    out.extend(b);
                }
            }
        }
    }

    /// Сверяет типы полей со схемой, вложенные сообщения — тоже.
    pub fn validate(&self, message: &str) -> Result<(), Error> {
        for (id, value) in &self.0 {
            let (expected, nested) = crate::schema::wire_type(message, *id)
                .ok_or_else(|| Error::Schema(format!("нет поля {message}.{id}")))?;
            if value.wire_type() != expected {
                return Err(Error::Schema(format!(
                    "{message}.{id}: тип {} вместо {expected}",
                    value.wire_type()
                )));
            }
            if let (false, Value::Bytes(raw)) = (nested.is_empty(), value) {
                Node::parse(raw)?.validate(nested)?;
            }
        }
        Ok(())
    }

    pub fn add(&mut self, field: u32, value: Value) -> &mut Self {
        self.0.push((field, value));
        self
    }

    pub fn int(&mut self, field: u32, value: u64) -> &mut Self {
        self.add(field, Value::Int(value))
    }

    pub fn boolean(&mut self, field: u32, value: bool) -> &mut Self {
        self.int(field, value as u64)
    }

    pub fn text(&mut self, field: u32, value: impl AsRef<str>) -> &mut Self {
        self.bytes(field, value.as_ref().as_bytes())
    }

    pub fn bytes(&mut self, field: u32, value: impl AsRef<[u8]>) -> &mut Self {
        self.add(field, Value::Bytes(value.as_ref().to_vec()))
    }

    pub fn node(&mut self, field: u32, value: &Node) -> &mut Self {
        self.add(field, Value::Bytes(value.encode()))
    }

    pub fn float(&mut self, field: u32, value: f32) -> &mut Self {
        self.add(field, Value::Fixed32(value.to_bits()))
    }

    pub fn double(&mut self, field: u32, value: f64) -> &mut Self {
        self.add(field, Value::Fixed64(value.to_bits()))
    }

    /// Убрать все записи поля.
    pub fn remove(&mut self, field: u32) -> &mut Self {
        self.0.retain(|(id, _)| *id != field);
        self
    }

    /// Последняя запись поля, как у protobuf для неповторяющихся полей.
    pub fn get(&self, field: u32) -> Option<&Value> {
        self.0
            .iter()
            .rev()
            .find(|(id, _)| *id == field)
            .map(|(_, v)| v)
    }

    pub fn number(&self, field: u32) -> Option<u64> {
        match self.get(field) {
            Some(Value::Int(n)) => Some(*n),
            _ => None,
        }
    }

    pub fn raw(&self, field: u32) -> Option<&[u8]> {
        match self.get(field) {
            Some(Value::Bytes(v)) => Some(v),
            _ => None,
        }
    }

    pub fn string(&self, field: u32) -> Option<&str> {
        self.raw(field).and_then(|v| std::str::from_utf8(v).ok())
    }

    /// Все строки повторяющегося поля.
    pub fn strings(&self, field: u32) -> impl Iterator<Item = &str> {
        self.0.iter().filter_map(move |(id, v)| match v {
            Value::Bytes(b) if *id == field => std::str::from_utf8(b).ok(),
            _ => None,
        })
    }

    /// Вложенное сообщение; нет поля — пустое.
    pub fn nested(&self, field: u32) -> Result<Node, Error> {
        Self::parse(self.raw(field).unwrap_or_default())
    }

    pub fn real(&self, field: u32) -> Option<f64> {
        match self.get(field) {
            Some(Value::Fixed32(n)) => Some(f32::from_bits(*n).into()),
            Some(Value::Fixed64(n)) => Some(f64::from_bits(*n)),
            _ => None,
        }
    }
}

/// Пакет канала данных: сообщения подряд, перед каждым — длина varint.
pub fn encode_packet(messages: &[Node]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut raw = Vec::new();
    for message in messages {
        raw.clear();
        message.encode_into(&mut raw);
        put_varint(raw.len() as u64, &mut out);
        out.extend_from_slice(&raw);
    }
    out
}

pub fn decode_packet(mut data: &[u8]) -> Result<Vec<Node>, Error> {
    let mut out = Vec::new();
    while !data.is_empty() {
        if out.len() > MAX_MESSAGES {
            return Err(Error::Protobuf("слишком много сообщений"));
        }
        // Байт 0x08 — тег поля `type`: одно сообщение без длины впереди,
        // так шлют некоторые версии iOS.
        if data[0] == 8 {
            out.push(Node::parse(data)?);
            break;
        }
        let mut at = 0;
        let len = varint(data, &mut at)? as usize;
        if len == 0 || len > data.len() - at {
            return Err(Error::Protobuf("неверная длина сообщения"));
        }
        out.push(Node::parse(&data[at..at + len])?);
        data = &data[at + len..];
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut inner = Node::default();
        inner.text(1, "Песня").double(2, 12.5).float(3, 0.5);
        let mut n = Node::default();
        n.int(1, 300)
            .boolean(2, true)
            .node(3, &inner)
            .int(4, u64::MAX);
        let parsed = Node::parse(&n.encode()).unwrap();
        assert_eq!(parsed, n);
        let inner = parsed.nested(3).unwrap();
        assert_eq!(inner.string(1), Some("Песня"));
        assert_eq!(inner.real(2), Some(12.5));
        assert_eq!(inner.real(3), Some(0.5));
        assert_eq!(parsed.number(4), Some(u64::MAX));
    }

    #[test]
    fn packets() {
        let mut a = Node::default();
        a.int(1, 15);
        let mut b = Node::default();
        b.int(1, 4).text(2, "id");
        let packet = encode_packet(&[a.clone(), b.clone()]);
        assert_eq!(decode_packet(&packet).unwrap(), vec![a.clone(), b]);
        // Без длины впереди.
        assert_eq!(decode_packet(&a.encode()).unwrap(), vec![a]);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(Node::parse(&[0x0a, 0xff, 0xff, 0x7f]).is_err());
        assert!(Node::parse(&[0x00]).is_err());
        assert!(Node::parse(&[0x0b]).is_err());
        assert!(Node::parse(&[0x80; 12]).is_err());
        assert!(decode_packet(&[0x05, 0x08]).is_err());
    }
}
