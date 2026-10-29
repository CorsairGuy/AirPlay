//! AAC-ELD через libavcodec: symphonia его не умеет, а свой декодер ELD —
//! это месяцы работы. Подключается фичей `ffmpeg`.

use ffmpeg_next as ff;

use crate::Error;

pub struct Eld {
    dec: ff::decoder::Audio,
    frame: ff::frame::Audio,
}

/// AudioSpecificConfig AAC-ELD: объектный тип 39, 44,1 кГц, стерео, кадр
/// 480 сэмплов — то, что шлёт iOS при зеркалировании (как у UxPlay).
pub const ASC_44100_STEREO_480: [u8; 4] = [0xF8, 0xE8, 0x50, 0x00];

impl Eld {
    pub fn new(config: &[u8]) -> Result<Self, Error> {
        let init = |e: ff::Error| Error::Init(format!("libavcodec: {e}"));
        crate::ff::init()?;
        let codec = ff::decoder::find(ff::codec::Id::AAC)
            .ok_or_else(|| Error::Init("в libavcodec нет декодера AAC".into()))?;
        let mut ctx = ff::codec::Context::new_with_codec(codec);
        crate::ff::set_extradata(&mut ctx, config)?;
        let dec = ctx
            .decoder()
            .open_as(codec)
            .map_err(init)?
            .audio()
            .map_err(init)?;
        Ok(Self {
            dec,
            frame: ff::frame::Audio::empty(),
        })
    }

    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<i16>) -> Result<usize, Error> {
        let err = |e: ff::Error| Error::Decode(e.to_string());
        self.dec
            .send_packet(&ff::Packet::copy(packet))
            .map_err(err)?;
        let mut frames = 0;
        while self.dec.receive_frame(&mut self.frame).is_ok() {
            if !matches!(self.frame.format(), ff::format::Sample::F32(_)) {
                return Err(Error::Decode(format!(
                    "неожиданный формат сэмплов {:?}",
                    self.frame.format()
                )));
            }
            let n = self.frame.samples();
            let channels = self.frame.channels().max(1) as usize;
            let planar = self.frame.is_planar();
            let left = self.frame.plane::<f32>(0);
            let right = if planar && channels > 1 {
                self.frame.plane::<f32>(1)
            } else {
                left
            };
            let to_i16 = |s: f32| (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            for i in 0..n {
                let (l, r) = if planar {
                    (left[i], right[i])
                } else {
                    (
                        left[i * channels],
                        left[i * channels + (channels > 1) as usize],
                    )
                };
                out.extend_from_slice(&[to_i16(l), to_i16(r)]);
            }
            frames += n;
        }
        Ok(frames)
    }

    pub fn reset(&mut self) {
        self.dec.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_eld() {
        let mut d = Eld::new(&ASC_44100_STEREO_480).unwrap();
        let mut out = Vec::new();
        // Мусор — ошибка, а не паника.
        let _ = d.decode(&[0x55; 64], &mut out);
    }

    /// Тот же путь через libavcodec, но на AAC-LC: ELD-кодера в обычном
    /// FFmpeg нет, а преобразование кадров общее.
    #[test]
    fn decodes_lc_through_libavcodec() {
        let mut data: &[u8] = include_bytes!("../tests/data/sine440.adts");
        let mut d = Eld::new(&[0x12, 0x10]).unwrap();
        let mut pcm = Vec::new();
        while data.len() > 7 && data[0] == 0xFF {
            let len = ((data[3] as usize & 3) << 11)
                | ((data[4] as usize) << 3)
                | (data[5] as usize >> 5);
            let header = if data[1] & 1 == 0 { 9 } else { 7 };
            d.decode(&data[header..len], &mut pcm).unwrap();
            data = &data[len..];
        }
        assert!(pcm.len() > 44_100 / 2, "кадров: {}", pcm.len() / 2);
        let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 2000, "тихо: {peak}");
    }
}
