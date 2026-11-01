//! Experimental AirPlay 2 realtime (type 96) PCM sender.
//!
//! This library owns protocol transport, not discovery, capture, decoding, local
//! output, processes or files. One [`MediaClock`] stamps packets for every receiver
//! and optional local renderer. Each [`Session`] is an independent transient-paired
//! receiver connection; the caller owns retries and group membership.
//!
//! This is a port of an experimental sender, not a claim of production-ready TX.
//! See the crate README and PROVENANCE for supported formats and limitations.

mod packet;
mod session;
mod transport;

pub use packet::{
    AudioPacket, FRAME_BYTES, FRAMES_PER_PACKET, MediaClock, PACKET_BYTES, SAMPLE_RATE,
};
pub use session::{Identity, Session, SessionConfig, StreamStats, Volume};
