//! Независимые wire-векторы PTP для разбора в `clock::Ptp`.
//!
//! Байты собраны по IEEE 1588-2008: messageType в младшей тетраде байта 0,
//! versionPTP в младшей тетраде байта 1, correctionField — i64 big-endian в
//! масштабе 2^-16 нс (байты 8..16), sourcePortIdentity.clockIdentity — байты
//! 20..28, Timestamp Follow_Up — 6 байт секунд и 4 байта наносекунд с 34-го.
//! Это самостоятельная сборка, а не копия чужого кода.

use std::net::IpAddr;

use clock::Ptp;

const SYNC: u8 = 0x0;
const FOLLOW_UP: u8 = 0x8;
const DELAY_REQ: u8 = 0x1;
const DELAY_RESP: u8 = 0x9;
const ANNOUNCE: u8 = 0xB;
const SIGNALING: u8 = 0xC;

const GM: u64 = 0x1122_3344_5566_7788;

fn ip(last: u8) -> IpAddr {
    IpAddr::from([192, 168, 1, last])
}

fn header(kind: u8, len: usize, clock: u64, correction_raw: i64) -> Vec<u8> {
    let mut m = vec![0u8; len];
    m[0] = 0x10 | kind;
    m[1] = 0x02;
    m[2..4].copy_from_slice(&(len as u16).to_be_bytes());
    m[8..16].copy_from_slice(&correction_raw.to_be_bytes());
    m[20..28].copy_from_slice(&clock.to_be_bytes());
    m
}

fn announce(clock: u64) -> Vec<u8> {
    let mut m = header(ANNOUNCE, 64, clock, 0);
    m[47] = 248;
    m[52] = 248;
    m[53..61].copy_from_slice(&clock.to_be_bytes());
    m
}

fn follow_up_ns(clock: u64, timestamp_ns: u64, correction_ns: i64) -> Vec<u8> {
    let mut m = header(FOLLOW_UP, 44, clock, correction_ns << 16);
    let secs = timestamp_ns / 1_000_000_000;
    let nanos = (timestamp_ns % 1_000_000_000) as u32;
    m[34..40].copy_from_slice(&secs.to_be_bytes()[2..]);
    m[40..44].copy_from_slice(&nanos.to_be_bytes());
    m
}

fn follow_up_parts(clock: u64, secs: u64, nanos: u32, correction_ns: i64) -> Vec<u8> {
    let mut m = header(FOLLOW_UP, 44, clock, correction_ns << 16);
    m[34..40].copy_from_slice(&secs.to_be_bytes()[2..]);
    m[40..44].copy_from_slice(&nanos.to_be_bytes());
    m
}

/// Смещение считается от big-endian секунд/наносекунд и correctionField,
/// сдвинутого на 16 бит; накопительный фильтр не меняет установившееся значение.
#[test]
fn follow_up_offset_uses_big_endian_time_and_scaled_correction() {
    let mut ptp = Ptp::new(7);
    ptp.on_packet(ip(5), &announce(GM), 0);
    for i in 0..10u64 {
        let local = 10_000_000_000 + i * 125_000_000;
        let master = local + 1_000_000_000_000;
        ptp.on_packet(ip(5), &follow_up_ns(GM, master, 250), local);
    }
    let timeline = ptp.timeline(GM).expect("шкала после Follow_Up");
    assert_eq!(timeline.offset_ns, 1_000_000_000_250);
    assert_eq!(timeline.clock_id, GM);
    assert_eq!(ptp.latest().unwrap().clock_id, GM);
}

#[test]
fn announce_is_required_and_foreign_clock_is_ignored() {
    let mut ptp = Ptp::new(7);
    ptp.on_packet(ip(5), &follow_up_ns(GM, 5_000_000_000, 0), 0);
    assert!(ptp.latest().is_none(), "Follow_Up без Announce");

    ptp.on_packet(ip(5), &announce(GM), 0);
    let other = GM ^ 0xFF;
    ptp.on_packet(ip(5), &follow_up_ns(other, 5_000_000_000, 0), 0);
    assert!(ptp.latest().is_none(), "чужой clockIdentity");
}

/// Наносекунды >= 1 с и 48-битные секунды за пределом u64 отклоняются без
/// паники: раньше умножение переполняло счётчик на пакете из сети.
#[test]
fn malformed_timestamps_are_refused_without_panicking() {
    let mut ptp = Ptp::new(7);
    ptp.on_packet(ip(5), &announce(GM), 0);

    for nanos in [1_000_000_000u32, u32::MAX] {
        ptp.on_packet(ip(5), &follow_up_parts(GM, 100, nanos, 0), 0);
        assert!(ptp.latest().is_none(), "nanos={nanos}");
    }
    // 2^48-1 секунд не помещаются в наносекунды u64.
    ptp.on_packet(ip(5), &follow_up_parts(GM, (1 << 48) - 1, 0, 0), 0);
    assert!(ptp.latest().is_none());

    // Максимум, который ещё помещается: 18 446 744 073 с + 709 551 615 нс.
    ptp.on_packet(
        ip(5),
        &follow_up_parts(GM, 18_446_744_073, 709_551_615, 0),
        0,
    );
    assert!(ptp.timeline(GM).is_some());
}

/// Delay/Signaling/Sync не создают шкалу и не порождают действий.
#[test]
fn delay_signaling_and_sync_do_not_start_a_timeline() {
    let mut ptp = Ptp::new(7);
    ptp.on_packet(ip(5), &announce(GM), 0);
    for (kind, len) in [
        (SYNC, 44),
        (DELAY_REQ, 44),
        (DELAY_RESP, 54),
        (SIGNALING, 64),
        (0xE, 64),
    ] {
        let actions = ptp.on_packet(ip(5), &header(kind, len, GM, 0), 1_000);
        assert!(actions.is_empty(), "kind={kind:#x}");
    }
    assert!(ptp.latest().is_none());
}

/// Неверная версия PTP и любой обрез пакета игнорируются без паники.
#[test]
fn wrong_version_and_every_truncation_are_ignored() {
    let mut ptp = Ptp::new(7);
    let mut wrong = announce(GM);
    wrong[1] = 0x01;
    ptp.on_packet(ip(5), &wrong, 0);
    assert!(ptp.latest().is_none());

    let valid = [announce(GM), follow_up_ns(GM, 5_000_000_000, 0)];
    for packet in valid {
        for cut in 0..packet.len() {
            let mut probe = Ptp::new(7);
            probe.on_packet(ip(5), &packet[..cut], 0);
            probe.on_packet(ip(5), &packet[..cut], 125_000_000);
        }
    }
}
