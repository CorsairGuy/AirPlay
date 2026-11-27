//! Устаревший TXT после смены сервиса (C74).
//!
//! Синтетические `ResolvedService`, без сети и реальных устройств. Проверяется
//! только таблица разрешённых служб: как обновляются поля и что удаление
//! освобождает запись.

use mdns::discovery::{Kind, ServiceEvent, ServiceInfo, ServiceTable};

fn service(name: &str, port: u16, address: &str, txt: &[(&str, &str)]) -> Box<mdns::discovery::ResolvedService> {
    Box::new(
        ServiceInfo::new(mdns::AIRPLAY, name, "host.local.", address, port, txt)
            .unwrap()
            .as_resolved_service(),
    )
}

fn table() -> ServiceTable {
    ServiceTable::new(Kind::AirPlay)
}

fn txt<'a>(table: &'a ServiceTable, key: &str) -> Option<&'a str> {
    table
        .services()
        .next()
        .and_then(|service| service.get_property_val_str(key))
}

#[test]
fn a_changed_txt_replaces_the_old_value_instead_of_keeping_both() {
    let mut table = table();
    assert!(table.apply(ServiceEvent::ServiceResolved(service(
        "Kitchen",
        7000,
        "192.0.2.10",
        &[("gid", "OLD"), ("model", "AudioAccessory5,1")]
    ))));
    assert_eq!(txt(&table, "gid"), Some("OLD"));

    assert!(
        table.apply(ServiceEvent::ServiceResolved(service(
            "Kitchen",
            7000,
            "192.0.2.10",
            &[("gid", "NEW"), ("model", "AudioAccessory5,1")]
        ))),
        "смена TXT должна уведомлять"
    );
    assert_eq!(table.services().count(), 1);
    assert_eq!(txt(&table, "gid"), Some("NEW"), "старое значение не остаётся");
}

#[test]
fn a_repeated_advertisement_does_not_churn() {
    let mut table = table();
    let ad = || {
        ServiceEvent::ServiceResolved(service(
            "Hall",
            7000,
            "192.0.2.20",
            &[("gid", "G1"), ("model", "X")],
        ))
    };
    assert!(table.apply(ad()));
    assert!(!table.apply(ad()), "тот же снимок не меняет таблицу");
    assert_eq!(table.services().count(), 1);
}

#[test]
fn a_new_port_or_address_replaces_the_endpoint() {
    let mut table = table();
    table.apply(ServiceEvent::ServiceResolved(service(
        "Study",
        7000,
        "192.0.2.30",
        &[("gid", "G")],
    )));
    assert!(table.apply(ServiceEvent::ServiceResolved(service(
        "Study",
        7001,
        "192.0.2.31",
        &[("gid", "G")],
    ))));
    assert_eq!(table.services().count(), 1);
    let service = table.services().next().unwrap();
    assert_eq!(service.port, 7001);
    assert_eq!(
        service.addresses.len(),
        1,
        "старый адрес не остаётся вторым"
    );
}

#[test]
fn removal_frees_the_entry_and_is_reported_once() {
    let mut table = table();
    let resolved = service("Living", 7000, "192.0.2.40", &[("gid", "G")]);
    let fullname = resolved.fullname.clone();
    table.apply(ServiceEvent::ServiceResolved(resolved));
    assert!(table.apply(ServiceEvent::ServiceRemoved(
        mdns::AIRPLAY.to_string(),
        fullname.clone()
    )));
    assert_eq!(table.services().count(), 0);
    assert!(
        !table.apply(ServiceEvent::ServiceRemoved(
            mdns::AIRPLAY.to_string(),
            fullname
        )),
        "повторное удаление ничего не уведомляет"
    );
}

#[test]
fn removal_of_another_service_type_keeps_this_table() {
    let mut table = ServiceTable::new(Kind::Both);
    let resolved = service("Both", 7000, "192.0.2.50", &[("gid", "G")]);
    let fullname = resolved.fullname.clone();
    table.apply(ServiceEvent::ServiceResolved(resolved));
    assert_eq!(table.services().count(), 1);
    assert!(!table.apply(ServiceEvent::ServiceRemoved(
        mdns::RAOP.to_string(),
        fullname.clone()
    )));
    assert_eq!(table.services().count(), 1);
    assert!(table.apply(ServiceEvent::ServiceRemoved(
        mdns::AIRPLAY.to_string(),
        fullname
    )));
    assert_eq!(table.services().count(), 0);
}

#[test]
fn the_table_is_bounded() {
    let mut table = table();
    for i in 0..256 {
        assert!(table.apply(ServiceEvent::ServiceResolved(service(
            &format!("Speaker-{i}"),
            7000,
            "192.0.2.60",
            &[("gid", "G")],
        ))));
    }
    assert_eq!(table.services().count(), 256);
    assert!(
        !table.apply(ServiceEvent::ServiceResolved(service(
            "Overflow",
            7000,
            "192.0.2.60",
            &[("gid", "G")],
        ))),
        "257-я запись отклоняется"
    );
    assert_eq!(table.services().count(), 256);
}
