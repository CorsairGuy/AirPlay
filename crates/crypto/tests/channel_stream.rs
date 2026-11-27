//! Поведение шифрованного канала после ошибки в потоке (C59).
//!
//! Проверяется контракт вызывающего транспорта: корректные кадры до ошибки
//! выдаются только в том вызове, где они целиком проверены; испорченный кадр
//! не выдаёт ни себя, ни последующие; после ошибки канал закрыт и требует
//! нового соединения. Настоящие ключи не используются.

use crypto::channel::{Channel, MAX_FRAME};
use crypto::kdf::ControlKeys;

fn pair(seed: u8) -> (Channel, Channel) {
    let shared = [seed; 32];
    (
        Channel::new(ControlKeys::accessory(&shared)),
        Channel::new(ControlKeys::controller(&shared)),
    )
}

/// Транспорт с явным состоянием «соединение закрыто».
struct Transport {
    channel: Channel,
    buf: Vec<u8>,
    closed: bool,
}

impl Transport {
    fn new(channel: Channel) -> Self {
        Self {
            channel,
            buf: Vec::new(),
            closed: false,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, ()> {
        if self.closed {
            return Err(());
        }
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        match self.channel.open(&mut self.buf, &mut out) {
            Ok(()) => Ok(out),
            Err(_) => {
                // Ошибка AEAD необратима: соединение закрывается, буфер
                // больше не обрабатывается.
                self.closed = true;
                self.buf.clear();
                Err(())
            }
        }
    }
}

#[test]
fn a_valid_frame_before_a_corrupt_one_is_not_released_in_the_same_call() {
    let (acc, mut ctl) = pair(1);
    let mut wire = Vec::new();
    ctl.seal(b"first", &mut wire).unwrap();
    let first_len = wire.len();
    ctl.seal(b"second", &mut wire).unwrap();
    // Портим второй кадр: его тег.
    let last = wire.len() - 1;
    wire[last] ^= 0x10;

    let mut transport = Transport::new(acc);
    // Первый кадр приходит целиком — он выдаётся.
    let out = transport.feed(&wire[..first_len]).unwrap();
    assert_eq!(out, b"first");
    // Второй (испорченный) кадр: ни открытого текста, ни частичной выдачи.
    assert!(transport.feed(&wire[first_len..]).is_err());
    assert!(transport.closed);
}

#[test]
fn a_partial_next_frame_is_not_delivered_after_an_error() {
    let (acc, mut ctl) = pair(2);
    let mut wire = Vec::new();
    ctl.seal(b"ok", &mut wire).unwrap();
    ctl.seal(&vec![0x5a; MAX_FRAME + 10], &mut wire).unwrap();
    let last = wire.len() - 1;
    wire[last] ^= 1;

    let mut transport = Transport::new(acc);
    // Режем испорченный кадр так, чтобы он был неполным в первом чтении.
    let split = wire.len() - 3;
    assert!(transport.feed(&wire[..split]).is_ok());
    assert!(transport.feed(&wire[split..]).is_err());
    // Даже продолжение потока больше не принимается.
    assert!(transport.feed(&[]).is_err());
}

#[test]
fn after_an_error_only_a_new_channel_accepts_frames() {
    let (acc, mut ctl) = pair(3);
    let mut bad = Vec::new();
    ctl.seal(b"x", &mut bad).unwrap();
    bad[3] ^= 0xFF;

    let mut transport = Transport::new(acc);
    assert!(transport.feed(&bad).is_err());

    // Тот же экземпляр канала не восстанавливается.
    let mut again = Vec::new();
    ctl.seal(b"y", &mut again).unwrap();
    assert!(transport.feed(&again).is_err());

    // Новое соединение начинается с новых каналов и счётчиков.
    let (acc, mut ctl) = pair(3);
    let mut fresh = Vec::new();
    ctl.seal(b"y", &mut fresh).unwrap();
    let mut transport = Transport::new(acc);
    assert_eq!(transport.feed(&fresh).unwrap(), b"y");
    assert!(!transport.closed);
}

#[test]
fn a_tampered_length_is_rejected_without_consuming_the_stream() {
    let (acc, mut ctl) = pair(4);
    let mut wire = Vec::new();
    ctl.seal(b"payload", &mut wire).unwrap();
    let original = wire.clone();
    wire[0] ^= 0x01;
    let mut buf = wire.clone();
    let mut out = Vec::new();
    let mut channel = acc;
    assert!(channel.open(&mut buf, &mut out).is_err());
    assert_eq!(buf, wire);
    assert!(out.is_empty());
    // Исходный (неизменённый) поток после ошибки тоже не принимается.
    let mut good = original;
    assert!(channel.open(&mut good, &mut out).is_err());
    assert!(out.is_empty());
}

#[test]
fn several_complete_frames_in_one_call_are_all_or_nothing() {
    let (acc, mut ctl) = pair(5);
    let mut wire = Vec::new();
    for msg in [&b"a"[..], b"bb", b"ccc"] {
        ctl.seal(msg, &mut wire).unwrap();
    }
    // Портим только последний тег.
    let last = wire.len() - 1;
    wire[last] ^= 1;
    let mut channel = acc;
    let mut out = Vec::new();
    assert!(channel.open(&mut wire, &mut out).is_err());
    assert!(
        out.is_empty(),
        "ни один кадр не выдаётся при ошибке в пачке"
    );
}
