use anyhow::{bail, Result};
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(u64),
    Fixed64(u64),
    Bytes(Vec<u8>),
    Fixed32(u32),
}
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Node(pub Vec<(u32, Value)>);
fn varint(data: &[u8], at: &mut usize) -> Result<u64> {
    let mut n = 0u64;
    for shift in (0..70).step_by(7) {
        let b = *data
            .get(*at)
            .ok_or_else(|| anyhow::anyhow!("short varint"))?;
        *at += 1;
        if shift == 63 && b > 1 {
            bail!("varint overflow")
        };
        n |= ((b & 127) as u64) << shift;
        if b < 128 {
            return Ok(n);
        }
    }
    bail!("varint length")
}
fn put_var(mut n: u64, out: &mut Vec<u8>) {
    while n > 127 {
        out.push((n as u8 & 127) | 128);
        n >>= 7
    }
    out.push(n as u8)
}
impl Node {
    pub fn validate(&self, message: &str) -> Result<()> {
        for (id, value) in &self.0 {
            let (expected, nested) = crate::mrp_service::schema::wire_type(message, *id)
                .ok_or_else(|| anyhow::anyhow!("unknown field {message}.{id}"))?;
            let actual = match value {
                Value::Int(_) => 0,
                Value::Fixed64(_) => 1,
                Value::Bytes(_) => 2,
                Value::Fixed32(_) => 5,
            };
            if actual != expected {
                bail!("wire type mismatch {message}.{id}: {actual} != {expected}")
            };
            if !nested.is_empty() {
                if let Value::Bytes(raw) = value {
                    Node::parse(raw)?.validate(nested)?;
                }
            }
        }
        Ok(())
    }
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut at = 0;
        let mut fields = Vec::new();
        while at < data.len() {
            if fields.len() > 8192 {
                bail!("too many fields")
            };
            let tag = varint(data, &mut at)?;
            let id = (tag >> 3) as u32;
            if id == 0 {
                bail!("zero field")
            };
            let value = match tag & 7 {
                0 => Value::Int(varint(data, &mut at)?),
                1 => {
                    let bytes = data
                        .get(at..at + 8)
                        .ok_or_else(|| anyhow::anyhow!("short fixed64"))?;
                    at += 8;
                    Value::Fixed64(u64::from_le_bytes(bytes.try_into()?))
                }
                2 => {
                    let len = varint(data, &mut at)? as usize;
                    if len > 4 * 1024 * 1024 || len > data.len() - at {
                        bail!("invalid field size")
                    };
                    let value = data[at..at + len].to_vec();
                    at += len;
                    Value::Bytes(value)
                }
                5 => {
                    let bytes = data
                        .get(at..at + 4)
                        .ok_or_else(|| anyhow::anyhow!("short fixed32"))?;
                    at += 4;
                    Value::Fixed32(u32::from_le_bytes(bytes.try_into()?))
                }
                _ => bail!("unsupported wire type"),
            };
            fields.push((id, value));
        }
        Ok(Self(fields))
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (id, value) in &self.0 {
            let wire = match value {
                Value::Int(_) => 0,
                Value::Fixed64(_) => 1,
                Value::Bytes(_) => 2,
                Value::Fixed32(_) => 5,
            };
            put_var((*id as u64) * 8 + wire, &mut out);
            match value {
                Value::Int(n) => put_var(*n, &mut out),
                Value::Fixed64(n) => out.extend(n.to_le_bytes()),
                Value::Fixed32(n) => out.extend(n.to_le_bytes()),
                Value::Bytes(bytes) => {
                    put_var(bytes.len() as u64, &mut out);
                    out.extend(bytes)
                }
            }
        }
        out
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
        self.bytes(field, value.encode())
    }
    pub fn float(&mut self, field: u32, value: f32) -> &mut Self {
        self.add(field, Value::Fixed32(value.to_bits()))
    }
    pub fn double(&mut self, field: u32, value: f64) -> &mut Self {
        self.add(field, Value::Fixed64(value.to_bits()))
    }
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
    pub fn nested(&self, field: u32) -> Result<Node> {
        Self::parse(self.raw(field).unwrap_or_default())
    }
    pub fn real(&self, field: u32) -> Option<f64> {
        match self.get(field) {
            Some(Value::Fixed32(n)) => Some(f32::from_bits(*n) as f64),
            Some(Value::Fixed64(n)) => Some(f64::from_bits(*n)),
            _ => None,
        }
    }
}
pub fn encode_packet(messages: &[Node]) -> Vec<u8> {
    let mut out = Vec::new();
    for message in messages {
        let raw = message.encode();
        put_var(raw.len() as u64, &mut out);
        out.extend(raw)
    }
    out
}
pub fn decode_packet(mut data: &[u8]) -> Result<Vec<Node>> {
    let mut output = Vec::new();
    while !data.is_empty() {
        if output.len() > 128 {
            bail!("message count")
        };
        if data[0] == 8 {
            output.push(Node::parse(data)?);
            break;
        }
        let mut at = 0;
        let length = varint(data, &mut at)? as usize;
        if length == 0 || length > data.len() - at {
            bail!("packet size")
        };
        output.push(Node::parse(&data[at..at + length])?);
        data = &data[at + length..]
    }
    Ok(output)
}
