//! Explicitly started DNS-SD browsing and a testable resolved-service table.
//! This layer retains raw TXT/scoped addresses; it does not infer audio support,
//! logical group membership or which network interface an application should use.
use crate::{AIRPLAY, Error, RAOP};
use mdns_sd::ServiceDaemon;
pub use mdns_sd::{Receiver, ResolvedService, ScopedIp, ServiceEvent, ServiceInfo};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    AirPlay,
    Raop,
    /// `_airplay._tcp` и `_raop._tcp` в одной таблице: одно устройство может
    /// объявлять себя обоими типами, и записи нужно сопоставлять, а не
    /// терять половину.
    Both,
}
impl Kind {
    fn service_types(self) -> &'static [&'static str] {
        match self {
            Self::AirPlay => &[AIRPLAY],
            Self::Raop => &[RAOP],
            Self::Both => &[AIRPLAY, RAOP],
        }
    }

    fn accepts(self, service_type: &str) -> bool {
        self.service_types()
            .iter()
            .any(|candidate| service_type.eq_ignore_ascii_case(candidate))
    }
}

pub struct Browser {
    daemon: ServiceDaemon,
    events: Vec<Receiver<ServiceEvent>>,
    service_types: &'static [&'static str],
}
impl Browser {
    /// Starts network discovery only when explicitly called by the application.
    pub fn start(kind: Kind) -> Result<Self, Error> {
        let daemon = ServiceDaemon::new()?;
        let mut events = Vec::new();
        for service_type in kind.service_types() {
            match daemon.browse(service_type) {
                Ok(receiver) => events.push(receiver),
                Err(error) => {
                    let _ = daemon.shutdown();
                    return Err(error.into());
                }
            }
        }
        Ok(Self {
            daemon,
            events,
            service_types: kind.service_types(),
        })
    }

    /// `_airplay._tcp` и `_raop._tcp` одним браузером.
    pub fn start_all() -> Result<Self, Error> {
        Self::start(Kind::Both)
    }

    /// All event streams, one per browsed service type.
    /// Supports blocking, try_recv and recv_async through `recv_async`.
    pub fn receivers(&self) -> &[Receiver<ServiceEvent>] {
        &self.events
    }

    /// First stream, kept for callers that browse a single type.
    pub fn events(&self) -> &Receiver<ServiceEvent> {
        &self.events[0]
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        for service_type in self.service_types {
            let _ = self.daemon.stop_browse(service_type);
        }
        let _ = self.daemon.shutdown();
    }
}

pub struct ServiceTable {
    kind: Kind,
    /// Ключ — (тип службы, полное имя, хост): одинаковые имена на разных
    /// хостах и разные типы одного устройства не должны затирать друг друга.
    services: BTreeMap<(String, String, String), ResolvedService>,
}
fn same(a: &ResolvedService, b: &ResolvedService) -> bool {
    a.ty_domain.eq_ignore_ascii_case(&b.ty_domain)
        && a.sub_ty_domain == b.sub_ty_domain
        && a.fullname.eq_ignore_ascii_case(&b.fullname)
        && a.host.eq_ignore_ascii_case(&b.host)
        && a.port == b.port
        && a.addresses == b.addresses
        && a.txt_properties == b.txt_properties
}
impl ServiceTable {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            services: BTreeMap::new(),
        }
    }
    pub fn services(&self) -> impl Iterator<Item = &ResolvedService> {
        self.services.values()
    }
    pub fn services_of(&self, service_type: &str) -> impl Iterator<Item = &ResolvedService> {
        self.services
            .values()
            .filter(move |service| service.ty_domain.eq_ignore_ascii_case(service_type))
    }
    /// True only when a resolved service changes or a known service disappears.
    /// Repeated advertisements/search notifications do not churn UI snapshots.
    /// A fixed256-service limit bounds retained state, without a model allowlist.
    pub fn apply(&mut self, event: ServiceEvent) -> bool {
        match event {
            ServiceEvent::ServiceResolved(service)
                if self.kind.accepts(&service.ty_domain)
                    && service.is_valid()
                    && service.port != 0 =>
            {
                let key = (
                    service.ty_domain.to_ascii_lowercase(),
                    service.fullname.to_ascii_lowercase(),
                    service.host.to_ascii_lowercase(),
                );
                if self
                    .services
                    .get(&key)
                    .is_some_and(|old| same(old, &service))
                {
                    return false;
                }
                if self.services.len() >= 256 && !self.services.contains_key(&key) {
                    return false;
                }
                self.services.insert(key, *service);
                true
            }
            ServiceEvent::ServiceRemoved(kind, name) if self.kind.accepts(&kind) => {
                let kind = kind.to_ascii_lowercase();
                let name = name.to_ascii_lowercase();
                let before = self.services.len();
                self.services.retain(|(service_type, fullname, _), _| {
                    *service_type != kind || *fullname != name
                });
                self.services.len() != before
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn service(name: &str, port: u16, group: &str) -> ResolvedService {
        mdns_sd::ServiceInfo::new(
            AIRPLAY,
            name,
            "host.local.",
            "127.0.0.1",
            port,
            &[("gid", group), ("model", "arbitrary")][..],
        )
        .unwrap()
        .as_resolved_service()
    }
    #[test]
    fn repeats_do_not_notify_but_endpoint_and_txt_changes_do() {
        let mut table = ServiceTable::new(Kind::AirPlay);
        assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(service(
            "speaker", 7000, "a"
        )))));
        assert!(!table.apply(ServiceEvent::ServiceResolved(Box::new(service(
            "speaker", 7000, "a"
        )))));
        assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(service(
            "speaker", 7001, "a"
        )))));
        assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(service(
            "speaker", 7001, "b"
        )))));
        assert_eq!(table.services().count(), 1);
        assert!(!table.apply(ServiceEvent::SearchStarted(AIRPLAY.into())));
        assert!(table.apply(ServiceEvent::ServiceRemoved(
            AIRPLAY.into(),
            "SPEAKER._airplay._tcp.local.".into()
        )));
        assert!(!table.apply(ServiceEvent::ServiceRemoved(
            AIRPLAY.into(),
            "speaker._airplay._tcp.local.".into()
        )));
    }
    #[test]
    fn another_service_type_cannot_remove_an_airplay_entry() {
        let mut table = ServiceTable::new(Kind::AirPlay);
        assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(service(
            "speaker", 7000, "a"
        )))));
        assert!(!table.apply(ServiceEvent::ServiceRemoved(
            RAOP.into(),
            "speaker._airplay._tcp.local.".into()
        )));
        assert_eq!(table.services().count(), 1);
        let mut table = ServiceTable::new(Kind::Raop);
        assert!(!table.apply(ServiceEvent::ServiceResolved(Box::new(service(
            "speaker", 7000, "a"
        )))));
    }
}

#[cfg(test)]
mod both_tests {
    use super::*;

    fn record(
        service_type: &str,
        name: &str,
        host: &str,
        addresses: &str,
        port: u16,
        txt: &[(&str, &str)],
    ) -> ResolvedService {
        mdns_sd::ServiceInfo::new(service_type, name, host, addresses, port, txt)
            .unwrap()
            .as_resolved_service()
    }

    /// Одно устройство объявляет оба типа: записи сохраняются раздельно, а
    /// обновление адреса не создаёт вторую запись.
    #[test]
    fn airplay_and_raop_records_coexist_and_address_updates_replace_in_place() {
        let mut table = ServiceTable::new(Kind::Both);
        assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(record(
            AIRPLAY,
            "Гостиная",
            "tv.local.",
            "192.0.2.10",
            7000,
            &[
                ("deviceid", "AA:BB:CC:DD:EE:FF"),
                ("features", "0x5A7FFEE6,0x0")
            ],
        )))));
        assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(record(
            RAOP,
            "AABBCCDDEEFF@Гостиная",
            "tv.local.",
            "192.0.2.10",
            7000,
            &[("ch", "2"), ("sr", "44100")],
        )))));
        assert_eq!(table.services().count(), 2);
        // Новый адрес того же AirPlay-имени заменяет старую запись.
        assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(record(
            AIRPLAY,
            "Гостиная",
            "tv.local.",
            "2001:db8::10",
            7000,
            &[
                ("deviceid", "AA:BB:CC:DD:EE:FF"),
                ("features", "0x5A7FFEE6,0x0")
            ],
        )))));
        assert_eq!(table.services().count(), 2);
        let airplay = table.services_of(AIRPLAY).next().unwrap();
        assert!(airplay.addresses.iter().any(|address| address.is_ipv6()));
        // Удаление одного типа не трогает второй.
        assert!(table.apply(ServiceEvent::ServiceRemoved(
            AIRPLAY.into(),
            "Гостиная._airplay._tcp.local.".into()
        )));
        assert_eq!(table.services().count(), 1);
        assert_eq!(table.services_of(RAOP).count(), 1);
        assert!(!table.apply(ServiceEvent::ServiceRemoved(
            AIRPLAY.into(),
            "Гостиная._airplay._tcp.local.".into()
        )));
    }

    /// Одинаковые имена на разных хостах — разные записи; удаление по имени
    /// не должно снимать соседнюю колонку с тем же именем.
    #[test]
    fn identically_named_speakers_on_different_hosts_stay_distinct() {
        let mut table = ServiceTable::new(Kind::Both);
        for (host, address) in [("one.local.", "192.0.2.1"), ("two.local.", "192.0.2.2")] {
            assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(record(
                AIRPLAY,
                "Кухня",
                host,
                address,
                7000,
                &[("deviceid", address)],
            )))));
        }
        assert_eq!(table.services().count(), 2);
    }

    #[test]
    fn single_kind_table_still_ignores_the_other_service_type() {
        let mut table = ServiceTable::new(Kind::AirPlay);
        assert!(!table.apply(ServiceEvent::ServiceResolved(Box::new(record(
            RAOP,
            "AABBCCDDEEFF@Гостиная",
            "tv.local.",
            "192.0.2.10",
            7000,
            &[("ch", "2")],
        )))));
        assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(record(
            AIRPLAY,
            "Гостиная",
            "tv.local.",
            "192.0.2.10",
            7000,
            &[("deviceid", "AA:BB:CC:DD:EE:FF")],
        )))));
        assert_eq!(table.services().count(), 1);
    }
}
