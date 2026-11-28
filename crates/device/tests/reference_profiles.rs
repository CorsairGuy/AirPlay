//! D11/D12: обезличенные reference fixtures не являются готовыми Profile.
//! Проверяется, что сохранены только подтверждённые поля и нет чужих ID/ключей.

use std::path::PathBuf;

use device::Profile;

fn load(name: &str) -> plist::Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/interop/fixtures")
        .join(name);
    plist::Value::from_file(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn appletv_reference_keeps_observed_fields_and_is_not_a_runtime_profile() {
    let value = load("appletv14-1.reference.plist");
    let d = value.as_dictionary().unwrap();
    assert_eq!(d["model"].as_string(), Some("AppleTV14,1"));
    assert_eq!(d["source_version"].as_string(), Some("960.13.1"));
    assert_eq!(d["features_hex"].as_string(), Some("0x3C177FDE4A7FDFD5"));
    assert_eq!(d["status_flags_hex"].as_string(), Some("0x10644"));
    assert_eq!(d["has_udp_mirroring_support"].as_boolean(), Some(true));
    assert_eq!(d["anonymized"].as_boolean(), Some(true));
    for forbidden in ["deviceID", "pk", "psi", "pi", "gid", "name"] {
        assert!(
            !d.contains_key(forbidden),
            "reference fixture не должен содержать {forbidden}"
        );
    }
    // Сырое наблюдение не выдаём за профиль: маска строкой, display нет.
    assert!(
        Profile::from_value(&value).is_err(),
        "AppleTV reference не является готовым Profile"
    );
}

#[test]
fn homepod_reference_does_not_invent_a_feature_mask() {
    let value = load("homepodmini.reference.plist");
    let d = value.as_dictionary().unwrap();
    assert_eq!(d["model"].as_string(), Some("AudioAccessory5,1"));
    assert_eq!(d["source_version"].as_string(), Some("550.10"));
    assert_eq!(d["features_hex"].as_string(), Some("unknown"));
    assert_eq!(d["anonymized"].as_boolean(), Some(true));
    for forbidden in ["deviceID", "pk", "psi", "pi", "gid", "name"] {
        assert!(!d.contains_key(forbidden), "лишнее поле {forbidden}");
    }
    assert!(
        Profile::from_value(&value).is_err(),
        "без подтверждённой маски профиль не запускается"
    );
}
