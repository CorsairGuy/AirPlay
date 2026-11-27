//! Пропажа и повторное появление интерфейсов в discovery-каталоге (C34).
//!
//! Модель детерминированная: вместо живого Bonjour подаются объявления с
//! адресами и TXT. Проверяется, что каталог не сохраняет устаревший endpoint и
//! не выбирает адрес сам при конфликте.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use discovery::catalog::{Advertisement, build};

fn ad(name: &str, endpoint: &str, psi: &str, gid: &str) -> Advertisement {
    let mut txt = BTreeMap::new();
    txt.insert("features".into(), "0x200,0x0".into());
    txt.insert("psi".into(), psi.into());
    txt.insert("gid".into(), gid.into());
    txt.insert("igl".into(), "1".into());
    txt.insert("model".into(), "Speaker".into());
    Advertisement {
        name: name.into(),
        endpoint: endpoint.parse::<SocketAddr>().unwrap(),
        txt,
    }
}

#[test]
fn device_disappearing_from_discovery_removes_its_route() {
    let first = build(vec![ad("kitchen", "192.0.2.10:7000", "psi-1", "g")], &[]).unwrap();
    assert!(first.resolve("psi-1").is_ok());

    // Повторный снимок без устройства: маршрут не должен остаться.
    let second = build(vec![], &[]).unwrap();
    assert!(second.resolve("psi-1").is_err());
    assert!(second.receivers(&["psi-1".into()]).is_err());
}

#[test]
fn address_change_replaces_the_old_endpoint() {
    let first = build(vec![ad("kitchen", "192.0.2.10:7000", "psi-1", "g")], &[]).unwrap();
    let second = build(vec![ad("kitchen", "192.0.2.20:7000", "psi-1", "g")], &[]).unwrap();
    assert_eq!(
        second.receiver("psi-1").unwrap().address,
        "192.0.2.20:7000".parse::<SocketAddr>().unwrap()
    );
    // Первый снимок не изменился задним числом.
    assert_eq!(
        first.receiver("psi-1").unwrap().address,
        "192.0.2.10:7000".parse::<SocketAddr>().unwrap()
    );
}

#[test]
fn parallel_advertisements_of_one_device_are_not_guessed() {
    // Одно и то же psi на двух разных адресах: выбирать произвольный нельзя.
    let result = build(
        vec![
            ad("kitchen", "192.0.2.10:7000", "psi-1", "g"),
            ad("kitchen", "192.0.2.10:7001", "psi-1", "g"),
        ],
        &[],
    );
    assert!(
        result.is_err(),
        "конфликт адресов для одного uid должен отклоняться"
    );
}

#[test]
fn ipv6_scope_is_preserved_and_not_collapsed_to_ipv4() {
    let catalog = build(
        vec![
            ad("v4", "192.0.2.10:7000", "psi-4", "g4"),
            ad("v6", "[fe80::1%3]:7000", "psi-6", "g6"),
        ],
        &[],
    )
    .unwrap();
    let v6 = catalog.receiver("psi-6").unwrap().address;
    assert!(v6.is_ipv6());
    assert_eq!(v6.port(), 7000);
    assert_eq!(v6.ip().to_string(), "fe80::1");
    assert_eq!(catalog.receiver("psi-4").unwrap().address.port(), 7000);
}

#[test]
fn local_receiver_is_excluded_by_any_of_its_identifiers() {
    let mut txt_ad = ad("self", "192.0.2.10:7000", "psi-self", "g");
    txt_ad.txt.insert("deviceid".into(), "AA:BB:CC:DD:EE:FF".into());
    let catalog = build(vec![txt_ad], &["AA:BB:CC:DD:EE:FF".into()]).unwrap();
    assert!(catalog.routes().is_empty(), "собственный приёмник не должен попадать в выдачу");
}

#[test]
fn advertisements_without_audio_bit_or_identifier_are_skipped() {
    let mut no_audio = ad("video-only", "192.0.2.10:7000", "psi-1", "g");
    no_audio.txt.insert("features".into(), "0x1,0x0".into());
    let mut no_id = ad("no-id", "192.0.2.10:7001", "psi-2", "g");
    no_id.txt.remove("psi");
    let catalog = build(vec![no_audio, no_id], &[]).unwrap();
    assert!(catalog.routes().is_empty());
}

#[test]
fn port_zero_or_unspecified_addresses_are_rejected() {
    assert!(build(vec![ad("x", "0.0.0.0:7000", "psi-1", "g")], &[]).is_err());
    assert!(build(vec![ad("x", "192.0.2.10:0", "psi-1", "g")], &[]).is_err());
}
