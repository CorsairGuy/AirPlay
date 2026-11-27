//! Независимое наблюдение сетевых доменов PTP (C32).
//!
//! Только публичные типы `clock::domain`. Общий локальный oscillator не
//! считается общим сетевым временем: у каждого домена свои clock id и эпоха, а
//! mapping имеет срок действия. NQPTP и общие PTP-модули не затрагиваются.

use clock::domain::{ClockMap, Domain, Error, LocalClock, Reading, translate};

fn local() -> LocalClock {
    LocalClock::new(0xA1B2_C3D4_E5F6_0001).unwrap()
}

fn at(ns: u64) -> clock::domain::LocalInstant {
    local().at(ns)
}

fn map(id: u64, epoch: u64, local_ns: u64, remote_ns: u64) -> ClockMap {
    ClockMap::new(
        at(local_ns),
        Domain::new(id, epoch).unwrap().reading(remote_ns),
        0,
        1_000_000_000,
    )
    .unwrap()
}

#[test]
fn different_clock_ids_are_not_interchangeable() {
    let map = map(1, 1, 1_000, 5_000);
    let foreign = Domain::new(2, 1).unwrap().reading(5_000);
    assert!(matches!(
        map.to_local(foreign, at(1_000)),
        Err(Error::WrongDomain)
    ));

    let ours = Domain::new(1, 1).unwrap().reading(5_000);
    assert_eq!(map.to_local(ours, at(1_000)).unwrap().ns(), 1_000);
}

#[test]
fn same_clock_id_with_another_epoch_is_a_different_domain() {
    let map = map(7, 1, 0, 100);
    let next_epoch = Domain::new(7, 2).unwrap().reading(100);
    assert!(matches!(
        map.to_local(next_epoch, at(0)),
        Err(Error::WrongDomain)
    ));
    assert_ne!(
        Domain::new(7, 1).unwrap().reading(100).domain(),
        Domain::new(7, 2).unwrap().reading(100).domain(),
        "эпоха должна входить в идентичность домена"
    );
}

#[test]
fn mapping_expires_and_cannot_be_extended_by_late_calls() {
    let map = map(1, 1, 1_000, 5_000);
    // В пределах срока mapping свеж.
    assert!(map.to_network(at(2_000), at(1_000)).is_ok());
    assert_eq!(map.remaining_ns(at(1_500)).unwrap(), 999_999_500);
    // Ровно на границе срока — ещё действует, дальше — Stale.
    assert!(map.remaining_ns(at(1_000 + 1_000_000_000)).is_ok());
    assert!(matches!(
        map.remaining_ns(at(1_000 + 1_000_000_001)),
        Err(Error::Stale)
    ));
    // Обратное время — TimeReversed, не «свежесть».
    assert!(matches!(
        map.remaining_ns(at(999)),
        Err(Error::TimeReversed)
    ));
}

#[test]
fn mapping_is_anchored_to_one_local_epoch() {
    let other_clock = LocalClock::new(0xA1B2_C3D4_E5F6_0002).unwrap();
    let map = map(1, 1, 1_000, 5_000);
    assert!(matches!(
        map.to_network(other_clock.at(1_000), other_clock.at(1_000)),
        Err(Error::WrongDomain)
    ));
}

#[test]
fn rate_is_applied_in_both_directions() {
    let start = 1_000_000;
    let fast = ClockMap::new(
        at(start),
        Domain::new(1, 1).unwrap().reading(2_000_000),
        1_000, // +1 ppm
        10_000_000_000,
    )
    .unwrap();
    // Через секунду локального времени сетевые часы ушли на 1 c + 1 мкс.
    let reading = fast
        .to_network(at(start + 1_000_000_000), at(start))
        .unwrap();
    assert_eq!(reading.ns(), 2_000_000 + 1_000_000_000 + 1_000);
    // Обратное преобразование возвращает исходный локальный момент.
    let back = fast.to_local(reading, at(start)).unwrap();
    assert_eq!(back.ns(), start + 1_000_000_000);
}

#[test]
fn translate_bridges_two_domains_at_the_same_physical_instant() {
    let now = 1_000;
    let a = map(11, 3, 1_000, 50_000);
    let b = map(22, 4, 1_000, 900_000);
    let incoming = Domain::new(11, 3).unwrap().reading(50_500);
    let out = translate(incoming, &a, &b, at(now)).unwrap();
    assert_eq!(out.domain(), Domain::new(22, 4).unwrap());
    assert_eq!(out.ns(), 900_500);
}

#[test]
fn translate_refuses_a_stale_or_foreign_source() {
    let a = map(11, 3, 1_000, 50_000);
    let b = map(22, 4, 1_000, 900_000);
    let wrong = Domain::new(99, 1).unwrap().reading(50_000);
    assert!(translate(wrong, &a, &b, at(1_000)).is_err());
    // Срок истёк — мост не выполняется, даже если домены совпадают.
    let valid = Domain::new(11, 3).unwrap().reading(50_000);
    assert!(matches!(
        translate(valid, &a, &b, at(1_001_000_001)),
        Err(Error::Stale)
    ));
}

#[test]
fn invalid_rate_or_zero_validity_is_rejected() {
    let reading = Domain::new(1, 1).unwrap().reading(0);
    assert!(
        ClockMap::new(at(0), reading, 1_000_001, 1_000).is_err(),
        "rate больше 1e6 ppb"
    );
    assert!(ClockMap::new(at(0), reading, -1_000_001, 1_000).is_err());
    assert!(ClockMap::new(at(0), reading, 0, 0).is_err());
    assert!(ClockMap::new(at(0), reading, 1_000_000, 1).is_ok());
}

#[test]
fn domain_reading_carries_its_own_identity() {
    let d1 = Domain::new(1, 1).unwrap();
    let d2 = Domain::new(2, 1).unwrap();
    let r1: Reading = d1.reading(10);
    assert_eq!(r1.domain(), d1);
    assert_ne!(r1.domain(), d2);
    assert_eq!(r1.ns(), 10);
}
