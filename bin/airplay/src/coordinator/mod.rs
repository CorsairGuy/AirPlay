//! Group selection and coordination using the existing runtime.
pub mod output_control;
pub mod output_router;
pub mod output_worker;
pub mod ptp_group;
pub mod relay_selection;

/// Shared group, output and volume state.
pub use ::coordinator as model;
