//! Persistent identities and pairing records.
#[cfg(target_os = "linux")]
pub mod daemon_identity;
pub mod route_pairing;
pub mod sender_store;
pub mod state;
