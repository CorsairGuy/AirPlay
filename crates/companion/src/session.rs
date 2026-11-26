//! Жизненный цикл сессии Companion Link поверх TCP.
//!
//! Транспорт остаётся у вызывающего, но последовательность обмена и её отказы
//! живут здесь: pair-verify до первого зашифрованного кадра, открытие и закрытие
//! сессии `_sessionStart`/`_sessionStop`, корреляция ответа по `_x`, таймаут,
//! обнаружение обрыва и очистка состояния. Ограничения размера кадра и
//! nonce-счётчики остаются в [`crate::frame`]: после переподключения создаётся
//! новый [`Link`], поэтому счётчики начинаются с нуля, а не продолжаются.

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::{Instant, timeout};

use crate::Error;
use crate::auth::{Credentials, PairVerify};
use crate::commands;
use crate::frame::{Frame, FrameType};
use crate::link::Link;
use crate::opack::Value;

/// Запас чтения на один вызов; кадр может прийти несколькими кусками.
const READ_CHUNK: usize = 8192;

/// Идентификаторы сессии пульта.
///
/// Приёмник возвращает свой `_sid` на `_sessionStart`; в `_sessionStop`
/// уходит склейка `(remote << 32) | local`, как это делает референсный
/// клиент pyatv 0.18.0 (`b277a4c8`, `protocols/companion/api.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionId {
    /// `_sid`, который мы объявили в `_sessionStart` (нижние 32 бита).
    pub local: u32,
    /// `_sid` из ответа приёмника.
    pub remote: u64,
}

impl SessionId {
    /// Значение `_sid` для `_sessionStop`.
    pub fn wire(self) -> u64 {
        (self.remote << 32) | u64::from(self.local)
    }
}

/// Открытая сессия Companion Link. После обрыва дальнейшие запросы сразу
/// возвращают [`Error::Closed`], не трогая мёртвый поток.
pub struct Session<S> {
    stream: S,
    link: Link,
    closed: bool,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Session<S> {
    /// Сессия без шифрования: только для тестов и диагностики в локальной сети.
    pub fn plain(stream: S) -> Self {
        Self {
            stream,
            link: Link::plain(),
            closed: false,
        }
    }

    /// Сессия с готовыми ключами канала: `(запись, чтение)`.
    pub fn encrypted(stream: S, write: [u8; 32], read: [u8; 32]) -> Self {
        Self {
            stream,
            link: Link::encrypted(write, read),
            closed: false,
        }
    }

    /// Выполняет pair-verify и возвращает сессию на зашифрованном канале.
    ///
    /// Первые два обмена идут открытыми кадрами pair-verify с `_pd`, затем
    /// канал шифруется ключами из [`PairVerify::keys`]. Ничего не пишется в
    /// поток до успешного `confirm` приёмника.
    pub async fn connect(
        stream: S,
        credentials: &Credentials,
        deadline: Duration,
    ) -> Result<Self, Error> {
        let mut session = Self::plain(stream);
        let mut verify = PairVerify::new(credentials.clone());
        let start = session
            .auth_exchange(FrameType::PairVerifyStart, &verify.start(), deadline)
            .await?;
        let proof = verify.finish(&start)?;
        let confirmed = session
            .auth_exchange(FrameType::PairVerifyNext, &proof, deadline)
            .await?;
        verify.confirm(&confirmed)?;
        let (write, read) = verify.keys()?;
        session.link = Link::encrypted(write, read);
        Ok(session)
    }

    /// `true`, пока соединение не оборвалось и не закрыто вызывающим.
    pub fn is_open(&self) -> bool {
        !self.closed
    }

    pub fn is_encrypted(&self) -> bool {
        self.link.is_encrypted()
    }

    pub fn link(&mut self) -> &mut Link {
        &mut self.link
    }

    async fn write_frame(&mut self, wire: &[u8], deadline: Duration) -> Result<(), Error> {
        // Cancellation during write_all may leave a partial frame on the wire.
        // Never reuse this session unless the complete write has finished.
        self.closed = true;
        match timeout(deadline, self.stream.write_all(wire)).await {
            Ok(Ok(())) => {
                self.closed = false;
                Ok(())
            }
            Ok(Err(_)) => Err(Error::Closed),
            Err(_) => Err(Error::Timeout),
        }
    }

    async fn auth_exchange(
        &mut self,
        kind: FrameType,
        value: &Value,
        deadline: Duration,
    ) -> Result<Value, Error> {
        let until = Instant::now() + deadline;
        let wire = self.link.send(kind, value)?;
        self.write_frame(&wire, deadline).await?;
        let frame = timeout(
            until.saturating_duration_since(Instant::now()),
            self.next_frame(),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        if frame.frame_type != FrameType::PairVerifyNext {
            return Err(Error::Session("неверный тип ответа pair-verify"));
        }
        crate::opack::unpack(&frame.payload)
    }

    /// Запрос с ответом: кадры с чужим `_x` (события, опоздавшие ответы)
    /// пропускаются, ответ на другой `_i` — ошибка, молчание дольше `deadline`
    /// — [`Error::Timeout`].
    pub async fn request(
        &mut self,
        identifier: &str,
        content: Value,
        deadline: Duration,
    ) -> Result<Value, Error> {
        if self.closed {
            return Err(Error::Closed);
        }
        let (xid, wire) = self.link.request(identifier, content)?;
        let until = Instant::now() + deadline;
        self.write_frame(&wire, deadline).await?;
        loop {
            let now = Instant::now();
            if now >= until {
                return Err(Error::Timeout);
            }
            let frame = match timeout(until - now, self.next_frame()).await {
                Err(_) => return Err(Error::Timeout),
                Ok(Ok(frame)) => frame,
                Ok(Err(error)) => return Err(error),
            };
            let value = Link::payload(&frame)?;
            if value.get("_x").and_then(Value::as_int) != Some(xid as i64) {
                // Чужой обмен или событие без `_x`: ответа на нас тут нет.
                continue;
            }
            if let Some(name) = value.get("_i").and_then(Value::as_str)
                && name != identifier
            {
                return Err(Error::Session("ответ на другой запрос"));
            }
            if value.get("_t").and_then(Value::as_int) != Some(commands::RESPONSE) {
                return Err(Error::Session("вместо ответа пришёл запрос"));
            }
            if value.get("_em").is_some() {
                return Err(Error::Session("приёмник отклонил команду"));
            }
            return Ok(value.get("_c").cloned().unwrap_or(Value::Null));
        }
    }

    /// Открывает сессию пульта: `{_srvT, _sid}` с локальным `_sid`.
    pub async fn open_remote(
        &mut self,
        local_sid: u32,
        deadline: Duration,
    ) -> Result<SessionId, Error> {
        let content = Value::object(vec![
            ("_srvT", Value::Str(commands::TV_REMOTE_SERVICES.into())),
            ("_sid", Value::Int(local_sid as i64)),
        ]);
        let reply = self.request("_sessionStart", content, deadline).await?;
        let remote = reply
            .get("_sid")
            .and_then(Value::as_int)
            .and_then(|sid| u32::try_from(sid).ok())
            .ok_or(Error::Session("в ответе _sessionStart нет _sid"))?;
        Ok(SessionId {
            local: local_sid,
            remote: u64::from(remote),
        })
    }

    /// Закрывает сессию и поток. Отказ приёмника не мешает освободить
    /// локальные ресурсы: ответ ждём не дольше `deadline`, ошибку возвращаем
    /// только если соединение уже мертво.
    pub async fn close_remote(&mut self, sid: SessionId, deadline: Duration) -> Result<(), Error> {
        let content = Value::object(vec![
            ("_srvT", Value::Str(commands::TV_REMOTE_SERVICES.into())),
            ("_sid", Value::Int(sid.wire() as i64)),
        ]);
        let result = match self.request("_sessionStop", content, deadline).await {
            Ok(_) => Ok(()),
            Err(Error::Closed) => Err(Error::Closed),
            Err(_) => Ok(()),
        };
        self.shutdown().await;
        result
    }

    /// Закрывает поток без сетевого обмена.
    pub async fn shutdown(&mut self) {
        self.closed = true;
        let _ = self.stream.shutdown().await;
    }

    async fn next_frame(&mut self) -> Result<Frame, Error> {
        loop {
            if let Some(frame) = self.link.poll()? {
                return Ok(frame);
            }
            let mut buffer = [0u8; READ_CHUNK];
            let read = match self.stream.read(&mut buffer).await {
                Ok(read) => read,
                Err(_) => {
                    self.closed = true;
                    return Err(Error::Closed);
                }
            };
            if read == 0 {
                self.closed = true;
                return Err(Error::Closed);
            }
            self.link.feed(&buffer[..read])?;
        }
    }
}
