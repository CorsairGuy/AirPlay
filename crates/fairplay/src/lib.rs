//! FairPlay SAP (fp-setup) приёмника: ответы на M1 и M2 и расшифровка 72-байтного
//! ключа (`ekey`) из SETUP.
//!
//! Поддерживается только версия 3 (`request[4] == 0x03`). Версия 2 (SAPv2.5,
//! `request[4] == 0x02`, macOS Music) отклоняется отдельной ошибкой: для неё
//! нужны производные whitebox-константы, которых нет в публичных wire-фактах.
//! Что известно и чего не хватает — `docs/FAIRPLAY-V2.md`.
//!
//! Код перенесён из [shairplay-rust](https://github.com/metaneutrons/shairplay-rust)
//! (LGPL-3.0, Copyright metaneutrons), который в свою очередь переводит на Rust
//! playfair из shairplay/UxPlay. Тестовые векторы сверены с исходным C-кодом.

mod garble;
mod sap;
mod tables;

pub use sap::FairPlay;

/// Ошибка разбора сообщений fp-setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("неподдерживаемая версия FairPlay")]
    Version,
    /// FairPlay v2 (SAPv2.5): `request[4] == 0x02`. Отличается от неизвестной
    /// версии и от v3 с mode 2 (`request[14] == 0x02`). Не реализовано, потому
    /// что криптопримитив v2 известен только как сторонние производные
    /// константы; см. `docs/FAIRPLAY-V2.md`.
    #[error("FairPlay v2 (SAPv2.5) не реализован; поддерживается только версия 3")]
    UnsupportedV2,
    #[error("неизвестный режим FairPlay")]
    Mode,
    #[error("fp-setup не завершён")]
    NotReady,
    #[error("неверная длина сообщения fp-setup: {0}")]
    Length(usize),
}

impl FairPlay {
    /// Отвечает на тело запроса `POST /fp-setup` любой из двух стадий.
    pub fn handle(&mut self, body: &[u8]) -> Result<Vec<u8>, Error> {
        if let Ok(m1) = <&[u8; 16]>::try_from(body) {
            Ok(self.setup(m1)?.to_vec())
        } else if let Ok(m2) = <&[u8; 164]>::try_from(body) {
            Ok(self.handshake(m2)?.to_vec())
        } else {
            Err(Error::Length(body.len()))
        }
    }
}
