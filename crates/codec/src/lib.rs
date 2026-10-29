//! Декодеры звука AirPlay: AAC-LC (буферизованный поток AirPlay 2) и ALAC
//! (realtime-поток и AirPlay 1). Чистый Rust на symphonia. AAC-ELD (звук
//! зеркалирования экрана) — через libavcodec с фичей `ffmpeg`.
//!
//! На выходе всегда интерлив i16: такой формат принимает любая звуковая
//! карта, и на слабом ARM не нужно гонять f32 через весь конвейер.

use symphonia::core::audio::{Channels, Position};
use symphonia::core::codecs::audio::well_known::{CODEC_ID_AAC, CODEC_ID_ALAC};
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoder, AudioDecoderOptions};
use symphonia::core::packet::PacketRef;
use symphonia::core::units::{Duration, Timestamp};

#[cfg(feature = "ffmpeg")]
mod eld;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("кодек не создан: {0}")]
    Init(String),
    #[error("ошибка декодирования: {0}")]
    Decode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Aac,
    Alac {
        bit_depth: u8,
        frames_per_packet: u32,
    },
    /// AAC-ELD, 480 сэмплов в пакете: звук зеркалирования экрана.
    AacEld,
}

/// Формат потока. AirPlay 2 сообщает его в SSRC каждого RTP-пакета
/// (значения из shairport-sync) или в поле `audioFormat` SETUP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub codec: Codec,
    pub sample_rate: u32,
    pub channels: u8,
}

impl Format {
    pub const ALAC_44100: Format = Format {
        codec: Codec::Alac {
            bit_depth: 16,
            frames_per_packet: 352,
        },
        sample_rate: 44100,
        channels: 2,
    };

    pub fn from_ssrc(ssrc: u32) -> Option<Format> {
        let aac = |sample_rate, channels| Format {
            codec: Codec::Aac,
            sample_rate,
            channels,
        };
        Some(match ssrc {
            0x0000_FACE => Self::ALAC_44100,
            0x1500_0000 => Format {
                codec: Codec::Alac {
                    bit_depth: 24,
                    frames_per_packet: 352,
                },
                sample_rate: 48000,
                channels: 2,
            },
            0x1600_0000 => aac(44100, 2),
            0x1700_0000 => aac(48000, 2),
            0x2700_0000 => aac(48000, 6),
            0x2800_0000 => aac(48000, 8),
            _ => return None,
        })
    }

    /// Кадров (сэмплов на канал) в одном пакете.
    pub fn frames_per_packet(&self) -> u32 {
        match self.codec {
            Codec::Aac => 1024,
            Codec::AacEld => 480,
            Codec::Alac {
                frames_per_packet, ..
            } => frames_per_packet,
        }
    }
}

pub struct Decoder {
    format: Format,
    inner: Inner,
    scratch: Vec<i16>,
}

enum Inner {
    Symphonia(Box<dyn AudioDecoder>),
    #[cfg(feature = "ffmpeg")]
    Ffmpeg(eld::Eld),
}

impl Decoder {
    pub fn new(format: Format) -> Result<Self, Error> {
        if format.codec == Codec::AacEld {
            #[cfg(feature = "ffmpeg")]
            return Ok(Self {
                format,
                inner: Inner::Ffmpeg(eld::Eld::new(&eld::ASC_44100_STEREO_480)?),
                scratch: Vec::new(),
            });
            #[cfg(not(feature = "ffmpeg"))]
            return Err(Error::Init(
                "AAC-ELD: соберите с фичей ffmpeg (нужен libavcodec)".into(),
            ));
        }
        let mut params = AudioCodecParameters::new();
        params
            .with_sample_rate(format.sample_rate)
            .with_channels(channels(format.channels));
        match format.codec {
            Codec::Aac => {
                params
                    .for_codec(CODEC_ID_AAC)
                    .with_extra_data(aac_config(format).into());
            }
            Codec::Alac {
                bit_depth,
                frames_per_packet,
            } => {
                params
                    .for_codec(CODEC_ID_ALAC)
                    .with_extra_data(alac_cookie(format, bit_depth, frames_per_packet).into());
            }
            Codec::AacEld => unreachable!("разобран выше"),
        }
        let inner = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| Error::Init(e.to_string()))?;
        Ok(Self {
            format,
            inner: Inner::Symphonia(inner),
            scratch: Vec::new(),
        })
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// Декодирует пакет и дописывает стерео i16 в `out`. Возвращает число кадров.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<usize, Error> {
        // Без фичи ffmpeg вариант один, и clippy предлагает `let`.
        #[allow(clippy::infallible_destructuring_match)]
        let inner = match &mut self.inner {
            Inner::Symphonia(d) => d,
            #[cfg(feature = "ffmpeg")]
            Inner::Ffmpeg(d) => return d.decode(packet, out),
        };
        let packet = PacketRef::new(0, Timestamp::new(0), Duration::new(1), packet);
        let buf = inner
            .decode_ref(&packet)
            .map_err(|e| Error::Decode(e.to_string()))?;
        let channels = buf.spec().channels().count().max(1);
        let frames = buf.frames();
        buf.copy_to_vec_interleaved(&mut self.scratch);
        if channels == 2 {
            out.extend_from_slice(&self.scratch);
        } else {
            for frame in self.scratch.chunks_exact(channels) {
                let (l, r) = downmix(frame);
                out.extend_from_slice(&[l, r]);
            }
        }
        Ok(frames)
    }

    pub fn reset(&mut self) {
        match &mut self.inner {
            Inner::Symphonia(d) => d.reset(),
            #[cfg(feature = "ffmpeg")]
            Inner::Ffmpeg(d) => d.reset(),
        }
    }
}

/// Сведение в стерео: моно дублируется, у многоканального берём фронт
/// и добавляем центр с −3 дБ (порядок каналов symphonia: FL, FR, FC, LFE…).
fn downmix(frame: &[i16]) -> (i16, i16) {
    match frame {
        [m] => (*m, *m),
        [l, r, c, ..] => {
            let c = *c as i32 * 181 / 256;
            let clip = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            (clip(*l as i32 + c), clip(*r as i32 + c))
        }
        _ => (0, 0),
    }
}

fn channels(n: u8) -> Channels {
    let p = match n {
        1 => Position::FRONT_CENTER,
        6 => {
            Position::FRONT_LEFT
                | Position::FRONT_RIGHT
                | Position::FRONT_CENTER
                | Position::LFE1
                | Position::REAR_LEFT
                | Position::REAR_RIGHT
        }
        8 => {
            Position::FRONT_LEFT
                | Position::FRONT_RIGHT
                | Position::FRONT_CENTER
                | Position::LFE1
                | Position::REAR_LEFT
                | Position::REAR_RIGHT
                | Position::SIDE_LEFT
                | Position::SIDE_RIGHT
        }
        _ => Position::FRONT_LEFT | Position::FRONT_RIGHT,
    };
    Channels::Positioned(p)
}

/// AudioSpecificConfig (ISO 14496-3): AAC-LC, индекс частоты, число каналов.
fn aac_config(f: Format) -> Vec<u8> {
    let freq: u16 = match f.sample_rate {
        96000 => 0,
        88200 => 1,
        64000 => 2,
        48000 => 3,
        32000 => 5,
        24000 => 6,
        22050 => 7,
        16000 => 8,
        _ => 4, // 44100
    };
    let ch: u16 = match f.channels {
        8 => 7,
        n => n as u16,
    };
    let v = (2u16 << 11) | (freq << 7) | (ch << 3);
    v.to_be_bytes().to_vec()
}

/// ALACSpecificConfig (24 байта) с параметрами по умолчанию кодера Apple.
fn alac_cookie(f: Format, bit_depth: u8, frames_per_packet: u32) -> Vec<u8> {
    let mut c = Vec::with_capacity(24);
    c.extend_from_slice(&frames_per_packet.to_be_bytes());
    c.extend_from_slice(&[0, bit_depth, 40, 10, 14, f.channels]);
    c.extend_from_slice(&255u16.to_be_bytes());
    c.extend_from_slice(&0u32.to_be_bytes());
    c.extend_from_slice(&0u32.to_be_bytes());
    c.extend_from_slice(&f.sample_rate.to_be_bytes());
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssrc_formats() {
        assert_eq!(Format::from_ssrc(0x1600_0000).unwrap().codec, Codec::Aac);
        assert_eq!(Format::from_ssrc(0xFACE), Some(Format::ALAC_44100));
        assert_eq!(Format::from_ssrc(0x1234), None);
        assert_eq!(Format::from_ssrc(0x1700_0000).unwrap().sample_rate, 48000);
    }

    #[test]
    fn aac_config_bytes() {
        // Эталон: AAC-LC 44,1 кГц стерео = 0x12 0x10.
        assert_eq!(
            aac_config(Format::from_ssrc(0x1600_0000).unwrap()),
            [0x12, 0x10]
        );
        assert_eq!(
            aac_config(Format::from_ssrc(0x1700_0000).unwrap()),
            [0x11, 0x90]
        );
    }

    #[test]
    fn decoders_initialise() {
        for ssrc in [0xFACE, 0x1500_0000, 0x1600_0000, 0x1700_0000] {
            Decoder::new(Format::from_ssrc(ssrc).unwrap()).unwrap();
        }
        // AAC 5.1/7.1 symphonia пока не умеет: такой поток отклоняем при SETUP.
        assert!(Decoder::new(Format::from_ssrc(0x2700_0000).unwrap()).is_err());
    }

    /// Несжатый кадр ALAC: заголовок с флагом «escape» и сырые сэмплы.
    #[test]
    fn alac_uncompressed_frame() {
        let mut d = Decoder::new(Format::ALAC_44100).unwrap();
        let frames = 352;
        let mut bits = BitWriter::default();
        bits.put(1, 3); // два канала (CPE)
        bits.put(0, 4);
        bits.put(0, 12);
        bits.put(0, 1); // размер кадра стандартный
        bits.put(0, 2);
        bits.put(1, 1); // escape: без сжатия
        for i in 0..frames {
            bits.put((i * 10) as u32 & 0xffff, 16);
            bits.put((-(i as i32) * 10) as u32 & 0xffff, 16);
        }
        bits.put(7, 3); // END
        let mut out = Vec::new();
        assert_eq!(d.decode(&bits.finish(), &mut out).unwrap(), frames);
        assert_eq!(out.len(), frames * 2);
        assert_eq!(&out[..4], &[0, 0, 10, -10]);
        assert_eq!(out[2 * 351], 3510);
    }

    #[test]
    fn downmix_keeps_center() {
        assert_eq!(downmix(&[100]), (100, 100));
        assert_eq!(downmix(&[100, -100, 256, 0, 0, 0]), (281, 81));
    }

    #[derive(Default)]
    struct BitWriter {
        bytes: Vec<u8>,
        acc: u64,
        n: u32,
    }

    impl BitWriter {
        fn put(&mut self, v: u32, bits: u32) {
            self.acc = (self.acc << bits) | (v as u64 & ((1u64 << bits) - 1));
            self.n += bits;
            while self.n >= 8 {
                self.n -= 8;
                self.bytes.push((self.acc >> self.n) as u8);
            }
        }
        fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                let pad = 8 - self.n;
                self.put(0, pad);
            }
            self.bytes
        }
    }
}
