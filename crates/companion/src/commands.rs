//! Команды Companion Link в том виде, в каком их ждёт приёмник:
//! конверт `{_i, _t, _c, _x}` и конкретные идентификаторы.

use crate::opack::Value;

/// Типы сообщений в конверте `_t`.
pub const EVENT: i64 = 1;
pub const REQUEST: i64 = 2;
pub const RESPONSE: i64 = 3;

/// Сервис, который объявляет пульт в `_sessionStart`.
pub const TV_REMOTE_SERVICES: &str = "com.apple.tvremoteservices";

/// HID-команды пульта.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum HidCommand {
    Up = 1,
    Down = 2,
    Left = 3,
    Right = 4,
    Menu = 5,
    Select = 6,
    Home = 7,
    VolumeUp = 8,
    VolumeDown = 9,
    Siri = 10,
    Screensaver = 11,
    Sleep = 12,
    Wake = 13,
    PlayPause = 14,
    ChannelIncrement = 15,
    ChannelDecrement = 16,
    Guide = 17,
    PageUp = 18,
    PageDown = 19,
}

/// Команды управления воспроизведением.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MediaControlCommand {
    Play = 1,
    Pause = 2,
    NextTrack = 3,
    PreviousTrack = 4,
    GetVolume = 5,
    SetVolume = 6,
    SkipBy = 7,
    FastForwardBegin = 8,
    FastForwardEnd = 9,
    RewindBegin = 10,
    RewindEnd = 11,
    GetCaptionSettings = 12,
    SetCaptionSettings = 13,
}

/// Состояние устройства из `FetchAttentionState`/`SystemStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemStatus {
    Asleep,
    Screensaver,
    Awake,
    Idle,
    Unknown(i64),
}

impl SystemStatus {
    pub fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Asleep,
            2 => Self::Screensaver,
            3 => Self::Awake,
            4 => Self::Idle,
            other => Self::Unknown(other),
        }
    }

    /// Включено ли устройство (экран/питание) по этому состоянию.
    pub fn is_on(self) -> Option<bool> {
        match self {
            Self::Asleep => Some(false),
            Self::Screensaver | Self::Awake | Self::Idle => Some(true),
            Self::Unknown(_) => None,
        }
    }
}

/// Конверт запроса. `_x` — идентификатор обмена, по нему приходит ответ.
pub fn request(identifier: &str, content: Value, xid: u32) -> Value {
    Value::object(vec![
        ("_i", Value::Str(identifier.into())),
        ("_t", Value::Int(REQUEST)),
        ("_c", content),
        ("_x", Value::Int(xid as i64)),
    ])
}

/// Конверт события (без `_x`, ответа не ждут).
pub fn event(identifier: &str, content: Value) -> Value {
    Value::object(vec![
        ("_i", Value::Str(identifier.into())),
        ("_t", Value::Int(EVENT)),
        ("_c", content),
    ])
}

pub fn session_start(xid: u32, local_sid: u32) -> Value {
    request(
        "_sessionStart",
        Value::object(vec![
            ("_srvT", Value::Str(TV_REMOTE_SERVICES.into())),
            ("_sid", Value::Int(local_sid as i64)),
        ]),
        xid,
    )
}

pub fn session_stop(xid: u32, sid: u64) -> Value {
    request(
        "_sessionStop",
        Value::object(vec![
            ("_srvT", Value::Str(TV_REMOTE_SERVICES.into())),
            ("_sid", Value::Int(sid as i64)),
        ]),
        xid,
    )
}

/// Открытие сессии TV Remote Client (tvOS 26 требует её до `FetchAttentionState`).
pub fn tvrc_session_start(xid: u32) -> Value {
    request(
        "TVRCSessionStart",
        Value::object(vec![("ProtocolVersionKey", Value::Str("1.2".into()))]),
        xid,
    )
}

/// Нажатие (`down = true`) или отпускание HID-кнопки.
pub fn hid(down: bool, command: HidCommand, xid: u32) -> Value {
    request(
        "_hidC",
        Value::object(vec![
            ("_hBtS", Value::Int(if down { 1 } else { 2 })),
            ("_hidC", Value::Int(command as u8 as i64)),
        ]),
        xid,
    )
}

pub fn media_control(command: MediaControlCommand, xid: u32) -> Value {
    request(
        "_mcc",
        Value::object(vec![("_mcc", Value::Int(command as u8 as i64))]),
        xid,
    )
}

/// Громкость: доля 0.0..=1.0 (проценты переводит вызывающий).
pub fn set_volume(level: f64, xid: u32) -> Value {
    request(
        "_mcc",
        Value::object(vec![
            (
                "_mcc",
                Value::Int(MediaControlCommand::SetVolume as u8 as i64),
            ),
            ("_vol", Value::Double(level)),
        ]),
        xid,
    )
}

/// Перемотка на `seconds` (отрицательное значение — назад).
pub fn skip_by(seconds: f64, xid: u32) -> Value {
    request(
        "_mcc",
        Value::object(vec![
            ("_mcc", Value::Int(MediaControlCommand::SkipBy as u8 as i64)),
            ("_skpS", Value::Double(seconds)),
        ]),
        xid,
    )
}

pub fn text_input_start(xid: u32) -> Value {
    request("_tiStart", Value::empty_object(), xid)
}

pub fn text_input_stop(xid: u32) -> Value {
    request("_tiStop", Value::empty_object(), xid)
}

pub fn fetch_attention_state(xid: u32) -> Value {
    request("FetchAttentionState", Value::empty_object(), xid)
}

/// Подписка на событие (`_iMC`, `SystemStatus`, `TVSystemStatus`, ...).
pub fn subscribe(event_name: &str, xid: u32) -> Value {
    request(
        "_interest",
        Value::object(vec![(
            "_regEvents",
            Value::Array(vec![Value::Str(event_name.into())]),
        )]),
        xid,
    )
}

pub fn unsubscribe(event_name: &str, xid: u32) -> Value {
    request(
        "_interest",
        Value::object(vec![(
            "_deregEvents",
            Value::Array(vec![Value::Str(event_name.into())]),
        )]),
        xid,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opack;

    #[test]
    fn request_envelope_roundtrips() {
        let value = hid(true, HidCommand::Select, 42);
        assert_eq!(value.get("_i").and_then(Value::as_str), Some("_hidC"));
        assert_eq!(value.get("_t").and_then(Value::as_int), Some(REQUEST));
        assert_eq!(value.get("_x").and_then(Value::as_int), Some(42));
        let content = value.get("_c").unwrap();
        assert_eq!(content.get("_hBtS").and_then(Value::as_int), Some(1));
        assert_eq!(content.get("_hidC").and_then(Value::as_int), Some(6));

        let bytes = opack::encode(&value).unwrap();
        assert_eq!(opack::unpack(&bytes).unwrap(), value);
    }

    #[test]
    fn press_and_release_differ() {
        let down = hid(true, HidCommand::Home, 1);
        let up = hid(false, HidCommand::Home, 2);
        assert_eq!(
            down.get("_c").unwrap().get("_hBtS").and_then(Value::as_int),
            Some(1)
        );
        assert_eq!(
            up.get("_c").unwrap().get("_hBtS").and_then(Value::as_int),
            Some(2)
        );
    }

    #[test]
    fn media_and_volume_payloads() {
        let play = media_control(MediaControlCommand::Play, 3);
        assert_eq!(
            play.get("_c").unwrap().get("_mcc").and_then(Value::as_int),
            Some(1)
        );

        let volume = set_volume(0.4, 4);
        let content = volume.get("_c").unwrap();
        assert_eq!(content.get("_mcc").and_then(Value::as_int), Some(6));
        assert_eq!(content.get("_vol"), Some(&Value::Double(0.4)));

        let skip = skip_by(-10.0, 5);
        assert_eq!(
            skip.get("_c").unwrap().get("_skpS"),
            Some(&Value::Double(-10.0))
        );
    }

    #[test]
    fn events_have_no_xid() {
        let value = event("_hidT", Value::object(vec![("_tFg", Value::Int(1))]));
        assert_eq!(value.get("_t").and_then(Value::as_int), Some(EVENT));
        assert!(value.get("_x").is_none());
    }

    #[test]
    fn session_start_shape() {
        let value = session_start(1, 0xDEAD_BEEF);
        assert_eq!(
            value.get("_i").and_then(Value::as_str),
            Some("_sessionStart")
        );
        let content = value.get("_c").unwrap();
        assert_eq!(
            content.get("_srvT").and_then(Value::as_str),
            Some("com.apple.tvremoteservices")
        );
        assert_eq!(
            content.get("_sid").and_then(Value::as_int),
            Some(0xDEAD_BEEF)
        );
    }

    #[test]
    fn system_status_mapping() {
        assert_eq!(SystemStatus::from_int(1).is_on(), Some(false));
        assert_eq!(SystemStatus::from_int(3).is_on(), Some(true));
        assert_eq!(SystemStatus::from_int(9).is_on(), None);
    }
}
