//! Приёмник AirPlay как библиотека: то же, что `airplay serve`, для
//! программ под конкретное железо. Они собирают [`serve::Options`],
//! при необходимости подставляют свой вывод звука
//! (`options.audio.sink = Some(audio::SinkFactory::new(...))`) и показ видео
//! (`options.mirror = Some(mirror::VideoSinkFactory::new(...))`, обычно с
//! `video::DecodingSink` и своим `video::FrameSink`), затем вызывают
//! [`serve::run`].

pub mod card;
pub mod events;
pub mod local_process;
pub mod media;
#[cfg(test)]
mod mrp_path_tests;
pub mod remote;
pub mod route_pairing;
pub mod send;
pub mod send_control;
#[cfg(unix)]
pub mod send_ipc;
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
