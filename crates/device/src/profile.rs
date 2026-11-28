//! Внешний профиль приёмника: имя, модель, маска возможностей, версия,
//! экран, accessory type и объявляемые службы.
//!
//! Профиль — обычный plist (XML или двоичный), поэтому его можно положить
//! рядом с конфигурацией устройства и не пересобирать программу. Сам профиль
//! ничего не доказывает о совместимости: значения только описывают, чем
//! приёмник представляется; непроверенные возможности объявлять нельзя.

use std::path::Path;

use plist::{Dictionary, Value};

use crate::{Display, Features};

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// Имя в списке AirPlay; `None` — взять имя из настроек хоста.
    pub name: Option<String>,
    pub model: String,
    pub features: Features,
    pub status_flags: u32,
    pub source_version: String,
    pub accessory_type: Option<u64>,
    /// Экран; обязателен ровно тогда, когда в маске есть бит SCREEN.
    pub display: Option<Display>,
    pub audio_only_ui: bool,
    pub enclosure_color: Option<String>,
    /// Объявлять `_raop._tcp` вместе с `_airplay._tcp`.
    pub raop: bool,
}

impl Profile {
    /// Читает XML- или binary-plist профиля.
    pub fn load(path: &Path) -> Result<Self, String> {
        let value = Value::from_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_value(&value)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let value =
            plist::from_bytes::<Value>(text.as_bytes()).map_err(|e| format!("профиль: {e}"))?;
        Self::from_value(&value)
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let d = value
            .as_dictionary()
            .ok_or("профиль: ожидался словарь plist")?;
        let known = [
            "name",
            "model",
            "features",
            "status_flags",
            "source_version",
            "accessory_type",
            "display",
            "audio_only_ui",
            "enclosure_color",
            "raop",
        ];
        for key in d.keys() {
            if !known.contains(&key.as_str()) {
                return Err(format!("профиль: неизвестный ключ {key}"));
            }
        }
        let text = |key: &str| -> Result<Option<String>, String> {
            match d.get(key) {
                None => Ok(None),
                Some(v) => v
                    .as_string()
                    .map(|s| Some(s.to_owned()))
                    .ok_or_else(|| format!("профиль: {key} должен быть строкой")),
            }
        };
        let number = |key: &str| -> Result<Option<u64>, String> { number(d, key) };
        let boolean = |key: &str| -> Result<Option<bool>, String> {
            match d.get(key) {
                None => Ok(None),
                Some(v) => v
                    .as_boolean()
                    .map(Some)
                    .ok_or_else(|| format!("профиль: {key} должен быть boolean")),
            }
        };

        let profile = Self {
            name: text("name")?,
            model: text("model")?.ok_or("профиль: нужен model")?,
            features: Features(number("features")?.ok_or("профиль: нужен features")?),
            status_flags: u32::try_from(number("status_flags")?.unwrap_or(0x404))
                .map_err(|_| "профиль: status_flags больше u32")?,
            source_version: text("source_version")?.unwrap_or_else(|| "366.0".into()),
            accessory_type: number("accessory_type")?,
            display: d.get("display").map(parse_display).transpose()?,
            audio_only_ui: boolean("audio_only_ui")?.unwrap_or(false),
            enclosure_color: text("enclosure_color")?,
            raop: boolean("raop")?.unwrap_or(true),
        };
        profile.validate()?;
        Ok(profile)
    }

    fn validate(&self) -> Result<(), String> {
        if self.model.is_empty() || self.model.len() > 64 || !self.model.is_ascii() {
            return Err("профиль: model — непустая ASCII-строка до 64 байт".into());
        }
        if !self.model.chars().all(|c| c.is_ascii_graphic() || c == ' ') {
            return Err("профиль: в model недопустимые символы".into());
        }
        if let Some(name) = &self.name
            && (name.is_empty() || name.len() > 64)
        {
            return Err("профиль: name — непустая строка до 64 байт".into());
        }
        if let Some(color) = &self.enclosure_color
            && (color.is_empty() || color.len() > 32)
        {
            return Err("профиль: enclosure_color — строка до 32 байт".into());
        }
        if self.source_version.is_empty()
            || self.source_version.len() > 32
            || !self
                .source_version
                .starts_with(|c: char| c.is_ascii_digit())
            || !self
                .source_version
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.')
        {
            return Err("профиль: source_version — версия вида 366.0".into());
        }
        if self.features.0 == 0 {
            return Err("профиль: features не может быть нулём".into());
        }
        let screen = self.features.0 & Features::SCREEN != 0;
        match (&self.display, screen) {
            (Some(display), true) => {
                if !(1..=7680).contains(&display.width)
                    || !(1..=7680).contains(&display.height)
                    || !(1..=120).contains(&display.fps)
                {
                    return Err("профиль: display — ширина/высота 1..7680, fps 1..120".into());
                }
            }
            (Some(_), false) => {
                return Err("профиль: display задан без бита SCREEN в features".into());
            }
            (None, true) => {
                return Err("профиль: бит SCREEN требует display".into());
            }
            (None, false) => {}
        }
        Ok(())
    }
}

fn parse_display(value: &Value) -> Result<Display, String> {
    let d = value
        .as_dictionary()
        .ok_or("профиль: display должен быть словарём")?;
    for key in d.keys() {
        if !matches!(key.as_str(), "width" | "height" | "fps") {
            return Err(format!("профиль: неизвестный ключ display.{key}"));
        }
    }
    let get =
        |key: &str| number(d, key)?.ok_or_else(|| format!("профиль: display.{key} обязателен"));
    Ok(Display {
        width: u32::try_from(get("width")?).map_err(|_| "профиль: display.width")?,
        height: u32::try_from(get("height")?).map_err(|_| "профиль: display.height")?,
        fps: u32::try_from(get("fps")?).map_err(|_| "профиль: display.fps")?,
        ..Display::default()
    })
}

/// Число: целое plist или строка `0x...`/десятичная (маски удобнее в hex).
fn number(d: &Dictionary, key: &str) -> Result<Option<u64>, String> {
    match d.get(key) {
        None => Ok(None),
        Some(Value::Integer(i)) => i
            .as_unsigned()
            .ok_or_else(|| format!("профиль: {key} не может быть отрицательным"))
            .map(Some),
        Some(Value::String(s)) => {
            let s = s.trim();
            let parsed = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                Some(hex) => u64::from_str_radix(hex, 16),
                None => s.parse(),
            };
            parsed
                .map(Some)
                .map_err(|_| format!("профиль: {key} — не число"))
        }
        Some(_) => Err(format!("профиль: {key} должен быть числом")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEAKER: &str = include_str!("../../../docs/profiles/AirPort.plist");
    const OTHER_TV: &str = include_str!("../../../docs/profiles/OtherTV.plist");
    const HOME_POD: &str = include_str!("../../../docs/profiles/HomePodMini.plist");
    const APPLE_TV: &str = include_str!("../../../docs/profiles/AppleTV.plist");

    #[test]
    fn examples_parse_and_validate() {
        let speaker = Profile::parse(SPEAKER).unwrap();
        assert_eq!(speaker.name.as_deref(), Some("AirPort"));
        assert_eq!(speaker.model, "AirPort10,115");
        assert!(speaker.display.is_none());
        assert!(speaker.features.0 & Features::AUDIO != 0);
        assert!(speaker.features.0 & Features::SCREEN == 0);

        let other_tv = Profile::parse(OTHER_TV).unwrap();
        assert_eq!(other_tv.name.as_deref(), Some("OtherTV"));
        assert_eq!(other_tv.model, "AppleTV3,2");
        assert_eq!(other_tv.features.0, 0x8300527FFEE6);
        assert_eq!(other_tv.accessory_type, Some(1));
        assert_eq!(other_tv.source_version, "550.10");

        let home_pod = Profile::parse(HOME_POD).unwrap();
        assert_eq!(home_pod.name.as_deref(), Some("HomePodMini"));
        assert_eq!(home_pod.model, "AudioAccessory5,1");
        assert_eq!(home_pod.features.0, 0x1C340405FCA00);
        assert_eq!(home_pod.source_version, "550.10");

        let tv = Profile::parse(APPLE_TV).unwrap();
        assert_eq!(tv.name.as_deref(), Some("AppleTV"));
        assert_eq!(tv.model, "AppleTV14,1");
        assert_eq!(tv.features.0, 0x3C177FDE4A7FDFD5);
        assert_eq!(tv.status_flags, 0x10644);
        assert_eq!(tv.source_version, "960.13.1");
        assert_eq!(
            tv.display,
            Some(Display {
                width: 1920,
                height: 1080,
                fps: 60,
                ..Display::default()
            })
        );
    }

    #[test]
    fn rejects_unknown_keys_and_bad_values() {
        let bad = |extra: &str| {
            Profile::parse(&format!(
                "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
                 <key>model</key><string>X</string>\
                 <key>features</key><integer>512</integer>{extra}\
                 </dict></plist>"
            ))
        };
        assert!(bad("").is_ok());
        assert!(bad("<key>magic</key><true/>").is_err());
        assert!(bad("<key>source_version</key><string>1.x</string>").is_err());
        assert!(
            bad(
                "<key>display</key><dict><key>width</key><integer>1920</integer>\
                 <key>height</key><integer>1080</integer>\
                 <key>fps</key><integer>60</integer></dict>"
            )
            .is_err(),
            "display без бита SCREEN"
        );
        assert!(bad("<key>features</key><integer>0</integer>").is_err());
        assert!(bad("<key>raop</key><string>yes</string>").is_err());
    }

    #[test]
    fn features_accept_hex_strings() {
        let p = Profile::parse(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>model</key><string>X</string>\
             <key>features</key><string>0x200</string>\
             </dict></plist>",
        )
        .unwrap();
        assert_eq!(p.features.0, Features::AUDIO);
        assert!(!p.audio_only_ui);
        assert!(p.raop);
    }
}
