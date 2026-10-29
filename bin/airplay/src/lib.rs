//! Приёмник AirPlay как библиотека: то же, что `airplay serve`, для
//! программ под конкретное железо. Они собирают [`serve::Options`],
//! при необходимости подставляют свой вывод звука
//! (`options.audio.sink = Some(audio::SinkFactory::new(...))`) и вызывают
//! [`serve::run`].

pub mod media;
pub mod serve;
pub mod session;
pub mod state;

pub use audio;
pub use audio_stream;
pub use device;
pub use mirror;
