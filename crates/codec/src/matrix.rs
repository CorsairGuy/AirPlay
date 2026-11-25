//! Матрица аудиокодеков AirPlay: что приёмник принимает и что объявляет.
//!
//! Источники значений — `docs/AIRPLAY-WIRE-SPEC.md` §C-MEDIA (проверенные
//! пути `ct`: 1 PCM, 2 ALAC, 4 AAC-LC, 8 AAC-ELD) и SSRC-таблица
//! shairport-sync в [`crate::Format::from_ssrc`]. Это wire-факты, а не
//! подтверждение совместимости: реальные отправители не проверялись.
//!
//! Матрица намеренно узкая. Она отделяет «структурно разбираем» от
//! «объявлено поддержанным»: 24-бит ALAC, 48 кГц и многоканальность
//! разбираются, но не рекламируются, пока нет аппаратной проверки (D82).

use crate::{Codec, Format};

/// `ct` — compression type из SETUP/ANNOUNCE.
pub mod compression {
    pub const PCM: u64 = 1;
    pub const ALAC: u64 = 2;
    pub const AAC_LC: u64 = 4;
    pub const AAC_ELD: u64 = 8;
}

/// Параметры, которые отправитель сообщает для потока.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advertisement {
    /// `ct`.
    pub compression: u64,
    /// `ss` — бит на сэмпл; по умолчанию 16.
    pub sample_bits: u64,
    /// `sr` — сэмплов в секунду; по умолчанию 44100.
    pub sample_rate: u64,
    /// `spf` — кадров в пакете; по умолчанию 352.
    pub frames_per_packet: u64,
}

impl Default for Advertisement {
    fn default() -> Self {
        Self {
            compression: compression::ALAC,
            sample_bits: 16,
            sample_rate: 44_100,
            frames_per_packet: 352,
        }
    }
}

impl Advertisement {
    /// Разбирает словарь SETUP. Отсутствующие поля получают значения по
    /// умолчанию, как в прежнем разборе realtime-потока.
    pub fn from_fields(
        compression: Option<u64>,
        sample_bits: Option<u64>,
        sample_rate: Option<u64>,
        frames_per_packet: Option<u64>,
    ) -> Self {
        Self {
            compression: compression.unwrap_or(compression::ALAC),
            sample_bits: sample_bits.unwrap_or(16),
            sample_rate: sample_rate.unwrap_or(44_100),
            frames_per_packet: frames_per_packet.unwrap_or(352),
        }
    }
}

/// Почему поток не принят.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// Неизвестный `ct`.
    UnknownCompression(u64),
    /// AAC-ELD (звук зеркалирования) — нужен libavcodec, в realtime-путь не
    /// входит.
    AacEld,
    /// PCM только 16 бит: другая разрядность в этом пути не проверялась.
    PcmBitDepth(u64),
    /// Ноль кадров в пакете или нулевая частота — поток нельзя нарезать.
    EmptyStream,
    InvalidParameter,
}

impl Rejection {
    pub fn reason(self) -> String {
        match self {
            Rejection::UnknownCompression(ct) => format!("неизвестный кодек ct={ct}"),
            Rejection::AacEld => "AAC-ELD пока не поддерживается".into(),
            Rejection::PcmBitDepth(bits) => {
                format!("PCM {bits} бит не поддерживается (только 16)")
            }
            Rejection::EmptyStream => "пустой формат потока (spf=0 или sr=0)".into(),
            Rejection::InvalidParameter => "недопустимые параметры аудиопотока".into(),
        }
    }
}

/// Формат realtime-потока по объявлению отправителя.
pub fn format(ad: Advertisement) -> Result<Format, Rejection> {
    if ad.frames_per_packet == 0 || ad.sample_rate == 0 {
        return Err(Rejection::EmptyStream);
    }
    if !(8_000..=384_000).contains(&ad.sample_rate) || ad.frames_per_packet > 8192 || ad.sample_bits > 32 {
        return Err(Rejection::InvalidParameter);
    }
    let codec = match ad.compression {
        compression::PCM if ad.sample_bits == 16 => Codec::Pcm {
            frames_per_packet: ad.frames_per_packet as u32,
        },
        compression::PCM => return Err(Rejection::PcmBitDepth(ad.sample_bits)),
        compression::ALAC if matches!(ad.sample_bits, 16 | 20 | 24 | 32) => Codec::Alac {
            bit_depth: ad.sample_bits as u8,
            frames_per_packet: ad.frames_per_packet as u32,
        },
        compression::ALAC => return Err(Rejection::InvalidParameter),
        compression::AAC_LC => Codec::Aac,
        compression::AAC_ELD => return Err(Rejection::AacEld),
        other => return Err(Rejection::UnknownCompression(other)),
    };
    Ok(Format {
        codec,
        sample_rate: ad.sample_rate as u32,
        channels: 2,
    })
}

/// Что приёмник объявляет в профиле/feature-битах.
///
/// Только проверенные пути: PCM16, ALAC16, AAC-LC. AAC-ELD живёт в
/// зеркалировании, 24-бит и 48 кГц разбираются, но не рекламируются
/// (D82) — объявлять их без аппаратной проверки нельзя.
pub const ADVERTISED: [u64; 3] = [compression::PCM, compression::ALAC, compression::AAC_LC];

/// Разбирается ли `ct` структурно (не значит «объявлен»).
pub fn is_parsed(compression: u64) -> bool {
    matches!(
        compression,
        compression::PCM | compression::ALAC | compression::AAC_LC | compression::AAC_ELD
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alac16() -> Advertisement {
        Advertisement {
            compression: compression::ALAC,
            sample_bits: 16,
            sample_rate: 44_100,
            frames_per_packet: 352,
        }
    }

    #[test]
    fn defaults_match_the_realtime_stream() {
        let ad = Advertisement::from_fields(None, None, None, None);
        assert_eq!(ad, alac16());
        assert_eq!(format(ad).unwrap(), Format::ALAC_44100);
    }

    #[test]
    fn golden_field_combinations() {
        // ct=1, 16 бит: несжатый PCM, как у наших отправителей.
        let pcm = Advertisement::from_fields(Some(1), Some(16), Some(44_100), Some(352));
        assert_eq!(
            format(pcm).unwrap().codec,
            Codec::Pcm {
                frames_per_packet: 352
            }
        );

        // ct=2, 24 бита, 48 кГц: структурно разбирается (D82 — не объявлено).
        let hi = Advertisement::from_fields(Some(2), Some(24), Some(48_000), Some(352));
        let high = format(hi).unwrap();
        assert_eq!(
            high.codec,
            Codec::Alac {
                bit_depth: 24,
                frames_per_packet: 352
            }
        );
        assert_eq!(high.sample_rate, 48_000);

        // ct=4: AAC-LC, размер кадра задаёт кодек, а не spf.
        let aac = Advertisement::from_fields(Some(4), None, Some(44_100), Some(1024));
        let aac_format = format(aac).unwrap();
        assert_eq!(aac_format.codec, Codec::Aac);
        assert_eq!(aac_format.frames_per_packet(), 1024);
    }

    #[test]
    fn unsupported_paths_have_specific_reasons() {
        let eld = Advertisement::from_fields(Some(8), None, None, Some(480));
        assert_eq!(format(eld), Err(Rejection::AacEld));
        assert_eq!(eld.sample_bits, 16);

        let pcm24 = Advertisement::from_fields(Some(1), Some(24), None, None);
        assert_eq!(format(pcm24), Err(Rejection::PcmBitDepth(24)));

        let unknown = Advertisement::from_fields(Some(9), None, None, None);
        assert_eq!(format(unknown), Err(Rejection::UnknownCompression(9)));
        assert!(Rejection::UnknownCompression(9).reason().contains("ct=9"));

        let empty = Advertisement::from_fields(Some(2), Some(16), Some(44_100), Some(0));
        assert_eq!(format(empty), Err(Rejection::EmptyStream));
        let wrapped = Advertisement::from_fields(Some(258), Some(16), Some(44_100), Some(352));
        assert_eq!(format(wrapped), Err(Rejection::UnknownCompression(258)));
        for ad in [
            Advertisement::from_fields(Some(2), Some(272), Some(44_100), Some(352)),
            Advertisement::from_fields(Some(2), Some(16), Some(u32::MAX as u64 + 44_101), Some(352)),
            Advertisement::from_fields(Some(2), Some(16), Some(44_100), Some(8193)),
        ] {
            assert_eq!(format(ad), Err(Rejection::InvalidParameter));
        }
    }

    #[test]
    fn advertised_set_is_narrower_than_parsed_set() {
        assert_eq!(ADVERTISED, [1, 2, 4]);
        for ct in ADVERTISED {
            assert!(is_parsed(ct));
        }
        assert!(is_parsed(compression::AAC_ELD));
        assert!(!ADVERTISED.contains(&compression::AAC_ELD));
        assert!(!is_parsed(9));
    }

    #[test]
    fn ssrc_table_covers_the_advertised_codecs() {
        // Каждый объявленный кодек имеет хотя бы один SSRC-профиль.
        let profiles = [
            0x0000_FACE,
            0x1500_0000,
            0x1600_0000,
            0x1700_0000,
            0x2700_0000,
            0x2800_0000,
        ];
        let codecs: Vec<Codec> = profiles
            .iter()
            .map(|ssrc| Format::from_ssrc(*ssrc).expect("известный SSRC").codec)
            .map(|codec| match codec {
                Codec::Aac => Codec::Aac,
                Codec::Alac { .. } => Codec::Alac {
                    bit_depth: 0,
                    frames_per_packet: 0,
                },
                other => other,
            })
            .collect();
        assert!(codecs.contains(&Codec::Aac));
        assert!(codecs.contains(&Codec::Alac {
            bit_depth: 0,
            frames_per_packet: 0
        }));
        assert_eq!(profiles.len(), 6);
        assert!(Format::from_ssrc(0x1234_5678).is_none());
    }
}
