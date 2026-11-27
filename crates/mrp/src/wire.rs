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
            if fields.len() >= MAX_FIELDS {
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
        self.validate_inner(message, false, 0)
    }

    fn validate_inner(
        &self,
        message: &str,
        allow_unknown: bool,
        depth: usize,
    ) -> Result<(), Error> {
        if depth > 64 {
            return Err(Error::Protobuf("слишком глубокая вложенность"));
        }
        for (id, value) in &self.0 {
            let Some((expected, nested)) = crate::schema::wire_type(message, *id) else {
                if allow_unknown {
                    continue;
                }
                return Err(Error::Schema(format!("нет поля {message}.{id}")));
            };
            if value.wire_type() != expected {
                if allow_unknown {
                    continue;
                }
                return Err(Error::Schema(format!(
                    "{message}.{id}: тип {} вместо {expected}",
                    value.wire_type()
                )));
            }
            if let (false, Value::Bytes(raw)) = (nested.is_empty(), value) {
                Node::parse(raw)?.validate_inner(nested, allow_unknown, depth + 1)?;
            }
        }
        Ok(())
    }

    /// Как [`Node::validate`], но для **входящих** сообщений: неизвестное
    /// поле или несовпадение типа пропускается, а не отклоняет сообщение.
    /// Клиент вправе прислать поле из новой версии — по §6 плана v12 это не
    /// повод рвать сессию; protobuf-парсеры так же считают такое поле
    /// неизвестным.
    pub fn validate_known(&self, message: &str) -> Result<(), Error> {
        self.validate_inner(message, true, 0)
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
        if out.len() >= MAX_MESSAGES {
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

    /// Неизвестные поля сохраняются при разборе и перекодировании, но
    /// строгая исходящая проверка их не пропускает; входящая — пропускает.
    #[test]
    fn unknown_fields_are_preserved_and_tolerated_inbound() {
        let mut node = Node::default();
        node.int(1, 1)
            .int(2, 0)
            .int(9_999, 7)
            .bytes(10_000, b"future");
        let parsed = Node::parse(&node.encode()).unwrap();
        assert_eq!(
            parsed, node,
            "порядок и неизвестные поля должны сохраниться"
        );
        assert!(parsed.validate_known("SetDiscoveryModeMessage").is_ok());
        assert!(
            parsed.validate("SetDiscoveryModeMessage").is_err(),
            "исходящее сообщение с полем вне схемы"
        );

        let mut wrong = Node::default();
        wrong.text(1, "not a varint");
        assert!(wrong.validate_known("SetDiscoveryModeMessage").is_ok());
        assert!(wrong.validate("SetDiscoveryModeMessage").is_err());
    }

    /// Порядок полей, повторяющиеся значения и fixed32/fixed64 переживают
    /// разбор без перестановки.
    #[test]
    fn field_order_and_wire_types_roundtrip() {
        let mut node = Node::default();
        node.int(5, 1)
            .int(3, 2)
            .int(5, 3)
            .text(7, "a")
            .text(7, "b")
            .float(8, 0.5)
            .double(9, -12.25);
        let parsed = Node::parse(&node.encode()).unwrap();
        assert_eq!(parsed, node);
        assert_eq!(parsed.number(5), Some(3), "последнее значение поля");
        assert_eq!(parsed.strings(7).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(parsed.real(8), Some(0.5));
        assert_eq!(parsed.real(9), Some(-12.25));
    }

    /// Пределы полей и сообщений включают ровно объявленный максимум.
    #[test]
    fn field_and_message_limits_are_exact() {
        let field = [0x08, 0x00];
        let mut at_limit = Vec::with_capacity(MAX_FIELDS * 2);
        for _ in 0..MAX_FIELDS {
            at_limit.extend_from_slice(&field);
        }
        assert_eq!(Node::parse(&at_limit).unwrap().0.len(), MAX_FIELDS);
        at_limit.extend_from_slice(&field);
        assert!(Node::parse(&at_limit).is_err(), "MAX_FIELDS + 1 поле");

        let mut message = Node::default();
        message.int(1, 0);
        let packet = encode_packet(&vec![message.clone(); MAX_MESSAGES]);
        assert_eq!(decode_packet(&packet).unwrap().len(), MAX_MESSAGES);
        let packet = encode_packet(&vec![message; MAX_MESSAGES + 1]);
        assert!(
            decode_packet(&packet).is_err(),
            "MAX_MESSAGES + 1 сообщение"
        );
    }

    #[test]
    fn empty_and_overlong_packets_are_rejected() {
        assert!(decode_packet(&[0x00]).is_err(), "нулевая длина сообщения");
        assert!(decode_packet(&[0x02, 0x08]).is_err(), "длина больше данных");
        assert!(decode_packet(&[0x01, 0x08, 0x08]).is_err());
    }
}
