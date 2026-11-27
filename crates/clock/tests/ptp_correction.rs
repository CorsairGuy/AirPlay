//! PTP correctionField и недопустимые пакеты (C72).
//!
//! Синтетические пакеты, собранные побайтово по формату PTP. Никаких сокетов:
//! `services/timesync` и общая PTP-ветка координатора не затрагиваются.

use std::net::IpAddr;

use clock::ptp::Ptp;

const ANNOUNCE: u8 = 0xB;
const SYNC: u8 = 0x0;
const FOLLOW_UP: u8 = 0x8;
const CLOCK: u64 = 0x1122_3344_5566_7788;

fn ip(last: u8) -> IpAddr {
    IpAddr::from([192, 168, 1, last])
}

fn packet(kind: u8, clock: u64, body_ts: Option<u64>, correction_ns: i64) -> Vec<u8> {
    let mut m = vec![0u8; if kind == ANNOUNCE { 64 } else { 44 }];
    m[0] = 0x10 | kind;
    m[1] = 0x02;
    m[8..16].copy_from_slice(&(correction_ns << 16).to_be_bytes());
    m[20..28].copy_from_slice(&clock.to_be_bytes());
    if let Some(body_ts) = body_ts {
        let secs = body_ts / 1_000_000_000;
        m[34..40].copy_from_slice(&secs.to_be_bytes()[2..]);
        m[40..44].copy_from_slice(&((body_ts % 1_000_000_000) as u32).to_be_bytes());
    }
    if kind == ANNOUNCE {
        m[47] = 248;
        m[52] = 248;
        m[53..61].copy_from_slice(&clock.to_be_bytes());
    }
    m
}

/// Десять Follow_Up с постоянным смещением: фильтр выдаёт установившееся значение.
fn feed(p: &mut Ptp, clock: u64, offset: i128, correction_ns: i64) {
    p.on_packet(ip(5), &packet(ANNOUNCE, clock, None, 0), 0);
    for i in 0..10u64 {
        let local = 5_000_000_000 + i * 125_000_000;
        let master = local as i128 + offset;
        let master = u64::try_from(master).expect("смещение должно быть положительным");
        p.on_packet(ip(5), &packet(SYNC, clock, None, 0), local);
        p.on_packet(
            ip(5),
            &packet(FOLLOW_UP, clock, Some(master), correction_ns),
            local,
        );
    }
}

#[test]
fn a_signed_correction_is_applied_in_nanoseconds() {
    let mut p = Ptp::new(7);
    feed(&mut p, CLOCK, 1_000_000_000, 250);
    assert_eq!(p.timeline(CLOCK).unwrap().offset_ns, 1_000_000_250);

    let mut negative = Ptp::new(7);
    feed(&mut negative, CLOCK, 1_000_000_000, -250);
    assert_eq!(
        negative.timeline(CLOCK).unwrap().offset_ns,
        999_999_750,
        "отрицательная коррекция вычитается"
    );
}

#[test]
fn correction_with_fractional_nanoseconds_is_truncated_toward_zero() {
    // correctionField хранит наносекунды в старших 48 битах, младшие 16 — доли.
    let mut p = Ptp::new(7);
    p.on_packet(ip(5), &packet(ANNOUNCE, CLOCK, None, 0), 0);
    for i in 0..10u64 {
        let local = 1_000 + i * 100;
        p.on_packet(ip(5), &packet(SYNC, CLOCK, None, 0), local);
        let master = local + 1_000;
        let mut follow = packet(FOLLOW_UP, CLOCK, Some(master), 0);
        // 1.5 нс коррекции: 0x0000_0000_0001_8000.
        follow[8..16].copy_from_slice(&0x1_8000i64.to_be_bytes());
        p.on_packet(ip(5), &follow, local);
    }
    assert_eq!(
        p.timeline(CLOCK).unwrap().offset_ns,
        1_001,
        "дробная часть отбрасывается"
    );
}

#[test]
fn an_unknown_clock_is_not_reported() {
    let mut p = Ptp::new(7);
    feed(&mut p, CLOCK, 1_000, 0);
    assert!(p.timeline(CLOCK).is_some());
    assert!(
        p.timeline(0xdead_beef).is_none(),
        "чужой идентификатор не получает шкалу"
    );
}

#[test]
fn a_follow_up_with_another_clock_id_is_ignored() {
    let mut p = Ptp::new(7);
    p.on_packet(ip(5), &packet(ANNOUNCE, CLOCK, None, 0), 0);
    for i in 0..10u64 {
        let local = 1_000 + i * 100;
        p.on_packet(ip(5), &packet(SYNC, CLOCK, None, 0), local);
        p.on_packet(
            ip(5),
            &packet(FOLLOW_UP, CLOCK ^ 1, Some(local + 5_000), 0),
            local,
        );
    }
    assert!(
        p.timeline(CLOCK).is_none(),
        "пакет другого источника не двигает шкалу"
    );
}

#[test]
fn an_out_of_range_offset_updates_nothing() {
    let mut p = Ptp::new(7);
    p.on_packet(ip(5), &packet(ANNOUNCE, CLOCK, None, 0), 0);
    // 292 года разницы не помещаются в i64 наносекунд.
    p.on_packet(ip(5), &packet(FOLLOW_UP, CLOCK, Some(u64::MAX - 1), 0), 0);
    assert!(p.timeline(CLOCK).is_none());

    // Большая, но представимая коррекция применяется целиком (1000 с), а
    // отрицательная — вычитается; переполнения нет.
    let mut positive = Ptp::new(7);
    positive.on_packet(ip(5), &packet(ANNOUNCE, CLOCK, None, 0), 0);
    positive.on_packet(
        ip(5),
        &packet(FOLLOW_UP, CLOCK, Some(5_000_000_000), 1_000_000_000_000),
        0,
    );
    assert_eq!(
        positive.timeline(CLOCK).unwrap().offset_ns,
        1_005_000_000_000
    );

    let mut negative = Ptp::new(7);
    negative.on_packet(ip(5), &packet(ANNOUNCE, CLOCK, None, 0), 0);
    negative.on_packet(
        ip(5),
        &packet(FOLLOW_UP, CLOCK, Some(0), -1_000_000_000_000),
        2_000_000_000_000,
    );
    assert_eq!(
        negative.timeline(CLOCK).unwrap().offset_ns,
        -3_000_000_000_000
    );
}

#[test]
fn malformed_packets_are_rejected() {
    let mut p = Ptp::new(7);
    p.on_packet(ip(5), &packet(ANNOUNCE, CLOCK, None, 0), 0);

    // Наносекунды ≥ 1e9 недопустимы.
    let mut bad = packet(FOLLOW_UP, CLOCK, Some(0), 0);
    bad[40..44].copy_from_slice(&1_000_000_000u32.to_be_bytes());
    p.on_packet(ip(5), &bad, 0);

    // Короткий пакет и чужая версия протокола.
    p.on_packet(ip(5), &[0x08, 0x02], 0);
    let mut wrong_version = packet(FOLLOW_UP, CLOCK, Some(1), 0);
    wrong_version[1] = 0x01;
    p.on_packet(ip(5), &wrong_version, 0);

    assert!(p.timeline(CLOCK).is_none());
    assert!(p.latest().is_none());
}
