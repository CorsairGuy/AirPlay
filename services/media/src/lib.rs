//! Media receiving, transmission and local output components.
pub mod events;
pub mod tx;
pub mod rx {
    pub use audio_stream;
    #[cfg(feature = "mirroring")]
    pub use mirror;
}
pub mod output;
pub use codec;
