//! Границы шифрованного control-канала HAP (задача C39).
//!
//! Проверяются только публичные контракты `Channel`: частичное чтение,
//! изменённый тег, повтор кадра, неверная длина и отсутствие открытых данных
//! после ошибки. Настоящие ключи не используются: общий секрет — константа.

use crypto::channel::{Channel, MAX_FRAME};
use crypto::kdf::ControlKeys;

fn channels(seed: u8) -> (Channel, Channel) {
    let shared = [seed; 32];
    (
        Channel::new(ControlKeys::accessory(&shared)),
        Channel::new(ControlKeys::controller(&shared)),
    )
}

fn sealed(seed: u8, msg: &[u8]) -> Vec<u8> {
    let (_, mut controller) = channels(seed);
    let mut wire = Vec::new();
    controller.seal(msg, &mut wire);
    wire
}

#[test]
fn frame_is_not_released_before_tag_arrives() {
    let (mut accessory, _) = channels(1);
    let wire = sealed(1, b"GET /info RTSP/1.0\r\n\r\n");
    // Без последнего байта тега кадр неполный: открытых данных быть не должно,
    // а буфер сохраняется целиком.
    let mut buf = wire[..wire.len() - 1].to_vec();
    let mut out = Vec::new();
    accessory.open(&mut buf, &mut out).unwrap();
    assert!(out.is_empty());
    assert_eq!(buf.len(), wire.len() - 1);

    buf.extend_from_slice(&wire[wire.len() - 1..]);
    accessory.open(&mut buf, &mut out).unwrap();
    assert_eq!(out, b"GET /info RTSP/1.0\r\n\r\n");
    assert!(buf.is_empty());
}

#[test]
fn a_single_byte_at_a_time_never_leaks_partial_plaintext() {
    let (mut accessory, _) = channels(2);
    let msg = vec![0x5a; MAX_FRAME * 2 + 7];
    let wire = sealed(2, &msg);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    for (i, byte) in wire.iter().enumerate() {
        buf.push(*byte);
        accessory.open(&mut buf, &mut out).unwrap();
        // Открытый текст приходит только целыми кадрами по MAX_FRAME.
        if i + 1 < wire.len() {
            assert_eq!(out.len() % MAX_FRAME, 0);
        }
    }
    assert_eq!(out, msg);
    assert!(buf.is_empty());
}

#[test]
fn tampered_tag_is_rejected_and_nothing_is_drained() {
    let (mut accessory, _) = channels(3);
    let mut wire = sealed(3, b"control frame");
    let last = wire.len() - 1;
    wire[last] ^= 0x80;
    let mut buf = wire.clone();
    let mut out = Vec::new();
    assert!(accessory.open(&mut buf, &mut out).is_err());
    assert!(out.is_empty(), "открытые данные после ошибки тега");
    assert_eq!(buf, wire, "буфер не должен сдвигаться при ошибке");
}

#[test]
fn valid_frame_before_corrupt_frame_is_not_partially_committed() {
    let (mut accessory, mut controller) = channels(12);
    let mut first = Vec::new();
    let mut second = Vec::new();
    controller.seal(b"first", &mut first);
    controller.seal(b"second", &mut second);
    *second.last_mut().unwrap() ^= 1;
    let mut wire = first;
    wire.extend_from_slice(&second);
    let original = wire.clone();
    let mut out = b"existing".to_vec();
    assert!(accessory.open(&mut wire, &mut out).is_err());
    assert_eq!(out, b"existing");
    assert_eq!(wire, original);
    // A poisoned receiver must not resume on an attacker-controlled retry.
    assert!(accessory.open(&mut wire, &mut out).is_err());
    assert_eq!(out, b"existing");
}

#[test]
fn tampered_length_prefix_is_rejected() {
    let (mut accessory, _) = channels(4);
    let mut wire = sealed(4, b"hello");
    wire[0] ^= 0x01; // длина в AAD не совпадает с фактической
    let mut buf = wire.clone();
    let mut out = Vec::new();
    assert!(accessory.open(&mut buf, &mut out).is_err());
    assert!(out.is_empty());
}

#[test]
fn replaying_the_same_wire_fails_closed_on_the_second_pass() {
    let (mut accessory, _) = channels(5);
    let wire = sealed(5, b"RECORD");
    let mut out = Vec::new();
    let mut buf = wire.clone();
    accessory.open(&mut buf, &mut out).unwrap();
    assert_eq!(out, b"RECORD");

    // Счётчик кадров уже сдвинут: тот же байтовый поток не должен открыться.
    let mut replay = wire;
    let mut again = Vec::new();
    assert!(accessory.open(&mut replay, &mut again).is_err());
    assert!(again.is_empty());
}

#[test]
fn super_short_and_empty_buffers_are_harmless() {
    let (mut accessory, _) = channels(6);
    let mut empty = Vec::new();
    let mut out = Vec::new();
    accessory.open(&mut empty, &mut out).unwrap();
    assert!(out.is_empty());

    // Один байт длины без остального кадра: ждём, не паникуем.
    let mut one = vec![0xff];
    accessory.open(&mut one, &mut out).unwrap();
    assert!(out.is_empty());
    assert_eq!(one, vec![0xff]);

    // Обещанная u16-длина больше MAX_FRAME допустима (iOS присылает длиннее),
    // но пока данных нет — открытого текста нет.
    let mut big = vec![0xff, 0xff, 1, 2, 3];
    accessory.open(&mut big, &mut out).unwrap();
    assert!(out.is_empty());
    assert_eq!(big.len(), 5);
}

#[test]
fn sealing_empty_input_produces_nothing_and_large_input_splits() {
    let (_, mut controller) = channels(7);
    let mut wire = Vec::new();
    controller.seal(&[], &mut wire);
    assert!(wire.is_empty());

    controller.seal(&vec![7u8; MAX_FRAME + 1], &mut wire);
    // Два кадра: MAX_FRAME и 1 байт, каждый с префиксом длины и тегом.
    let tag = 16;
    assert_eq!(wire.len(), MAX_FRAME + 1 + 2 * (2 + tag));
}
