//! Group selection and coordination using the existing runtime.
pub use ::remote::output_control;
pub mod output_router;
/// Очередь вывода и подготовка сессий живут в сервисе `sessions`.
pub use ::sessions::output_worker;
pub mod ptp_group;
pub mod relay_selection;

/// Shared group, output and volume state.
pub use ::coordinator as model;
