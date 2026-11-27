//! Биты возможностей: объявленная маска против реализованных путей (C56).
//!
//! Проверяется только консистентность констант и сериализации. Наличие бита в
//! профиле не означает, что соответствующий протокольный путь реализован: это
//! требует отдельной проверки.

use device::Features;

const DOCUMENTED: &[(&str, u64)] = &[
    ("VIDEO", Features::VIDEO),
    ("PHOTO", Features::PHOTO),
    ("SCREEN", Features::SCREEN),
    ("AUDIO", Features::AUDIO),
    ("AUDIO_REDUNDANT", Features::AUDIO_REDUNDANT),
    ("FAIRPLAY_AUTH", Features::FAIRPLAY_AUTH),
    ("METADATA_PROGRESS", Features::METADATA_PROGRESS),
    ("METADATA_ARTWORK", Features::METADATA_ARTWORK),
    ("METADATA_TEXT", Features::METADATA_TEXT),
    ("AUDIO_FORMATS_1", Features::AUDIO_FORMATS_1),
    ("AUDIO_FORMATS_2", Features::AUDIO_FORMATS_2),
    ("AUDIO_FORMATS_3", Features::AUDIO_FORMATS_3),
    ("AUDIO_FORMATS_4", Features::AUDIO_FORMATS_4),
    ("UNIFIED_ADVERTISER_INFO", Features::UNIFIED_ADVERTISER_INFO),
    ("UNIFIED_MEDIA_CONTROL", Features::UNIFIED_MEDIA_CONTROL),
    ("BUFFERED_AUDIO", Features::BUFFERED_AUDIO),
    ("PTP", Features::PTP),
    ("SCREEN_MULTI_CODEC", Features::SCREEN_MULTI_CODEC),
    (
        "HK_PAIRING_AND_ACCESS_CONTROL",
        Features::HK_PAIRING_AND_ACCESS_CONTROL,
    ),
    ("SYSTEM_PAIRING", Features::SYSTEM_PAIRING),
    (
        "COREUTILS_PAIRING_AND_ENCRYPTION",
        Features::COREUTILS_PAIRING_AND_ENCRYPTION,
    ),
];

#[test]
fn every_documented_bit_is_single_and_unique() {
    for (name, mask) in DOCUMENTED {
        assert!(mask.is_power_of_two(), "{name} = {mask:#x} не один бит");
    }
    for (i, (a, mask_a)) in DOCUMENTED.iter().enumerate() {
        for (b, mask_b) in &DOCUMENTED[i + 1..] {
            assert_ne!(mask_a, mask_b, "{a} и {b} делят один бит");
        }
    }
}

#[test]
fn txt_encoding_splits_low_and_high_words_without_loss() {
    let mask = Features(0xDEAD_BEEF_1234_5678);
    assert_eq!(mask.to_txt(), "0x12345678,0xDEADBEEF");
    for features in [
        Features(0),
        Features(Features::AUDIO),
        Features(u64::MAX),
        Features::MIRROR_RECEIVER,
        Features::MIRROR_RECEIVER_HEVC,
    ] {
        let txt = features.to_txt();
        let (low, high) = txt.split_once(',').expect("два слова");
        let low = u32::from_str_radix(low.trim_start_matches("0x"), 16).unwrap() as u64;
        let high = u32::from_str_radix(high.trim_start_matches("0x"), 16).unwrap() as u64;
        assert_eq!(low | (high << 32), features.0, "{txt}");
    }
}

#[test]
fn built_in_mirror_mask_preserves_its_current_bits() {
    // This checks the built-in mask, not externally supplied device profiles.
    let mask = Features::MIRROR_RECEIVER.0;
    for bit in [27u64, 40, 41, 48] {
        assert_eq!(mask & (1 << bit), 0, "MIRROR_RECEIVER выставляет бит {bit}");
    }
    // HEVC добавляет ровно бит SCREEN_MULTI_CODEC и ничего больше.
    assert_eq!(
        Features::MIRROR_RECEIVER_HEVC.0 ^ Features::MIRROR_RECEIVER.0,
        Features::SCREEN_MULTI_CODEC
    );
}

#[test]
fn audio_receiver_mask_contains_its_documented_core_bits() {
    let mask = Features::AUDIO_RECEIVER.0;
    for bit in [
        Features::AUDIO,
        Features::AUDIO_REDUNDANT,
        Features::METADATA_TEXT,
        Features::METADATA_ARTWORK,
        Features::METADATA_PROGRESS,
        Features::BUFFERED_AUDIO,
        Features::PTP,
    ] {
        assert_ne!(mask & bit, 0, "AUDIO_RECEIVER без бита {bit:#x}");
    }
    assert_eq!(mask & Features::SCREEN, 0, "аудио-профиль не объявляет экран");
}
