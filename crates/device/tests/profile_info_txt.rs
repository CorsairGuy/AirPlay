//! Согласованность профиля, TXT и `/info` (C45).
//!
//! Профиль не доказывает совместимость: тесты проверяют только то, что
//! объявленные значения одинаковы во всех трёх местах и что несогласованный
//! plist отклоняется. Реальные ключи и устройства не используются.

use device::{DeviceInfo, Display, Features, Profile};
use plist::{Dictionary, Value};

fn info() -> DeviceInfo {
    DeviceInfo {
        name: "Kitchen".into(),
        device_id: "AA:BB:CC:DD:EE:01".into(),
        model: "AirPort10,115".into(),
        accessory_type: Some(5),
        features: Features(Features::AUDIO | Features::METADATA_TEXT),
        status_flags: 0x404,
        pairing_id: "psi-1".into(),
        public_key: [9; 32],
        source_version: "366.0".into(),
        display: None,
        audio_only_ui: true,
        enclosure_color: Some("1".into()),
        group: None,
        pgid: "pgid-1".into(),
        raop: true,
    }
}

fn get<'a>(d: &'a Dictionary, key: &str) -> &'a Value {
    d.get(key).unwrap_or_else(|| panic!("нет ключа {key}"))
}

fn dict_of(info: &DeviceInfo) -> Dictionary {
    let value = plist::from_bytes::<Value>(&info.info_plist()).expect("plist разбирается");
    value.as_dictionary().expect("словарь").clone()
}

#[test]
fn info_features_match_the_advertised_txt_words() {
    let info = info();
    let dict = dict_of(&info);
    let features = get(&dict, "features").as_unsigned_integer().unwrap() as u64;
    assert_eq!(features, info.features.0);

    // featuresEx — те же биты little-endian в base64 без паддинга.
    assert_eq!(
        get(&dict, "featuresEx").as_string(),
        Some(info.fex().as_str()),
        "featuresEx не совпадает с битами features"
    );

    // TXT-запись внутри /info содержит ту же маску и тот же идентификатор.
    let record = get(&dict, "txtAirPlay").as_data().unwrap();
    let text = String::from_utf8_lossy(record);
    assert!(
        text.contains(&format!("features={}", info.features.to_txt())),
        "TXT features не совпадает: {text}"
    );
    assert!(text.contains("psi=psi-1"));
    assert!(text.contains("srcvers=366.0"));
    assert!(text.contains("model=AirPort10,115"));
}

#[test]
fn info_model_and_versions_come_from_the_same_fields() {
    let info = info();
    let dict = dict_of(&info);
    assert_eq!(get(&dict, "model").as_string(), Some("AirPort10,115"));
    assert_eq!(get(&dict, "sourceVersion").as_string(), Some("366.0"));
    assert_eq!(get(&dict, "psi").as_string(), Some("psi-1"));
    assert_eq!(get(&dict, "deviceID").as_string(), Some("AA:BB:CC:DD:EE:01"));
    assert_eq!(get(&dict, "accessoryType").as_unsigned_integer(), Some(5));
    assert_eq!(get(&dict, "statusFlags").as_unsigned_integer(), Some(0x404));
    let caps = get(&dict, "playbackCapabilities").as_dictionary().unwrap();
    assert_eq!(
        get(caps, "supportsUIForAudioOnlyContent").as_boolean(),
        Some(true)
    );
    assert!(
        dict.get("displays").is_none(),
        "экран без профиля не объявляется"
    );
}

#[test]
fn display_in_info_matches_the_profile_geometry() {
    let mut info = info();
    info.features = Features(Features::AUDIO | Features::SCREEN);
    info.display = Some(Display {
        width: 1920,
        height: 1080,
        fps: 60,
        ..Display::default()
    });
    let dict = dict_of(&info);
    let displays = get(&dict, "displays").as_array().unwrap();
    let display = displays[0].as_dictionary().unwrap();
    assert_eq!(get(display, "width").as_unsigned_integer(), Some(1920));
    assert_eq!(get(display, "heightPixels").as_unsigned_integer(), Some(1080));
    assert_eq!(get(display, "maxFPS").as_unsigned_integer(), Some(60));
}

#[test]
fn profile_rejects_inconsistent_feature_and_display_combinations() {
    let base = "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
                <key>model</key><string>X</string>{body}</dict></plist>";
    // SCREEN без display и display без SCREEN — оба несогласованы.
    assert!(
        Profile::parse(&base.replace("{body}", "<key>features</key><integer>128</integer>"))
            .is_err()
    );
    assert!(
        Profile::parse(&base.replace(
            "{body}",
            "<key>features</key><integer>512</integer><key>display</key><dict>\
             <key>width</key><integer>1920</integer><key>height</key><integer>1080</integer>\
             <key>fps</key><integer>60</integer></dict>"
        ))
        .is_err()
    );
    // Правильная пара принимается.
    let ok = Profile::parse(&base.replace(
        "{body}",
        "<key>features</key><integer>640</integer><key>display</key><dict>\
         <key>width</key><integer>1920</integer><key>height</key><integer>1080</integer>\
         <key>fps</key><integer>60</integer></dict>",
    ))
    .unwrap();
    assert_eq!(ok.features.0, Features::AUDIO | Features::SCREEN);
    assert_eq!(ok.display.unwrap().height, 1080);
}

#[test]
fn missing_and_limit_values_are_reported_not_defaulted_silently() {
    // Нет model — отказ; нет features — отказ.
    assert!(
        Profile::parse(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>features</key><integer>512</integer></dict></plist>"
        )
        .is_err()
    );
    assert!(
        Profile::parse(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>model</key><string>X</string></dict></plist>"
        )
        .is_err()
    );
    assert!(
        Profile::parse(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>model</key><string>X</string>\
             <key>features</key><string>0x200</string>\
             <key>status_flags</key><integer>4294967296</integer></dict></plist>"
        )
        .is_err(),
        "status_flags больше u32 должен отклоняться"
    );

    // Верхняя граница имени: 64 байта принимается, 65 — нет.
    let name64 = "a".repeat(64);
    assert!(
        Profile::parse(&format!(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>model</key><string>X</string><key>features</key><integer>512</integer>\
             <key>name</key><string>{name64}</string></dict></plist>"
        ))
        .is_ok()
    );
    let name65 = "a".repeat(65);
    assert!(
        Profile::parse(&format!(
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict>\
             <key>model</key><string>X</string><key>features</key><integer>512</integer>\
             <key>name</key><string>{name65}</string></dict></plist>"
        ))
        .is_err()
    );
}
