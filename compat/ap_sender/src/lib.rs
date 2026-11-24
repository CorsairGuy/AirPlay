//! Compatibility re-export; new integrations should depend on `media_tx` and `coordinator`.

pub use media_tx::*;
pub use coordinator::{group_volume, outputs};
pub mod group {
    pub use media_tx::group::*;
    pub use coordinator::group::{Attempt, Coordinator, Membership, Observation};
}
