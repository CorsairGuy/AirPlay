//! Приёмник AirPlay как библиотека: то же, что `airplay`, для
//! программ под конкретное железо. Они собирают [`sessions::serve::Options`],
//! при необходимости подставляют свой вывод звука
//! (`options.audio.sink = Some(audio::SinkFactory::new(...))`) и показ видео
//! (`options.mirror = Some(mirror::VideoSinkFactory::new(...))`, обычно с
//! `video::DecodingSink` и своим `video::FrameSink`), затем вызывают
//! [`sessions::serve::run`].

// CLI-specific frontends; embedders use the owning service crates directly.
pub mod media;
#[cfg(test)]
mod mrp_path_tests;
#[cfg(unix)]
pub mod send_ipc;
