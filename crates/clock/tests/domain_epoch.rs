//! Перевод между доменами часов при смене эпохи и на границах (C71).
//!
//! Никаких запущенных часов: только аффинные сэмплы с заданными вручную
//! метками. Точность реального PTP здесь не проверяется и не заявляется.

use clock::domain::{ClockMap, Domain, Error, LocalClock, LocalInstant};

const CLOCK: u128 = 7;

fn local(ns: u64) -> LocalInstant {
    LocalClock::new(CLOCK).unwrap().at(ns)
}

fn reading(epoch: u64, ns: u64) -> clock::domain::Reading {
    Domain::new(42, epoch).unwrap().reading(ns)
}

/// Сэмпл со сроком жизни 5 с и явными метками.
fn map(epoch: u64, local_ns: u64, remote_ns: u64, rate_ppb: i64) -> ClockMap {
    ClockMap::new(local(local_ns), reading(epoch, remote_ns), rate_ppb, 5_000_000_000).unwrap()
}

#[test]
fn a_new_epoch_cannot_be_reached_through_the_old_mapping() {
    let old_epoch = map(1, 0, 1_000, 0);
    assert_eq!(
        old_epoch.to_local(reading(2, 1_500), local(10)),
        Err(Error::WrongDomain),
        "метка новой эпохи не переводится старым сэмплом"
    );
    assert_eq!(old_epoch.domain().epoch(), 1);
}

#[test]
fn the_same_domain_id_in_another_epoch_is_distinct() {
    let first = Domain::new(42, 1).unwrap();
    let second = Domain::new(42, 2).unwrap();
    assert_ne!(first, second);
    assert_eq!(first.id(), second.id(), "id на проводе не меняется");
    assert!(Domain::new(0, 1).is_err());
    assert!(Domain::new(42, 0).is_err());
}

#[test]
fn rate_scales_the_offset_in_both_directions() {
    let fast = map(1, 0, 5_000, 500_000); // +0.05 %
    assert_eq!(
        fast.to_network(local(1_000_000_000), local(1_000_000_000)).unwrap().ns(),
        5_000 + 1_000_000_000 + 500_000
    );

    let slow = map(1, 0, 5_000, -500_000);
    assert_eq!(
        slow.to_network(local(1_000_000_000), local(1_000_000_000)).unwrap().ns(),
        5_000 + 1_000_000_000 - 500_000
    );

    // Отрицательное смещение: метка раньше сэмпла даёт меньший номер, а уход
    // ниже нуля — Overflow, а не заворачивание u64.
    let earlier = map(1, 1_000_000_000, 2_000_000_000, 500_000);
    assert_eq!(
        earlier.to_network(local(999_999_001), local(1_000_000_000)).unwrap().ns(),
        2_000_000_000 - 999
    );
    let base_at_one_second = map(1, 1_000_000_000, 0, 0);
    assert_eq!(
        base_at_one_second.to_network(local(1), local(1_000_000_000)),
        Err(Error::Overflow)
    );
}

#[test]
fn zero_rate_makes_the_roundtrip_exact() {
    let mapping = map(1, 1_000, 5_000, 0);
    let target = local(123_456_789);
    let network = mapping.to_network(target, local(2_000)).unwrap();
    let back = mapping.to_local(network, local(2_000)).unwrap();
    assert_eq!(back.ns(), target.ns(), "одна метка не переводится дважды");
    assert_eq!(back.clock(), target.clock());
}

#[test]
fn an_expired_mapping_never_translates() {
    let mapping = ClockMap::new(
        local(0),
        reading(1, 0),
        0,
        1_000_000_000,
    )
    .unwrap();
    assert_eq!(mapping.remaining_ns(local(1_000_000_000)), Ok(0));
    assert_eq!(
        mapping.to_network(local(10), local(1_000_000_001)),
        Err(Error::Stale)
    );
    assert_eq!(
        mapping.to_local(reading(1, 10), local(1_000_000_001)),
        Err(Error::Stale)
    );

    // Время назад — не «свежесть», а отдельная ошибка.
    let later = map(1, 5_000, 0, 0);
    assert_eq!(later.remaining_ns(local(1)), Err(Error::TimeReversed));
    assert_eq!(
        later.to_network(local(10), local(1)),
        Err(Error::TimeReversed)
    );
}

#[test]
fn another_local_clock_is_rejected() {
    let mapping = map(1, 0, 0, 0);
    let other = LocalClock::new(CLOCK + 1).unwrap();
    assert_eq!(
        mapping.to_network(other.at(10), other.at(20)),
        Err(Error::WrongDomain)
    );
}

#[test]
fn out_of_range_inputs_do_not_overflow() {
    let mapping = ClockMap::new(
        local(0),
        reading(1, 0),
        1_000_000,
        u64::MAX,
    )
    .unwrap();
    assert_eq!(
        mapping.to_network(local(u64::MAX), local(0)),
        Err(Error::Overflow)
    );
    assert!(matches!(
        ClockMap::new(local(0), reading(1, 0), -1_000_001, 1),
        Err(Error::Invalid)
    ));
    assert!(
        matches!(
            ClockMap::new(local(0), reading(1, 0), 0, 0),
            Err(Error::Invalid)
        ),
        "сэмпл без срока жизни бесполезен"
    );
}
