//! Приёмник AirPlay как библиотека: то же, что `airplay serve`, для
//! программ под конкретное железо. Они собирают [`serve::Options`],
//! при необходимости подставляют свой вывод звука
//! (`options.audio.sink = Some(audio::SinkFactory::new(...))`) и показ видео
//! (`options.mirror = Some(mirror::VideoSinkFactory::new(...))`, обычно с
//! `video::DecodingSink` и своим `video::FrameSink`), затем вызывают
//! [`serve::run`].

pub mod card;
pub mod events;
pub mod media;
pub mod remote;
pub mod send;
pub mod serve;
pub mod session;
pub mod state;
#[macro_use]
pub mod station_mrp;

pub use audio;
pub use audio_stream;
pub use device;
pub use mirror;
pub use video;
