//! Приёмник AirPlay как библиотека: то же, что `airplay serve`, для
//! программ под конкретное железо. Они собирают [`serve::Options`],
//! при необходимости подставляют свой вывод звука
//! (`options.audio.sink = Some(audio::SinkFactory::new(...))`) и показ видео
//! (`options.mirror = Some(mirror::VideoSinkFactory::new(...))`, обычно с
//! `video::DecodingSink` и своим `video::FrameSink`), затем вызывают
//! [`serve::run`].

pub mod card;
pub mod discovery_catalog;
pub mod discovery_feed;
pub mod events;
pub mod local_process;
pub mod media;
#[cfg(test)]
mod mrp_path_tests;
pub mod output_control;
pub mod output_worker;
pub mod relay_selection;
pub mod remote;
pub mod route_pairing;
pub mod send;
pub mod send_control;
#[cfg(unix)]
pub mod send_ipc;
pub mod sender_store;
pub mod serve;
pub mod session;
pub mod session_preparer;
pub mod state;
#[macro_use]
pub mod mrp_service;
/// Compatibility import for older adapters; new code uses mrp_service.
pub use mrp_service as station_mrp;
/// Compatibility macro for older adapters.
#[macro_export]
macro_rules! station_elog {($($arg:tt)*)=>{{$crate::mrp_log!($($arg)*);}}}

pub use audio;
pub use audio_stream;
pub use device;
pub use mirror;
pub use video;
