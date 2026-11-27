//! Жизненный цикл объявлений при нескольких экземплярах (C46).
//!
//! Тесты идут на `ServiceTable` без запуска Bonjour: события подаются
//! детерминированно, сеть не затрагивается.

use mdns::discovery::{Kind, ServiceTable};
use mdns_sd::{ServiceEvent, ServiceInfo};

fn resolved(service_type: &str, name: &str, host: &str, port: u16) -> ServiceEvent {
    let info = ServiceInfo::new(
        service_type,
        name,
        host,
        "127.0.0.1",
        port,
        &[("gid", name)][..],
    )
    .unwrap();
    ServiceEvent::ServiceResolved(Box::new(info.as_resolved_service()))
}

#[test]
fn several_instances_are_kept_independently() {
    let mut table = ServiceTable::new(Kind::AirPlay);
    assert!(table.apply(resolved(mdns::AIRPLAY, "one", "host.local.", 7000)));
    assert!(table.apply(resolved(mdns::AIRPLAY, "two", "host.local.", 7001)));
    assert_eq!(table.services().count(), 2);

    // Снятие одного экземпляра не трогает второй.
    assert!(table.apply(ServiceEvent::ServiceRemoved(
        mdns::AIRPLAY.into(),
        "ONE._airplay._tcp.local.".into()
    )));
    let left: Vec<_> = table.services().map(|s| s.port).collect();
    assert_eq!(left, vec![7001]);
}

#[test]
fn same_name_on_two_hosts_does_not_overwrite() {
    let mut table = ServiceTable::new(Kind::AirPlay);
    assert!(table.apply(resolved(mdns::AIRPLAY, "speaker", "a.local.", 7000)));
    assert!(table.apply(resolved(mdns::AIRPLAY, "speaker", "b.local.", 7000)));
    assert_eq!(
        table.services().count(),
        2,
        "конфликт имён не должен терять запись"
    );

    // Удаление по имени снимает оба объявления этого имени.
    assert!(table.apply(ServiceEvent::ServiceRemoved(
        mdns::AIRPLAY.into(),
        "speaker._airplay._tcp.local.".into()
    )));
    assert_eq!(table.services().count(), 0);
}

#[test]
fn invalid_and_foreign_events_are_ignored() {
    let mut table = ServiceTable::new(Kind::AirPlay);
    // Порт 0 нерегистрируем.
    assert!(!table.apply(resolved(mdns::AIRPLAY, "bad", "host.local.", 0)));
    // Чужой тип службы при AirPlay-таблице.
    assert!(!table.apply(resolved(mdns::RAOP, "raop", "host.local.", 5000)));
    // Удаление неизвестного и события поиска не меняют состояние.
    assert!(!table.apply(ServiceEvent::ServiceRemoved(
        mdns::AIRPLAY.into(),
        "nobody._airplay._tcp.local.".into()
    )));
    assert!(!table.apply(ServiceEvent::SearchStarted(mdns::AIRPLAY.into())));
    assert_eq!(table.services().count(), 0);
}

#[test]
fn raop_table_and_airplay_table_are_separate() {
    let mut airplay = ServiceTable::new(Kind::AirPlay);
    let mut raop = ServiceTable::new(Kind::Raop);
    assert!(!airplay.apply(resolved(mdns::RAOP, "raop-one", "host.local.", 5000)));
    assert!(raop.apply(resolved(mdns::RAOP, "raop-one", "host.local.", 5000)));
    assert_eq!(airplay.services().count(), 0);
    assert_eq!(raop.services_of(mdns::RAOP).count(), 1);
}

#[test]
fn table_is_bounded_at_256_services() {
    let mut table = ServiceTable::new(Kind::AirPlay);
    for i in 0..300u16 {
        table.apply(resolved(
            mdns::AIRPLAY,
            &format!("s{i}"),
            "host.local.",
            7000 + i,
        ));
    }
    assert!(
        table.services().count() <= 256,
        "таблица не ограничена: {}",
        table.services().count()
    );
}

#[test]
fn removing_one_service_releases_only_it() {
    let mut table = ServiceTable::new(Kind::AirPlay);
    for (i, name) in ["a", "b", "c"].iter().enumerate() {
        assert!(table.apply(resolved(
            mdns::AIRPLAY,
            name,
            "host.local.",
            7000 + i as u16
        )));
    }
    assert!(table.apply(ServiceEvent::ServiceRemoved(
        mdns::AIRPLAY.into(),
        "b._airplay._tcp.local.".into()
    )));
    let ports: Vec<_> = table.services().map(|s| s.port).collect();
    assert_eq!(ports, vec![7000, 7002]);
}
