//! Видео зеркалирования: декодер на libavcodec ([`DecodingSink`], фича
//! `ffmpeg`) и выдача кадров по часам отправителя ([`Pacer`]).
//!
//! Показ зависит от железа (KMS на Станции, D3D11 на Windows) и живёт в
//! программе, которая встраивает приёмник: она реализует [`FrameSink`] и
//! отдаёт приёмнику `mirror::VideoSinkFactory` с [`DecodingSink`] внутри.

mod pacer;

pub use pacer::Pacer;

#[cfg(feature = "ffmpeg")]
mod decode;

#[cfg(feature = "ffmpeg")]
pub use codec::video::{Picture, PixelFormat, VideoOptions};
#[cfg(feature = "ffmpeg")]
pub use decode::{DecodingSink, Discard, FrameSink, Options};
