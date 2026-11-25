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

/// Предел координат тачпада: обе оси в диапазоне `0..=1000`, начало — левый
/// верхний угол (pyatv, `docs/documentation/protocols.md`).
pub const TOUCHPAD_MAX: i64 = 1000;

/// Фазы касания в `_hidT._tPh`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Press,
    Hold,
    Release,
    Click,
}

impl TouchPhase {
    pub fn value(self) -> i64 {
        match self {
            Self::Press => 1,
            Self::Hold => 3,
            Self::Release => 4,
            Self::Click => 5,
        }
    }
}

/// Открытие тачпада: приёмник отвечает идентификатором устройства касания.
/// Размер в запросе — `Double`, как в захватах pyatv.
pub fn touch_start(xid: u32) -> Value {
    request(
        "_touchStart",
        Value::object(vec![
            ("_height", Value::Double(TOUCHPAD_MAX as f64)),
            ("_width", Value::Double(TOUCHPAD_MAX as f64)),
            ("_tFl", Value::Int(0)),
        ]),
        xid,
    )
}

pub fn touch_stop(xid: u32) -> Value {
    request("_touchStop", Value::empty_object(), xid)
}

/// Событие касания. `_t = 1` (event), но `_x` присутствует — так его присылают
/// захваты pyatv, в отличие от [`event`] без идентификатора обмена.
/// `ns` — наносекунды с момента установления соединения. Координаты вне
/// `0..=1000` прижимаются к границе, а не отбрасываются.
pub fn touch(x: i64, y: i64, phase: TouchPhase, ns: u64, xid: u32) -> Value {
    let x = x.clamp(0, TOUCHPAD_MAX);
    let y = y.clamp(0, TOUCHPAD_MAX);
    Value::object(vec![
        ("_i", Value::Str("_hidT".into())),
        ("_t", Value::Int(EVENT)),
        (
            "_c",
            Value::object(vec![
                ("_ns", Value::Int(ns as i64)),
                ("_tFg", Value::Int(1)),
                ("_tPh", Value::Int(phase.value())),
                ("_cx", Value::Int(x)),
                ("_cy", Value::Int(y)),
            ]),
        ),
        ("_x", Value::Int(xid as i64)),
    ])
}

/// Тап: нажатие и отпускание Select плюс событие `Click` в правом нижнем углу.
/// Возвращает три значения; идентификаторы обмена идут подряд от `xid`.
pub fn tap(xid: u32, ns: u64) -> [Value; 3] {
    [
        hid(true, HidCommand::Select, xid),
        hid(false, HidCommand::Select, xid.wrapping_add(1)),
        touch(
            TOUCHPAD_MAX,
            TOUCHPAD_MAX,
            TouchPhase::Click,
            ns,
            xid.wrapping_add(2),
        ),
    ]
}

/// Точки свайпа от `from` к `to` включительно (`steps` — число промежутков,
/// не считая начальной). Промежуточные округляются, концы сохраняются.
pub fn swipe_points(from: (i64, i64), to: (i64, i64), steps: u32) -> Vec<(i64, i64)> {
    let steps = steps.max(1) as i64;
    let mut points = Vec::with_capacity(steps as usize + 1);
    for step in 0..=steps {
        let x = from.0 + (to.0 - from.0) * step / steps;
        let y = from.1 + (to.1 - from.1) * step / steps;
        points.push((x.clamp(0, TOUCHPAD_MAX), y.clamp(0, TOUCHPAD_MAX)));
    }
    points
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
    fn touch_event_has_phases_and_coordinates() {
        let value = touch(120, 980, TouchPhase::Hold, 7_130_181_240_917, 9);
        assert_eq!(value.get("_i").and_then(Value::as_str), Some("_hidT"));
        assert_eq!(value.get("_t").and_then(Value::as_int), Some(EVENT));
        assert_eq!(value.get("_x").and_then(Value::as_int), Some(9));
        let content = value.get("_c").unwrap();
        assert_eq!(content.get("_tFg").and_then(Value::as_int), Some(1));
        assert_eq!(content.get("_tPh").and_then(Value::as_int), Some(3));
        assert_eq!(content.get("_cx").and_then(Value::as_int), Some(120));
        assert_eq!(content.get("_cy").and_then(Value::as_int), Some(980));
        assert_eq!(
            content.get("_ns").and_then(Value::as_int),
            Some(7_130_181_240_917)
        );
    }

    #[test]
    fn touch_phases_match_the_wire_values() {
        assert_eq!(TouchPhase::Press.value(), 1);
        assert_eq!(TouchPhase::Hold.value(), 3);
        assert_eq!(TouchPhase::Release.value(), 4);
        assert_eq!(TouchPhase::Click.value(), 5);
    }

    #[test]
    fn touch_coordinates_are_clamped() {
        for (x, y) in [(-5, 5000), (5000, -5)] {
            let content = touch(x, y, TouchPhase::Press, 0, 1);
            let content = content.get("_c").unwrap();
            let cx = content.get("_cx").and_then(Value::as_int).unwrap();
            let cy = content.get("_cy").and_then(Value::as_int).unwrap();
            assert!((0..=TOUCHPAD_MAX).contains(&cx));
            assert!((0..=TOUCHPAD_MAX).contains(&cy));
        }
    }

    #[test]
    fn touch_start_declares_a_thousand_unit_pad() {
        let value = touch_start(5);
        assert_eq!(value.get("_i").and_then(Value::as_str), Some("_touchStart"));
        let content = value.get("_c").unwrap();
        assert_eq!(content.get("_width"), Some(&Value::Double(1000.0)));
        assert_eq!(content.get("_height"), Some(&Value::Double(1000.0)));
        assert_eq!(content.get("_tFl").and_then(Value::as_int), Some(0));
        assert_eq!(
            touch_stop(5).get("_i").and_then(Value::as_str),
            Some("_touchStop")
        );
    }

    #[test]
    fn tap_is_select_down_up_and_click() {
        let [down, up, click] = tap(10, 1);
        assert_eq!(
            down.get("_c").unwrap().get("_hBtS").and_then(Value::as_int),
            Some(1)
        );
        assert_eq!(
            up.get("_c").unwrap().get("_hBtS").and_then(Value::as_int),
            Some(2)
        );
        assert_eq!(up.get("_x").and_then(Value::as_int), Some(11));
        assert_eq!(click.get("_x").and_then(Value::as_int), Some(12));
        let content = click.get("_c").unwrap();
        assert_eq!(content.get("_tPh").and_then(Value::as_int), Some(5));
        assert_eq!(content.get("_cx").and_then(Value::as_int), Some(1000));
        assert_eq!(content.get("_cy").and_then(Value::as_int), Some(1000));
    }

    #[test]
    fn swipe_points_hit_both_ends() {
        let points = swipe_points((500, 800), (100, 200), 4);
        assert_eq!(points.len(), 5);
        assert_eq!(points[0], (500, 800));
        assert_eq!(*points.last().unwrap(), (100, 200));
        assert_eq!(points[2], (300, 500));
        assert_eq!(swipe_points((0, 0), (0, 0), 0).len(), 2);
    }

    #[test]
    fn system_status_mapping() {
        assert_eq!(SystemStatus::from_int(1).is_on(), Some(false));
        assert_eq!(SystemStatus::from_int(3).is_on(), Some(true));
        assert_eq!(SystemStatus::from_int(9).is_on(), None);
    }
}
