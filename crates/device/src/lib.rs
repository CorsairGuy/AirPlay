//! Описание устройства-приёмника: биты Features, ответ на `GET /info`
//! и TXT-записи Bonjour для `_airplay._tcp` и `_raop._tcp`.

use plist::{Dictionary, Value};

/// Биты поля Features. Названия по публичным описаниям протокола
/// (openairplay, shairport-sync); значения части битов Apple не раскрывает.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Features(pub u64);

impl Features {
    pub const VIDEO: u64 = 1 << 0;
    pub const PHOTO: u64 = 1 << 1;
    pub const SCREEN: u64 = 1 << 7;
    pub const AUDIO: u64 = 1 << 9;
    pub const AUDIO_REDUNDANT: u64 = 1 << 11;
    pub const FAIRPLAY_AUTH: u64 = 1 << 14;
    pub const METADATA_ARTWORK: u64 = 1 << 16;
    pub const AUDIO_FORMATS_1: u64 = 1 << 18;
    pub const AUDIO_FORMATS_2: u64 = 1 << 19;
    pub const AUDIO_FORMATS_3: u64 = 1 << 20;
    pub const AUDIO_FORMATS_4: u64 = 1 << 22;
    pub const UNIFIED_ADVERTISER_INFO: u64 = 1 << 30;
    pub const UNIFIED_MEDIA_CONTROL: u64 = 1 << 38;
    pub const BUFFERED_AUDIO: u64 = 1 << 40;
    pub const PTP: u64 = 1 << 41;
    pub const HK_PAIRING_AND_ACCESS_CONTROL: u64 = 1 << 46;
    pub const SYSTEM_PAIRING: u64 = 1 << 47;
    pub const COREUTILS_PAIRING_AND_ENCRYPTION: u64 = 1 << 48;

    /// Приёмник звука AirPlay 2 с transient-сопряжением, как shairport-sync
    /// (0x1C340405D4A00): этот набор iOS принимает без MFi.
    pub const AUDIO_RECEIVER: Features = Features(
        Self::AUDIO
            | Self::AUDIO_REDUNDANT
            | Self::FAIRPLAY_AUTH
            | Self::METADATA_ARTWORK
            | Self::AUDIO_FORMATS_1
            | Self::AUDIO_FORMATS_2
            | Self::AUDIO_FORMATS_3
            | Self::AUDIO_FORMATS_4
            | Self::UNIFIED_ADVERTISER_INFO
            | Self::UNIFIED_MEDIA_CONTROL
            | Self::BUFFERED_AUDIO
            | Self::PTP
            | Self::HK_PAIRING_AND_ACCESS_CONTROL
            | Self::SYSTEM_PAIRING
            | Self::COREUTILS_PAIRING_AND_ENCRYPTION,
    );

    /// Запись для TXT: `0xНИЖНИЕ32,0xВЕРХНИЕ32`.
    pub fn to_txt(self) -> String {
        format!("0x{:X},0x{:X}", self.0 as u32, (self.0 >> 32) as u32)
    }
}

/// Всё, что приёмник сообщает о себе.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    /// Идентификатор в виде MAC-адреса `AA:BB:CC:DD:EE:FF`.
    pub device_id: String,
    pub model: String,
    pub features: Features,
    pub status_flags: u32,
    /// UUID устройства: `pi`, он же идентификатор для сопряжения.
    pub pairing_id: String,
    /// Публичный ключ Ed25519 (`pk`).
    pub public_key: [u8; 32],
    pub source_version: String,
}

impl DeviceInfo {
    pub const PROTOCOL_VERSION: &'static str = "1.1";
    /// statusFlags: приёмник готов к работе.
    pub const STATUS_READY: u32 = 0x4;
    /// statusFlags: нужен PIN (как у shairplay-rust в режиме PIN).
    pub const STATUS_PIN_REQUIRED: u32 = 0x200;
    /// statusFlags: сопряжение по PIN уже выполнено хотя бы с одним контроллером.
    pub const STATUS_PAIRED: u32 = 0x400;

    /// Режим «только по PIN»: снимает transient-сопряжение (бит 47), без
    /// которого iOS спрашивает PIN, и выставляет флаги состояния.
    pub fn require_pin(&mut self, paired: bool) {
        self.features = Features(self.features.0 & !Features::SYSTEM_PAIRING);
        self.status_flags = Self::STATUS_READY | Self::STATUS_PIN_REQUIRED;
        if paired {
            self.status_flags |= Self::STATUS_PAIRED;
        }
    }

    /// Тело ответа на `GET /info` (двоичный plist).
    pub fn info_plist(&self) -> Vec<u8> {
        let mut d = Dictionary::new();
        d.insert("deviceID".into(), self.device_id.clone().into());
        d.insert("macAddress".into(), self.device_id.clone().into());
        d.insert(
            "features".into(),
            Value::Integer((self.features.0 as i64).into()),
        );
        d.insert(
            "statusFlags".into(),
            Value::Integer(self.status_flags.into()),
        );
        d.insert("name".into(), self.name.clone().into());
        d.insert("model".into(), self.model.clone().into());
        d.insert("pi".into(), self.pairing_id.clone().into());
        d.insert("pk".into(), Value::Data(self.public_key.to_vec()));
        d.insert("protocolVersion".into(), Self::PROTOCOL_VERSION.into());
        d.insert("sourceVersion".into(), self.source_version.clone().into());
        d.insert("keepAliveLowPower".into(), true.into());
        d.insert("keepAliveSendStatsAsBody".into(), true.into());
        let mut out = Vec::new();
        Value::Dictionary(d)
            .to_writer_binary(&mut out)
            .expect("запись plist в память не может завершиться ошибкой");
        out
    }

    /// TXT-записи службы `_airplay._tcp`.
    pub fn airplay_txt(&self) -> Vec<(String, String)> {
        let pk = hex::encode(self.public_key);
        [
            ("acl", "0".to_string()),
            ("deviceid", self.device_id.clone()),
            ("features", self.features.to_txt()),
            ("flags", format!("0x{:X}", self.status_flags)),
            ("gid", self.pairing_id.clone()),
            ("igl", "1".into()),
            ("gcgl", "1".into()),
            ("model", self.model.clone()),
            ("protovers", Self::PROTOCOL_VERSION.into()),
            ("pi", self.pairing_id.clone()),
            ("psi", self.pairing_id.clone()),
            ("pk", pk),
            ("srcvers", self.source_version.clone()),
            ("vv", "2".into()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    /// Имя экземпляра `_raop._tcp`: `AABBCCDDEEFF@Имя`.
    pub fn raop_instance(&self) -> String {
        format!("{}@{}", self.device_id.replace(':', ""), self.name)
    }

    /// TXT-записи службы `_raop._tcp`.
    pub fn raop_txt(&self) -> Vec<(String, String)> {
        [
            ("cn", "0,1".to_string()),
            ("da", "true".into()),
            ("et", "0,4".into()),
            ("ft", self.features.to_txt()),
            ("am", self.model.clone()),
            ("md", "0,1,2".into()),
            ("sf", format!("0x{:X}", self.status_flags)),
            ("tp", "UDP".into()),
            ("vn", "65537".into()),
            ("vs", self.source_version.clone()),
            ("vv", "2".into()),
            ("pk", hex::encode(self.public_key)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> DeviceInfo {
        DeviceInfo {
            name: "Кухня".into(),
            device_id: "02:11:22:33:44:55".into(),
            model: "AirPlay".into(),
            features: Features::AUDIO_RECEIVER,
            status_flags: 0x4,
            pairing_id: "b6a2c8d0-0000-4000-8000-000000000001".into(),
            public_key: [9; 32],
            source_version: "366.0".into(),
        }
    }

    #[test]
    fn audio_receiver_matches_shairport_sync() {
        assert_eq!(Features::AUDIO_RECEIVER.0, 0x1C340405D4A00);
        assert_eq!(Features::AUDIO_RECEIVER.to_txt(), "0x405D4A00,0x1C340");
    }

    #[test]
    fn info_plist_roundtrip() {
        let v = Value::from_reader(std::io::Cursor::new(info().info_plist())).unwrap();
        let d = v.as_dictionary().unwrap();
        assert_eq!(d["deviceID"].as_string(), Some("02:11:22:33:44:55"));
        assert_eq!(d["features"].as_signed_integer(), Some(0x1C340405D4A00));
        assert_eq!(d["pk"].as_data(), Some(&[9u8; 32][..]));
    }

    #[test]
    fn raop_instance_name() {
        assert_eq!(info().raop_instance(), "021122334455@Кухня");
    }
}
