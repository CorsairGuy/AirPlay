//! Compatibility re-export; new integrations should depend on `media::tx` and `coordinator`.

pub use ::media::tx::*;
pub use coordinator::{group_volume, outputs};
pub mod group {
    pub use ::media::tx::group::*;
    pub use coordinator::group::{Attempt, Coordinator, Membership, Observation};
}
