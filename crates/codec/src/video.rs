//! Видео H.264/H.265 через libavcodec: программный декодер или аппаратный
//! по имени (`h264_v4l2m2m` на Raspberry Pi, `h264_cuvid` и т. п.).
//!
//! На вход — кадр целиком в Annex B (стартовые коды), наборы параметров —
//! тоже в Annex B: так одинаково работают и программные, и v4l2m2m
//! декодеры. На выходе — [`Picture`] без копирования: буферы кадра
//! libavcodec считает ссылками.

use ffmpeg_next as ff;

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
}

#[derive(Debug, Clone, Default)]
pub struct VideoOptions {
    /// Декодер libavcodec по имени, например `h264_v4l2m2m`; `None` —
    /// встроенный программный.
    pub decoder: Option<String>,
    /// Не держать кадры в очереди потоков декодера: на кадр меньше
    /// задержка на каждый поток, но и к/с на слабом процессоре меньше.
    pub low_delay: bool,
}

pub struct VideoDecoder {
    dec: ff::decoder::Video,
    frame: ff::frame::Video,
    name: String,
}

impl VideoDecoder {
    pub fn new(
        codec: VideoCodec,
        parameter_sets: &[u8],
        opts: &VideoOptions,
    ) -> Result<Self, Error> {
        let init = |e: ff::Error| Error::Init(format!("libavcodec: {e}"));
        crate::ff::init()?;
        let found = match &opts.decoder {
            Some(name) => ff::decoder::find_by_name(name),
            None => ff::decoder::find(match codec {
                VideoCodec::H264 => ff::codec::Id::H264,
                VideoCodec::H265 => ff::codec::Id::HEVC,
            }),
        };
        let found = found.ok_or_else(|| {
            Error::Init(format!(
                "в libavcodec нет декодера {}",
                opts.decoder.as_deref().unwrap_or(match codec {
                    VideoCodec::H264 => "H.264",
                    VideoCodec::H265 => "H.265",
                })
            ))
        })?;
        let mut ctx = ff::codec::Context::new_with_codec(found);
        crate::ff::set_extradata(&mut ctx, parameter_sets)?;
        if opts.low_delay {
            ctx.set_flags(ff::codec::Flags::LOW_DELAY);
            ctx.set_threading(ff::threading::Config {
                kind: ff::threading::Type::Slice,
                ..Default::default()
            });
        } else {
            // Потоки по кадрам: на четырёх ядрах ARM это и даёт 60 к/с.
            ctx.set_threading(ff::threading::Config {
                kind: ff::threading::Type::Frame,
                ..Default::default()
            });
        }
        let dec = ctx
            .decoder()
            .open_as(found)
            .map_err(init)?
            .video()
            .map_err(init)?;
        Ok(Self {
            dec,
            frame: ff::frame::Video::empty(),
            name: found.name().to_owned(),
        })
    }

    /// Имя декодера libavcodec.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Кадр в Annex B. `pts` возвращается в [`Picture::pts`] как есть; кадры
    /// выходят в порядке показа, с задержкой декодера.
    pub fn decode(
        &mut self,
        pts: i64,
        data: &[u8],
        out: &mut impl FnMut(Picture),
    ) -> Result<(), Error> {
        let mut packet = ff::Packet::copy(data);
        packet.set_pts(Some(pts));
        let sent = self.dec.send_packet(&packet);
        // Очередь декодера полна: сначала забираем готовые кадры.
        self.drain(out);
        match sent {
            Err(ff::Error::Other { errno }) if errno == ff::util::error::EAGAIN => {
                self.dec.send_packet(&packet).map_err(decode_err)?;
                self.drain(out);
                Ok(())
            }
            other => other.map_err(decode_err),
        }
    }

    /// Конец потока: отдаёт кадры, что ещё в декодере.
    pub fn flush(&mut self, out: &mut impl FnMut(Picture)) {
        if self.dec.send_eof().is_ok() {
            self.drain(out);
        }
        self.dec.flush();
    }

    fn drain(&mut self, out: &mut impl FnMut(Picture)) {
        while self.dec.receive_frame(&mut self.frame).is_ok() {
            out(Picture(std::mem::replace(
                &mut self.frame,
                ff::frame::Video::empty(),
            )));
        }
    }
}

fn decode_err(e: ff::Error) -> Error {
    Error::Decode(e.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Три плоскости Y, U, V; цветность вдвое меньше по обеим осям.
    /// Сюда же — полный диапазон (`yuvj420p`).
    Yuv420p,
    /// Плоскость Y и плоскость UV через одну: так отдают аппаратные декодеры.
    Nv12,
    /// Что-то другое (10 бит, кадр в памяти GPU): имя формата libavutil.
    Other(&'static str),
}

/// Декодированный кадр.
pub struct Picture(ff::frame::Video);

impl Picture {
    pub fn width(&self) -> u32 {
        self.0.width()
    }

    pub fn height(&self) -> u32 {
        self.0.height()
    }

    /// То, что передали в [`VideoDecoder::decode`] с этим кадром.
    pub fn pts(&self) -> i64 {
        self.0.pts().unwrap_or(0)
    }

    pub fn format(&self) -> PixelFormat {
        use ff::format::Pixel;
        match self.0.format() {
            Pixel::YUV420P | Pixel::YUVJ420P => PixelFormat::Yuv420p,
            Pixel::NV12 => PixelFormat::Nv12,
            other => PixelFormat::Other(other.descriptor().map(|d| d.name()).unwrap_or("?")),
        }
    }

    pub fn planes(&self) -> usize {
        self.0.planes()
    }

    /// Байты плоскости `index`: строки по [`Picture::stride`] байт.
    pub fn plane(&self, index: usize) -> &[u8] {
        self.0.data(index)
    }

    pub fn stride(&self, index: usize) -> usize {
        self.0.stride(index)
    }

    /// Кадр libavutil как есть: для показа без копирования (DRM PRIME,
    /// текстуры GPU) в программе, которая сама связана с FFmpeg.
    pub fn as_av_frame(&self) -> *const ff::ffi::AVFrame {
        // SAFETY: только указатель; кадр живёт, пока жив `self`.
        unsafe { self.0.as_ptr() }
    }
}
