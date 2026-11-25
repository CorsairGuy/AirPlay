//! OPACK — формат сериализации Companion Link.
//!
//! Формат восстановлен по независимым референсам (pyatv `support/opack.py`,
//! apple-opack, AppleTVr); код не портирован, сверена только структура байтов.
//!
//! Тег — первый байт значения:
//! - `0x01`/`0x02` — true/false, `0x04` — null, `0x05` + 16 байт — UUID;
//! - `0x08..=0x2F` — целое 0..39 (значение = тег − 0x08);
//! - `0x30..=0x33` — целое в 1/2/4/8 байт little-endian;
//! - `0x35`/`0x36` — float 4/8 байт little-endian;
//! - `0x40..=0x60` — строка до 32 байт, `0x61..=0x64` — с длиной в 1/2/3/4 байта;
//! - `0x70..=0x90` — данные до 32 байт, `0x91..=0x94` — с длиной в 1/2/4/8 байт;
//! - `0xD0..=0xDF` — массив (нижняя тетрада — число элементов, `0xF` — до `0x03`);
//! - `0xE0..=0xEF` — словарь (то же правило);
//! - `0xA0..=0xC4` — ссылка на ранее встреченное значение.
//!
//! Ссылки понимаются при разборе, но не порождаются при сборке: они лишь
//! сокращают размер, и их отсутствие остаётся корректным OPACK.

use crate::Error;

/// Предел вложенности при разборе, чтобы битый кадр не уронил стек.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Double(f64),
    Str(String),
    Data(Vec<u8>),
    Uuid([u8; 16]),
    Array(Vec<Value>),
    Dict(Vec<(Value, Value)>),
}

impl Value {
    /// Словарь со строковыми ключами — обычная форма команд Companion.
    pub fn object<K: Into<String>>(entries: Vec<(K, Value)>) -> Self {
        Self::Dict(
            entries
                .into_iter()
                .map(|(k, v)| (Self::Str(k.into()), v))
                .collect(),
        )
    }

    pub fn empty_object() -> Self {
        Self::Dict(Vec::new())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        let Value::Dict(entries) = self else {
            return None;
        };
        entries.iter().find_map(|(k, v)| match k {
            Value::Str(k) if k == key => Some(v),
            _ => None,
        })
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_data(&self) -> Option<&[u8]> {
        match self {
            Value::Data(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(v) => Some(v),
            _ => None,
        }
    }
}

pub fn encode(value: &Value) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    write(value, &mut out)?;
    Ok(out)
}

fn write(value: &Value, out: &mut Vec<u8>) -> Result<(), Error> {
    match value {
        Value::Null => out.push(0x04),
        Value::Bool(true) => out.push(0x01),
        Value::Bool(false) => out.push(0x02),
        Value::Uuid(bytes) => {
            out.push(0x05);
            out.extend_from_slice(bytes);
        }
        Value::Int(v) => write_int(*v, out)?,
        Value::Double(v) => {
            out.push(0x36);
            out.extend_from_slice(&v.to_le_bytes());
        }
        Value::Str(s) => {
            let bytes = s.as_bytes();
            let len = bytes.len();
            match len {
                0..=0x20 => out.push(0x40 + len as u8),
                0x21..=0xFF => {
                    out.push(0x61);
                    out.push(len as u8);
                }
                0x100..=0xFFFF => {
                    out.push(0x62);
                    out.extend_from_slice(&(len as u16).to_le_bytes());
                }
                0x1_0000..=0xFF_FFFF => {
                    out.push(0x63);
                    out.extend_from_slice(&(len as u32).to_le_bytes()[..3]);
                }
                _ => {
                    out.push(0x64);
                    out.extend_from_slice(&(len as u32).to_le_bytes());
                }
            }
            out.extend_from_slice(bytes);
        }
        Value::Data(bytes) => {
            let len = bytes.len();
            match len {
                0..=0x20 => out.push(0x70 + len as u8),
                0x21..=0xFF => {
                    out.push(0x91);
                    out.push(len as u8);
                }
                0x100..=0xFFFF => {
                    out.push(0x92);
                    out.extend_from_slice(&(len as u16).to_le_bytes());
                }
                0x1_0000..=0xFFFF_FFFF => {
                    out.push(0x93);
                    out.extend_from_slice(&(len as u32).to_le_bytes());
                }
                _ => {
                    out.push(0x94);
                    out.extend_from_slice(&(len as u64).to_le_bytes());
                }
            }
            out.extend_from_slice(bytes);
        }
        Value::Array(items) => {
            out.push(0xD0 + items.len().min(0xF) as u8);
            for item in items {
                write(item, out)?;
            }
            if items.len() >= 0xF {
                out.push(0x03);
            }
        }
        Value::Dict(entries) => {
            out.push(0xE0 + entries.len().min(0xF) as u8);
            for (k, v) in entries {
                write(k, out)?;
                write(v, out)?;
            }
            if entries.len() >= 0xF {
                out.push(0x03);
            }
        }
    }
    Ok(())
}

fn write_int(v: i64, out: &mut Vec<u8>) -> Result<(), Error> {
    if v < 0 {
        // Референсный сборщик отрицательные целые не пишет; не подменяем
        // их двумя дополнениями молча — пусть вызывающий использует Double.
        return Err(Error::Opack("отрицательное целое не кодируется"));
    }
    let v = v as u64;
    if v < 0x28 {
        out.push(0x08 + v as u8);
    } else if v <= 0xFF {
        out.push(0x30);
        out.push(v as u8);
    } else if v <= 0xFFFF {
        out.push(0x31);
        out.extend_from_slice(&(v as u16).to_le_bytes());
    } else if v <= 0xFFFF_FFFF {
        out.push(0x32);
        out.extend_from_slice(&(v as u32).to_le_bytes());
    } else {
        out.push(0x33);
        out.extend_from_slice(&v.to_le_bytes());
    }
    Ok(())
}

/// Разбирает одно значение и возвращает его вместе с числом прочитанных байт.
pub fn decode(data: &[u8]) -> Result<(Value, usize), Error> {
    let mut decoder = Decoder {
        data,
        pos: 0,
        objects: Vec::new(),
    };
    let value = decoder.value(0)?;
    Ok((value, decoder.pos))
}

/// Разбирает значение, требуя, чтобы хвоста не осталось.
pub fn unpack(data: &[u8]) -> Result<Value, Error> {
    let (value, used) = decode(data)?;
    if used != data.len() {
        return Err(Error::Opack("лишние байты после значения"));
    }
    Ok(value)
}

struct Decoder<'a> {
    data: &'a [u8],
    pos: usize,
    objects: Vec<Value>,
}

impl Decoder<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(Error::Opack("переполнение длины"))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(Error::Opack("обрезанное значение"))?;
        self.pos = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn uint(&mut self, n: usize) -> Result<u64, Error> {
        let bytes = self.take(n)?;
        Ok(bytes
            .iter()
            .rev()
            .fold(0u64, |acc, b| (acc << 8) | *b as u64))
    }

    fn remember(&mut self, value: Value, track: bool) -> Value {
        if track {
            self.objects.push(value.clone());
        }
        value
    }

    fn value(&mut self, depth: usize) -> Result<Value, Error> {
        if depth > MAX_DEPTH {
            return Err(Error::Opack("слишком глубокая вложенность"));
        }
        let tag = self.byte()?;
        let value = match tag {
            0x01 => self.remember(Value::Bool(true), false),
            0x02 => self.remember(Value::Bool(false), false),
            0x04 => self.remember(Value::Null, false),
            0x03 => return Err(Error::Opack("маркер конца без контейнера")),
            0x05 => {
                let bytes: [u8; 16] = self
                    .take(16)?
                    .try_into()
                    .expect("длина проверена через take");
                self.remember(Value::Uuid(bytes), true)
            }
            0x06 => {
                // Абсолютное время: референс читает его как целое little-endian.
                let raw = self.uint(8)?;
                self.remember(Value::Int(raw as i64), true)
            }
            0x08..=0x2F => self.remember(Value::Int((tag - 0x08) as i64), false),
            0x30..=0x33 => {
                let size = 1usize << (tag & 0x0F);
                let raw = self.uint(size)?;
                self.remember(Value::Int(raw as i64), true)
            }
            0x35 => {
                let bytes: [u8; 4] = self.take(4)?.try_into().expect("длина проверена");
                self.remember(Value::Double(f32::from_le_bytes(bytes) as f64), true)
            }
            0x36 => {
                let bytes: [u8; 8] = self.take(8)?.try_into().expect("длина проверена");
                self.remember(Value::Double(f64::from_le_bytes(bytes)), true)
            }
            0x40..=0x60 => {
                let len = (tag - 0x40) as usize;
                let bytes = self.take(len)?.to_vec();
                let text = String::from_utf8(bytes).map_err(|_| Error::Opack("не UTF-8"))?;
                self.remember(Value::Str(text), true)
            }
            0x61..=0x64 => {
                let len = self.uint((tag - 0x60) as usize)? as usize;
                let bytes = self.take(len)?.to_vec();
                let text = String::from_utf8(bytes).map_err(|_| Error::Opack("не UTF-8"))?;
                self.remember(Value::Str(text), true)
            }
            0x70..=0x90 => {
                let len = (tag - 0x70) as usize;
                let bytes = self.take(len)?.to_vec();
                self.remember(Value::Data(bytes), true)
            }
            0x91..=0x94 => {
                let len = self.uint(1usize << (tag - 0x91))? as usize;
                let bytes = self.take(len)?.to_vec();
                self.remember(Value::Data(bytes), true)
            }
            0xD0..=0xDF => {
                let count = (tag & 0x0F) as usize;
                let mut items = Vec::new();
                if count == 0x0F {
                    while self.peek()? != 0x03 {
                        items.push(self.value(depth + 1)?);
                    }
                    self.pos += 1;
                } else {
                    for _ in 0..count {
                        items.push(self.value(depth + 1)?);
                    }
                }
                self.remember(Value::Array(items), false)
            }
            0xE0..=0xEF => {
                let count = (tag & 0x0F) as usize;
                let mut entries = Vec::new();
                if count == 0x0F {
                    while self.peek()? != 0x03 {
                        let key = self.value(depth + 1)?;
                        let value = self.value(depth + 1)?;
                        entries.push((key, value));
                    }
                    self.pos += 1;
                } else {
                    for _ in 0..count {
                        let key = self.value(depth + 1)?;
                        let value = self.value(depth + 1)?;
                        entries.push((key, value));
                    }
                }
                self.remember(Value::Dict(entries), false)
            }
            0xA0..=0xC0 => {
                let index = (tag - 0xA0) as usize;
                self.objects
                    .get(index)
                    .cloned()
                    .ok_or(Error::Opack("ссылка на неизвестное значение"))?
            }
            0xC1..=0xC4 => {
                let index = self.uint((tag - 0xC0) as usize)? as usize;
                self.objects
                    .get(index)
                    .cloned()
                    .ok_or(Error::Opack("ссылка на неизвестное значение"))?
            }
            other => return Err(Error::Tag(other)),
        };
        Ok(value)
    }

    fn peek(&self) -> Result<u8, Error> {
        self.data
            .get(self.pos)
            .copied()
            .ok_or(Error::Opack("обрезанное значение"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_golden_bytes() {
        let cases: Vec<(Value, &str)> = vec![
            (Value::Bool(true), "01"),
            (Value::Bool(false), "02"),
            (Value::Null, "04"),
            (Value::Int(0), "08"),
            (Value::Int(39), "2f"),
            (Value::Int(40), "3028"),
            (Value::Int(0x1234), "313412"),
            (Value::Str(String::new()), "40"),
            (Value::Str("abc".into()), "43616263"),
            (Value::Data(vec![1, 2]), "720102"),
        ];
        for (value, want) in cases {
            assert_eq!(hex::encode(encode(&value).unwrap()), want, "{value:?}");
        }
    }

    #[test]
    fn containers_golden_bytes() {
        assert_eq!(
            hex::encode(encode(&Value::Array(vec![Value::Int(1), Value::Bool(true)])).unwrap()),
            "d20901"
        );
        assert_eq!(hex::encode(encode(&Value::Array(vec![])).unwrap()), "d0");
        assert_eq!(
            hex::encode(encode(&Value::object(vec![("a", Value::Int(1))])).unwrap()),
            "e1416109"
        );
        assert_eq!(hex::encode(encode(&Value::empty_object()).unwrap()), "e0");
    }

    #[test]
    fn long_strings_and_data_use_extended_lengths() {
        let s = "x".repeat(33);
        let bytes = encode(&Value::Str(s.clone())).unwrap();
        assert_eq!(bytes[0], 0x61);
        assert_eq!(bytes[1], 33);
        assert_eq!(&bytes[2..], s.as_bytes());

        let data = vec![0xAB; 33];
        let bytes = encode(&Value::Data(data.clone())).unwrap();
        assert_eq!(bytes[0], 0x91);
        assert_eq!(bytes[1], 33);
        assert_eq!(&bytes[2..], data.as_slice());
    }

    #[test]
    fn fifteen_items_are_terminated() {
        let items: Vec<Value> = (0..15i64).map(Value::Int).collect();
        let bytes = encode(&Value::Array(items)).unwrap();
        assert_eq!(bytes[0], 0xDF);
        assert_eq!(*bytes.last().unwrap(), 0x03);
        assert_eq!(
            unpack(&bytes).unwrap(),
            Value::Array((0..15i64).map(Value::Int).collect())
        );

        let entries: Vec<(&str, Value)> = (0..15i64).map(|i| ("k", Value::Int(i))).collect();
        let bytes = encode(&Value::object(entries)).unwrap();
        assert_eq!(bytes[0], 0xEF);
        assert_eq!(*bytes.last().unwrap(), 0x03);
        let Value::Dict(entries) = unpack(&bytes).unwrap() else {
            panic!("ожидался словарь");
        };
        assert_eq!(entries.len(), 15);
        assert_eq!(entries[14].1, Value::Int(14));
    }

    #[test]
    fn value_references_are_resolved() {
        // Массив из трёх элементов: строка "ab" и две ссылки на неё.
        let bytes = hex::decode("d3426162a0a0").unwrap();
        assert_eq!(
            unpack(&bytes).unwrap(),
            Value::Array(vec![
                Value::Str("ab".into()),
                Value::Str("ab".into()),
                Value::Str("ab".into())
            ])
        );
        // Расширенная ссылка c1 00.
        let bytes = hex::decode("d2426162c100").unwrap();
        assert_eq!(
            unpack(&bytes).unwrap(),
            Value::Array(vec![Value::Str("ab".into()), Value::Str("ab".into())])
        );
    }

    #[test]
    fn truncated_or_bad_input_is_an_error() {
        assert!(unpack(&[]).is_err());
        assert!(unpack(&[0x43, b'a']).is_err());
        assert!(unpack(&[0x91]).is_err());
        assert!(unpack(&[0xE1, 0x41]).is_err());
        assert!(unpack(&[0x00]).is_err());
        assert!(unpack(&[0x04, 0x04]).is_err());
        assert!(encode(&Value::Int(-1)).is_err());
    }

    #[test]
    fn deep_nesting_is_rejected() {
        let bytes = vec![0xD1; MAX_DEPTH + 2];
        assert!(unpack(&bytes).is_err());
    }

    #[test]
    fn roundtrip_of_a_command_payload() {
        let value = Value::object(vec![
            ("_i", Value::Str("_hidC".into())),
            ("_t", Value::Int(2)),
            ("_x", Value::Int(7)),
            (
                "_c",
                Value::object(vec![
                    ("_hBtS", Value::Int(1)),
                    ("_hidC", Value::Int(6)),
                    ("_vol", Value::Double(0.25)),
                ]),
            ),
        ]);
        let bytes = encode(&value).unwrap();
        assert_eq!(unpack(&bytes).unwrap(), value);
    }
}
