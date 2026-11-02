//! Experimental AirPlay 2 realtime (type 96) PCM sender.
//!
//! This library owns protocol transport and a local-renderer contract, not discovery,
//! capture, decoding, device output, processes or files. One [`MediaClock`] stamps packets for every receiver
//! and optional local renderer. Each [`Session`] is an independent transient-paired
//! receiver connection; the caller owns retries and group membership.
//!
//! This is a port of an experimental sender, not a claim of production-ready TX.
//! See the crate README and PROVENANCE for supported formats and limitations.

mod local;
pub mod group;
mod local_pipe;
mod packet;
mod session;
mod transport;

pub use local::{LocalSink, run_local};
pub use local_pipe::LocalPcmPipe;
pub use packet::{
    AudioPacket, FRAME_BYTES, FRAMES_PER_PACKET, MediaClock, PACKET_BYTES, SAMPLE_RATE,
};
pub use session::{Identity, MediaProgress, Session, SessionConfig, StreamStats, Volume};
