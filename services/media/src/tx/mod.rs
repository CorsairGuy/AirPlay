//! Experimental AirPlay 2 realtime (type 96) PCM sender.
//!
//! This library owns protocol transport and a local-renderer contract, not discovery,
//! capture, decoding, device output, processes or files. One [`MediaClock`] stamps packets for every receiver
//! and optional local renderer. Each [`Session`] is an independent authenticated
//! receiver connection (transient or saved pairing); the caller owns retries,
//! credential persistence and group membership.
//!
//! This is a port of an experimental sender, not a claim of production-ready TX.
//! See the crate README and PROVENANCE for supported formats and limitations.

pub mod airplay1;
pub mod continuous;
pub mod continuous_source;
pub mod group;
mod local;
mod local_pipe;
pub mod mirror;
mod packet;
pub mod pool;
mod session;
pub mod source_process;
pub mod source_runtime;
pub mod timing;
mod transport;

pub use local::{LocalSink, run_local};
pub use local_pipe::LocalPcmPipe;
pub use mirror::{
    AudioFormat, MirrorAuth, MirrorConfig, MirrorSession, VIDEO_HEADER_LEN, VideoCodec,
};
pub use packet::{
    AudioPacket, FRAME_BYTES, FRAMES_PER_PACKET, MediaClock, PACKET_BYTES, SAMPLE_RATE,
};
pub use session::{
    Artwork, Identity, MAX_ARTWORK_BYTES, MediaProgress, MetadataHandle, PairingCredentials,
    PinPairing, PtpPeerControl, Session, SessionConfig, StreamStats, TrackMetadata, Volume,
};
