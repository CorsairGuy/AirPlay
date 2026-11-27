//! TLV8 из спецификации HAP: тип (1 байт), длина (1 байт), значение.
//! Значения длиннее 255 байт режутся на подряд идущие куски одного типа.

use crate::Error;

pub const METHOD: u8 = 0x00;
pub const IDENTIFIER: u8 = 0x01;
pub const SALT: u8 = 0x02;
pub const PUBLIC_KEY: u8 = 0x03;
pub const PROOF: u8 = 0x04;
pub const ENCRYPTED_DATA: u8 = 0x05;
pub const STATE: u8 = 0x06;
pub const ERROR: u8 = 0x07;
pub const SIGNATURE: u8 = 0x0A;
pub const FLAGS: u8 = 0x13;

/// Коды ошибок HAP.
pub const ERROR_AUTHENTICATION: u8 = 0x02;

/// Флаг transient-сопряжения в поле FLAGS.
pub const FLAG_TRANSIENT: u32 = 0x10;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tlv8(Vec<(u8, Vec<u8>)>);

impl Tlv8 {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, ty: u8, value: impl Into<Vec<u8>>) -> Self {
        self.0.push((ty, value.into()));
        self
    }

    pub fn get(&self, ty: u8) -> Option<&[u8]> {
        self.0
            .iter()
            .find(|(t, _)| *t == ty)
            .map(|(_, v)| v.as_slice())
    }

    pub fn require(&self, ty: u8, what: &'static str) -> Result<&[u8], Error> {
        self.get(ty).ok_or(Error::Malformed(what))
    }

    /// Целое без знака в little-endian (так кодируются STATE, FLAGS, METHOD).
    pub fn get_uint(&self, ty: u8) -> Option<u32> {
        self.get(ty)
            .filter(|v| v.len() <= 4)
            .map(|v| v.iter().rev().fold(0u32, |acc, b| (acc << 8) | *b as u32))
    }

    pub fn state(&self) -> Result<u8, Error> {
        self.get_uint(STATE)
            .map(|s| s as u8)
            .ok_or(Error::Malformed("нет поля State"))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (ty, value) in &self.0 {
            if value.is_empty() {
                out.extend_from_slice(&[*ty, 0]);
            }
            for chunk in value.chunks(255) {
                out.push(*ty);
                out.push(chunk.len() as u8);
                out.extend_from_slice(chunk);
            }
        }
        out
    }

    pub fn decode(mut data: &[u8]) -> Result<Self, Error> {
        let mut items: Vec<(u8, Vec<u8>)> = Vec::new();
        let mut prev_full = false;
        while !data.is_empty() {
            if data.len() < 2 {
                return Err(Error::Malformed("обрезанный TLV8"));
            }
            let (ty, len) = (data[0], data[1] as usize);
            let value = data
                .get(2..2 + len)
                .ok_or(Error::Malformed("обрезанный TLV8"))?;
            match items.last_mut() {
                // Продолжение длинного значения: тот же тип сразу после куска в 255 байт.
                Some((last, v)) if *last == ty && prev_full => v.extend_from_slice(value),
                _ => items.push((ty, value.to_vec())),
            }
            prev_full = len == 255;
            data = &data[2 + len..];
        }
        Ok(Self(items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_values_are_fragmented_and_joined() {
        let key = vec![0xAB; 384];
        let tlv = Tlv8::new()
            .with(STATE, [2])
            .with(PUBLIC_KEY, key.clone())
            .with(SALT, [1; 16]);
        let wire = tlv.encode();
        assert_eq!(wire.len(), 3 + (2 + 255) + (2 + 129) + 18);
        let back = Tlv8::decode(&wire).unwrap();
        assert_eq!(back.get(PUBLIC_KEY).unwrap(), key.as_slice());
        assert_eq!(back.state().unwrap(), 2);
        assert_eq!(back, tlv);
    }

    #[test]
    fn truncated_input_is_an_error() {
        assert!(Tlv8::decode(&[STATE, 5, 1]).is_err());
    }

    /// Любой обрез действительного сообщения — ошибка или короткий разбор,
    /// но не паника и не выход за границу значения.
    #[test]
    fn every_truncation_is_handled() {
        let wire = Tlv8::new()
            .with(STATE, [1])
            .with(PUBLIC_KEY, vec![7; 300])
            .with(SIGNATURE, vec![3; 70])
            .encode();
        for cut in 0..wire.len() {
            let _ = Tlv8::decode(&wire[..cut]);
        }
    }

    #[test]
    fn empty_and_oversized_values_are_bounded() {
        // Пустое значение сохраняется как есть и переживает перекодирование.
        let tlv = Tlv8::decode(&[STATE, 0]).unwrap();
        assert_eq!(tlv.state().unwrap(), 0);
        assert_eq!(Tlv8::decode(&tlv.encode()).unwrap(), tlv);

        // State длиннее 4 байт не превращается в число.
        let long = Tlv8::decode(&[STATE, 5, 1, 2, 3, 4, 5]).unwrap();
        assert!(long.state().is_err());

        // 255-байтовый кусок — не конец сообщения: следующий тип читается.
        let mut wire = vec![5, 255];
        wire.extend_from_slice(&[0xAA; 255]);
        wire.extend_from_slice(&[6, 1, 2]);
        let tlv = Tlv8::decode(&wire).unwrap();
        assert_eq!(tlv.get(ENCRYPTED_DATA).unwrap().len(), 255);
        assert_eq!(tlv.state().unwrap(), 2);
    }

    /// 255-байтовый кусок — продолжение значения, а не отдельное поле;
    /// 254 байта — уже отдельная запись, даже если тип совпадает.
    #[test]
    fn fragment_boundaries_are_joined_and_not_conflated_with_repeats() {
        for len in [254usize, 255, 256, 509, 510, 511] {
            let value = vec![0x5A; len];
            let tlv = Tlv8::new().with(PUBLIC_KEY, value.clone());
            let wire = tlv.encode();
            let chunks = len.div_ceil(255);
            assert_eq!(wire.len(), len + 2 * chunks, "len={len}");
            let back = Tlv8::decode(&wire).unwrap();
            assert_eq!(back.get(PUBLIC_KEY).unwrap(), value.as_slice());
            assert_eq!(back, tlv);
        }

        // 255 + 45 склеиваются в одно значение из 300 байт.
        let mut wire = vec![PUBLIC_KEY, 255];
        wire.extend_from_slice(&[0x11; 255]);
        wire.extend_from_slice(&[PUBLIC_KEY, 45]);
        wire.extend_from_slice(&[0x22; 45]);
        let tlv = Tlv8::decode(&wire).unwrap();
        assert_eq!(tlv.get(PUBLIC_KEY).unwrap().len(), 300);
        assert_eq!(tlv.encode(), wire);

        // 254 байта — не полный кусок: следующее поле того же типа остаётся отдельным.
        let mut wire = vec![PUBLIC_KEY, 254];
        wire.extend_from_slice(&[0x11; 254]);
        wire.extend_from_slice(&[PUBLIC_KEY, 2, 0x22, 0x22]);
        let tlv = Tlv8::decode(&wire).unwrap();
        assert_eq!(tlv.get(PUBLIC_KEY).unwrap().len(), 254);
        assert_eq!(tlv.encode(), wire);
    }

    /// Повторное поле с тем же типом — не фрагмент: `get` возвращает первое,
    /// и перекодирование сохраняет исходный порядок записей.
    #[test]
    fn repeated_fields_keep_the_first_value_and_wire_order() {
        let wire = Tlv8::new()
            .with(STATE, [1])
            .with(FLAGS, [0])
            .with(STATE, [2])
            .encode();
        let tlv = Tlv8::decode(&wire).unwrap();
        assert_eq!(tlv.state().unwrap(), 1, "первое поле побеждает");
        assert_eq!(tlv.get_uint(STATE), Some(1));
        assert_eq!(tlv.encode(), wire);

        // Пустая запись не поглощает следующую запись того же типа.
        let raw = [STATE, 0, STATE, 1, 3];
        let tlv = Tlv8::decode(&raw).unwrap();
        assert_eq!(tlv.get(STATE).unwrap(), &[] as &[u8]);
        assert_eq!(tlv.encode(), raw);
    }

    /// Детерминированный набор повреждённых входов: разбор и перекодирование
    /// не паникуют и не выходят за границы.
    #[test]
    fn arbitrary_bytes_are_handled_without_panics() {
        let mut state = 0x1234_5678u32;
        for _ in 0..500 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let len = (state as usize) % 512;
            let mut buffer = vec![0u8; len];
            for byte in &mut buffer {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                *byte = (state >> 16) as u8;
            }
            if let Ok(tlv) = Tlv8::decode(&buffer) {
                let _ = tlv.encode();
                let _ = tlv.state();
                let _ = tlv.get_uint(FLAGS);
                let _ = tlv.require(ERROR, "error");
            }
        }
    }

}
