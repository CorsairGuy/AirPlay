//! MediaRemote protocol (MRP): карточка «Сейчас играет», пульт и громкость
//! на iPhone для приёмника AirPlay.
//!
//! MRP идёт внутри шифрованного канала данных потока 130 (его открывает
//! RTSP-сессия). Здесь — только протокол: [`frame`] — кадры канала,
//! [`wire`] — protobuf, [`schema`] — номера полей из описаний pyatv,
//! [`messages`] — сообщения приёмника. Обработку запросов и рассылку
//! изменений делает активный `mrp_service` приложения; старый пакет
//! `mediaremote` — альтернативная реализация, ожидающая консолидации в control.

pub mod authorization;
pub mod frame;
pub mod messages;
pub mod output_context;
pub mod schema;
pub mod wire;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Длина кадра вне допустимого: дальше поток не разобрать.
    #[error("кадр MRP: неверная длина")]
    FrameSize,
    #[error("кадр MRP: {0}")]
    Frame(&'static str),
    #[error("protobuf: {0}")]
    Protobuf(&'static str),
    #[error("схема MRP: {0}")]
    Schema(String),
}
