//! Совместимость: разбор RTSP-сессии переехал в `sessions::session`.
//!
//! Адаптеры продолжают видеть `airplay::sessions::session::*`; реализация
//! живёт в сервисе `sessions`.
pub use ::sessions::session::*;
