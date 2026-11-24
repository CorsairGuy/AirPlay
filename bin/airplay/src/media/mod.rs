//! Media receive, send, and local output boundaries.
pub mod output;
pub mod rx;
pub mod tx;
pub use codec;
// Preserve the original receiver imports while callers migrate to media::rx.
pub use rx::*;
