//! Companion Link: пульт и клавиатура к Apple TV.
//!
//! Слои:
//! - [`opack`] — формат сериализации;
//! - [`frame`] — кадры и шифрование канала;
//! - [`auth`] — pair-verify поверх сохранённых HAP-ключей;
//! - [`commands`] — команды `_hidC`, `_mcc`, `_sessionStart`, события;
//! - [`link`] — сборка/разбор без привязки к транспорту;
//! - [`session`] — сессия поверх TCP: pair-verify, корреляция, таймауты,
//!   обрыв и очистка состояния.
//!
//! Проверено только на референсных wire-фактах и автономных тестах:
//! обмен с настоящим Apple TV не выполнялся, TVRCSessionStart/SystemStatus
//! требуют аппаратной проверки.

pub mod auth;
pub mod commands;
pub mod frame;
pub mod link;
pub mod opack;
pub mod session;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("OPACK: {0}")]
    Opack(&'static str),
    #[error("OPACK: неизвестный тег {0:#04x}")]
    Tag(u8),
    #[error("кадр: {0}")]
    Frame(&'static str),
    #[error("в кадре нет данных сопряжения `_pd`")]
    NoPairingData,
    #[error(transparent)]
    Crypto(#[from] crypto::Error),
    #[error(transparent)]
    Pair(#[from] pair::Error),
    #[error("сессия Companion: {0}")]
    Session(&'static str),
    #[error("таймаут ожидания ответа Companion")]
    Timeout,
    #[error("соединение Companion закрыто")]
    Closed,
}
