//! Приёмник AirPlay как библиотека: то же, что `airplay`, для
//! программ под конкретное железо. Они собирают [`serve::Options`],
//! при необходимости подставляют свой вывод звука
//! (`options.audio.sink = Some(audio::SinkFactory::new(...))`) и показ видео
//! (`options.mirror = Some(mirror::VideoSinkFactory::new(...))`, обычно с
//! `video::DecodingSink` и своим `video::FrameSink`), затем вызывают
//! [`serve::run`].

pub mod card;
pub mod card_transport;
pub mod continuous_control;
pub mod identity;
#[cfg(target_os = "linux")]
pub use identity::daemon_identity;
pub use identity::{route_pairing, sender_store, state};
pub mod discovery;
pub use discovery::{catalog as discovery_catalog, feed as discovery_feed};
pub mod events;
pub mod local_process;
pub mod media;
#[cfg(test)]
mod mrp_path_tests;
pub use media::output::card as output_card;
pub mod coordinator;
pub use coordinator::{output_control, output_router, output_worker, ptp_group, relay_selection};
pub mod remote;
pub use media::tx as send;
pub mod send_control;
#[cfg(unix)]
pub mod send_ipc;
pub mod sessions;
pub use sessions::{preparer as session_preparer, serve, session};
pub mod source_process;
pub mod source_runtime;
/// Compatibility import for older adapters; new code uses mrp_service.
pub use mrp_service as station_mrp;
pub use remote::mrp_service;
/// Compatibility macro for older adapters.
#[macro_export]
macro_rules! station_elog {($($arg:tt)*)=>{{$crate::mrp_log!($($arg)*);}}}

pub use audio;
pub use audio_stream;
pub use device;
#[cfg(feature = "mirroring")]
pub use mirror;
#[cfg(feature = "mirroring")]
pub use video;

pub use timesync;
