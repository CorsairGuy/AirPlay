//! Дробление и неполная запись buffered-аудио (C70).
//!
//! Синтетический `AsyncWrite`: пишет не больше N байт за вызов, один раз
//! отдаёт `Pending`, может оборваться на заданном байте. Проверяется, что
//! ключевой поток не повторяет nonce и не продолжается после обрыва, а новая
//! сессия с новым ключом работает как обычно.

use std::pin::Pin;
use std::task::{Context, Poll};

use media::tx::buffered::{BufferedSender, StreamSetup};
use rtp::AudioCipher;
use tokio::io::AsyncWrite;

/// Пишет кусками, один раз возвращает `Pending`, затем может оборваться.
struct Chunked {
    wire: Vec<u8>,
    limit: usize,
    pending_once: bool,
    fail_after: Option<usize>,
    failed: bool,
}

impl Chunked {
    fn new(limit: usize) -> Self {
        Self {
            wire: Vec::new(),
            limit,
            pending_once: false,
            fail_after: None,
            failed: false,
        }
    }

    fn failing(limit: usize, after: usize) -> Self {
        Self {
            fail_after: Some(after),
            ..Self::new(limit)
        }
    }

    fn decrypt(&self, shk: &[u8; 32]) -> Vec<(u16, Vec<u8>)> {
        let cipher = AudioCipher::new(shk);
        let mut buf = self.wire.clone();
        let mut seen = Vec::new();
        while let Some(mut packet) = rtp::split_buffered(&mut buf) {
            let (header, payload) = cipher.open(&mut packet).expect("пакет расшифровывается");
            seen.push((header.seq, payload.to_vec()));
        }
        seen
    }

    fn complete_packets(&self) -> usize {
        let mut buf = self.wire.clone();
        let mut count = 0;
        while rtp::split_buffered(&mut buf).is_some() {
            count += 1;
        }
        count
    }
}

impl AsyncWrite for Chunked {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.failed {
            return Poll::Ready(Err(std::io::Error::other("соединение оборвано")));
        }
        if !self.pending_once {
            self.pending_once = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if let Some(limit) = self.fail_after {
            if self.wire.len() >= limit {
                self.failed = true;
                return Poll::Ready(Err(std::io::Error::other("соединение оборвано")));
            }
        }
        let allowed = self.limit.min(buf.len());
        if let Some(limit) = self.fail_after {
            let room = limit.saturating_sub(self.wire.len()).min(allowed);
            if room == 0 {
                self.failed = true;
                return Poll::Ready(Err(std::io::Error::other("соединение оборвано")));
            }
            self.wire.extend_from_slice(&buf[..room]);
            return Poll::Ready(Ok(room));
        }
        self.wire.extend_from_slice(&buf[..allowed]);
        Poll::Ready(Ok(allowed))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn shk() -> [u8; 32] {
    [9; 32]
}

fn sender(sink: Chunked) -> BufferedSender<Chunked> {
    BufferedSender::new(sink, &shk(), 0x1600_0000, 20, 1_000)
}

#[tokio::test]
async fn byte_sized_writes_still_deliver_complete_decryptable_frames() {
    let mut sender = sender(Chunked::new(3));
    for payload in [&b"first"[..], &b"second"[..], &b"third"[..]] {
        sender.send_frame(payload).await.unwrap();
    }
    assert_eq!(sender.next_sequence(), 23);
    assert_eq!(sender.next_timestamp(), 1_000 + 3 * 1024);

    let sink = sender.into_inner();
    assert_eq!(
        sink.decrypt(&shk()),
        vec![
            (20, b"first".to_vec()),
            (21, b"second".to_vec()),
            (22, b"third".to_vec())
        ],
        "дробление не портит кадры и порядок"
    );
}

#[tokio::test]
async fn an_interrupted_write_ends_the_keyed_stream() {
    let mut sender = sender(Chunked::failing(5, 50));
    sender.send_frame(b"ok").await.unwrap();
    let error = sender.send_frame(b"broken-payload").await.unwrap_err();
    assert!(error.to_string().contains("buffered"), "{error}");

    // Тот же поток больше не используется: nonce не повторяется.
    let repeated = sender.send_frame(b"again").await.unwrap_err();
    assert!(
        repeated.to_string().contains("unusable"),
        "поток должен быть закрыт, а не продолжен: {repeated}"
    );
    assert_eq!(
        sender.next_sequence(),
        21,
        "счётчик не сдвинулся после обрыва"
    );

    let sink = sender.into_inner();
    assert_eq!(
        sink.complete_packets(),
        1,
        "в сокет ушёл ровно один целый пакет"
    );
    assert_eq!(sink.decrypt(&shk()), vec![(20, b"ok".to_vec())]);
}

#[tokio::test]
async fn a_new_session_after_a_failure_starts_clean() {
    let mut first = sender(Chunked::failing(4, 10));
    let _ = first.send_frame(b"doomed").await;
    assert!(first.into_inner().complete_packets() <= 1);

    // A replacement session must have fresh key material: starting nonce zero
    // with the old key would reuse an AEAD nonce even if the first write failed.
    let fresh_key = [10; 32];
    let mut second = BufferedSender::new(Chunked::new(7), &fresh_key, 0x1600_0000, 20, 1_000);
    second.send_frames(&[b"one", b"two"]).await.unwrap();
    let sink = second.into_inner();
    assert_eq!(
        sink.decrypt(&fresh_key),
        vec![(20, b"one".to_vec()), (21, b"two".to_vec())],
        "новая сессия использует свои seq/nonce, а не остатки прежней"
    );
}

#[tokio::test]
async fn an_oversized_frame_is_refused_before_any_io() {
    let mut sender = sender(Chunked::new(1024));
    let huge = vec![0u8; u16::MAX as usize];
    let error = sender.send_frame(&huge).await.unwrap_err();
    assert!(error.to_string().contains("65533") || error.to_string().contains("больше"));

    // Поток остаётся рабочим: IO не начиналось.
    sender.send_frame(b"after").await.unwrap();
    assert_eq!(sender.next_sequence(), 21);
    assert_eq!(sender.into_inner().complete_packets(), 1);
}

#[tokio::test]
async fn an_empty_frame_does_not_touch_the_wire() {
    let mut sender = sender(Chunked::new(64));
    assert!(sender.send_frame(b"").await.is_err());
    let sink = sender.into_inner();
    assert!(sink.wire.is_empty());
    assert_eq!(sink.complete_packets(), 0);
}

#[tokio::test]
async fn stream_setup_roundtrips_through_plist() {
    let setup = StreamSetup::aac_44100(7101, shk(), 11);
    let dict = setup.plist();
    assert_eq!(
        dict.get("type").and_then(plist::Value::as_unsigned_integer),
        Some(103)
    );
    assert_eq!(
        dict.get("spf").and_then(plist::Value::as_unsigned_integer),
        Some(1024)
    );
}
