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
    pub const METADATA_PROGRESS: u64 = 1 << 15;
    pub const METADATA_ARTWORK: u64 = 1 << 16;
    pub const METADATA_TEXT: u64 = 1 << 17;
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
            | Self::METADATA_PROGRESS
            | Self::METADATA_ARTWORK
            | Self::METADATA_TEXT
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

    /// Зеркалирование экрана: набор UxPlay без битов AirPlay 2 (40, 41, 46,
    /// 48) и без legacy-сопряжения (бит 27). Только с ним iOS 18 присылает
    /// ключ видео `ekey` (проверено shairplay-rust).
    pub const MIRROR_RECEIVER: Features = Features(0x527F_FEE6);

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
    /// Экран для зеркалирования; без него iOS не предлагает «Повтор экрана».
    pub display: Option<Display>,
    /// `playbackCapabilities.supportsUIForAudioOnlyContent`: iPhone
    /// показывает для приёмника карточку «Сейчас играет» (нужен канал MRP).
    pub audio_only_ui: bool,
    /// `deviceEnclosureColor`: цвет корпуса для значка в iOS, например «1»
    /// — белый, «2» — чёрный (как у HomePod).
    pub enclosure_color: Option<String>,
    /// Группа текущей сессии отправителя (`groupUUID` из SETUP).
    pub group: Option<String>,
    /// `pgid`: случайный UUID на запуск, как у shairport-sync.
    pub pgid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl DeviceInfo {
    pub const PROTOCOL_VERSION: &'static str = "1.1";
    /// `osvers` и `fv`, как у shairport-sync 5.5.1 на станции.
    pub const OS_VERSION: &'static str = "15.0";
    pub const FIRMWARE_VERSION: &'static str = "5.5.1";

    /// `gid`: как в shairport-sync — группа сессии отправителя (`groupUUID`
    /// из SETUP), пока она идёт, иначе собственный `pi`.
    pub fn group_id(&self) -> String {
        self.group.clone().unwrap_or_else(|| self.pi())
    }

    /// `pi`: как `generate_device_uuid(deviceid)` в shairport-sync — UUID,
    /// выведенный из deviceid, отдельный от `psi` (идентичности сопряжения).
    pub fn pi(&self) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(self.device_id.to_ascii_lowercase().as_bytes());
        let mut b = [0u8; 16];
        b.copy_from_slice(&digest[..16]);
        b[6] = (b[6] & 0x0f) | 0x50;
        b[8] = (b[8] & 0x3f) | 0x80;
        let h = hex::encode(b);
        format!(
            "{}-{}-{}-{}-{}",
            &h[..8],
            &h[8..12],
            &h[12..16],
            &h[16..20],
            &h[20..]
        )
    }

    /// `fex`/`featuresEx`: биты Features little-endian в base64 без `=`, как у shairport-sync.
    pub fn fex(&self) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = self.features.0.to_le_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
            for i in 0..=chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        out
    }
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

    /// Тело ответа на `GET /info` (двоичный plist): набор ключей shairport-sync
    /// со станции.
    pub fn info_plist(&self) -> Vec<u8> {
        self.info_plist_for(None)
    }

    /// То же с `senderAddress` — адресом спрашивающего, как у shairport-sync.
    pub fn info_plist_for(&self, sender: Option<&str>) -> Vec<u8> {
        let mut d = Dictionary::new();
        d.insert("canRecordScreenStream".into(), false.into());
        if let Some(color) = &self.enclosure_color {
            d.insert("deviceEnclosureColor".into(), color.clone().into());
        }
        d.insert("deviceID".into(), self.device_id.clone().into());
        d.insert(
            "features".into(),
            Value::Integer((self.features.0 as i64).into()),
        );
        d.insert("featuresEx".into(), self.fex().into());
        d.insert("initialVolume".into(), Value::Real(-20.0));
        d.insert("keepAliveSendStatsAsBody".into(), false.into());
        d.insert("model".into(), self.model.clone().into());
        d.insert("name".into(), self.name.clone().into());
        d.insert("pi".into(), self.pi().into());
        d.insert("pk".into(), Value::Data(self.public_key.to_vec()));
        let mut caps = Dictionary::new();
        caps.insert("supportsFPSSecureStop".into(), false.into());
        caps.insert("supportsInterstitials".into(), false.into());
        caps.insert(
            "supportsUIForAudioOnlyContent".into(),
            self.audio_only_ui.into(),
        );
        d.insert("playbackCapabilities".into(), caps.into());
        d.insert("protocolVersion".into(), Self::PROTOCOL_VERSION.into());
        d.insert("psi".into(), self.pairing_id.clone().into());
        d.insert("screenDemoMode".into(), false.into());
        if let Some(sender) = sender {
            d.insert("senderAddress".into(), sender.into());
        }
        d.insert("sourceVersion".into(), self.source_version.clone().into());
        d.insert(
            "statusFlags".into(),
            Value::Integer(self.status_flags.into()),
        );
        let mut formats = Dictionary::new();
        formats.insert("audioStream".into(), Value::Integer(21235712.into()));
        formats.insert(
            "bufferStream".into(),
            Value::Integer(1649282383872u64.into()),
        );
        d.insert("supportedFormats".into(), formats.into());
        d.insert("txtAirPlay".into(), Value::Data(self.txt_airplay_record()));
        d.insert("volumeControlType".into(), Value::Integer(3.into()));
        d.insert("vv".into(), Value::Integer(2.into()));
        if let Some(display) = self.display {
            let mut s = Dictionary::new();
            s.insert("widthPixels".into(), Value::Integer(display.width.into()));
            s.insert("heightPixels".into(), Value::Integer(display.height.into()));
            s.insert("maxFPS".into(), Value::Integer(display.fps.into()));
            s.insert("uuid".into(), format!("{}-display", self.pairing_id).into());
            s.insert("features".into(), Value::Integer(2.into()));
            d.insert("displays".into(), Value::Array(vec![s.into()]));
        }
        let mut out = Vec::new();
        Value::Dictionary(d)
            .to_writer_binary(&mut out)
            .expect("запись plist в память не может завершиться ошибкой");
        out
    }

    /// `txtAirPlay` в `/info`: TXT-запись shairport-sync, каждая строка с байтом длины.
    fn txt_airplay_record(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for (k, v) in [
            ("acl", "0".to_string()),
            ("btaddr", "00:00:00:00:00:00".into()),
            ("deviceid", self.device_id.clone()),
            ("fex", self.fex()),
            ("features", self.features.to_txt()),
            ("flags", format!("0x{:X}", self.status_flags)),
            ("gid", self.group_id()),
            ("igl", "1".into()),
            ("gcgl", "1".into()),
            ("pgid", self.pgid.clone()),
            ("model", self.model.clone()),
            ("protovers", Self::PROTOCOL_VERSION.into()),
            ("pi", self.pi()),
            ("psi", self.pairing_id.clone()),
            ("pk", hex::encode(self.public_key)),
            ("srcvers", self.source_version.clone()),
            ("osvers", Self::OS_VERSION.into()),
            ("vv", "2".into()),
        ] {
            let entry = format!("{k}={v}");
            out.push(entry.len() as u8);
            out.extend_from_slice(entry.as_bytes());
        }
        out
    }

    /// TXT-записи службы `_airplay._tcp`.
    pub fn airplay_txt(&self) -> Vec<(String, String)> {
        let pk = hex::encode(self.public_key);
        [
            ("acl", "0".to_string()),
            ("deviceid", self.device_id.clone()),
            ("fex", self.fex()),
            ("features", self.features.to_txt()),
            ("flags", format!("0x{:X}", self.status_flags)),
            ("gid", self.group_id()),
            ("igl", "1".into()),
            ("gcgl", "1".into()),
            ("model", self.model.clone()),
            ("protovers", Self::PROTOCOL_VERSION.into()),
            ("pi", self.pi()),
            ("psi", self.pairing_id.clone()),
            ("pk", pk),
            ("srcvers", self.source_version.clone()),
            ("osvers", Self::OS_VERSION.into()),
            ("vv", "2".into()),
            ("fv", Self::FIRMWARE_VERSION.into()),
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
            display: None,
            audio_only_ui: false,
            enclosure_color: None,
            group: None,
            pgid: "00000000-0000-4000-8000-000000000000".into(),
        }
    }

    #[test]
    fn audio_receiver_matches_shairport_sync() {
        // Как TXT станции (shairport-sync 5.5.1 с метаданными).
        assert_eq!(Features::AUDIO_RECEIVER.0, 0x1C340405FCA00);
        assert_eq!(Features::AUDIO_RECEIVER.to_txt(), "0x405FCA00,0x1C340");
        let mut dev = info();
        dev.features = Features::AUDIO_RECEIVER;
        assert_eq!(dev.fex(), "AMpfQEDDAQA");
    }

    #[test]
    fn info_plist_roundtrip() {
        let v = Value::from_reader(std::io::Cursor::new(info().info_plist())).unwrap();
        let d = v.as_dictionary().unwrap();
        assert_eq!(d["deviceID"].as_string(), Some("02:11:22:33:44:55"));
        assert_eq!(d["features"].as_signed_integer(), Some(0x1C340405FCA00));
        assert_eq!(d["pk"].as_data(), Some(&[9u8; 32][..]));
        // Как у shairport: pi из deviceid, psi — идентичность сопряжения.
        assert_ne!(d["psi"].as_string(), d["pi"].as_string());
        assert_eq!(d["volumeControlType"].as_signed_integer(), Some(3));
        let txt = d["txtAirPlay"].as_data().unwrap();
        assert!(txt.windows(5).any(|w| w == b"igl=1"));
    }

    #[test]
    fn mirroring_advertises_display() {
        let mut dev = info();
        assert!(
            !Value::from_reader(std::io::Cursor::new(dev.info_plist()))
                .unwrap()
                .as_dictionary()
                .unwrap()
                .contains_key("displays")
        );
        dev.features = Features::MIRROR_RECEIVER;
        dev.display = Some(Display {
            width: 1280,
            height: 720,
            fps: 60,
        });
        let v = Value::from_reader(std::io::Cursor::new(dev.info_plist())).unwrap();
        let d = v.as_dictionary().unwrap();
        let display = d["displays"].as_array().unwrap()[0]
            .as_dictionary()
            .unwrap();
        assert_eq!(display["widthPixels"].as_unsigned_integer(), Some(1280));
        assert_eq!(display["maxFPS"].as_unsigned_integer(), Some(60));
        assert_eq!(dev.features.to_txt(), "0x527FFEE6,0x0");
    }

    #[test]
    fn raop_instance_name() {
        assert_eq!(info().raop_instance(), "021122334455@Кухня");
    }
}
