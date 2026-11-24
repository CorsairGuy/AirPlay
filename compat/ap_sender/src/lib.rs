//! Compatibility re-export; new integrations should depend on `media::tx` and `coordinator`.

pub use ::media::tx::*;
pub use coordinator::group_volume;
pub mod outputs {
    pub use ::coordinator::outputs::{ReceiverEndpoint, SelectionPlan};
    pub use ::discovery::catalog::{OutputCatalog, OutputRoute};
}
pub mod group {
    pub use ::media::tx::group::*;
    pub use coordinator::group::{Attempt, Coordinator, Membership, Observation};
}
