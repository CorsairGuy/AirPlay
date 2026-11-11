//! Explicitly started DNS-SD browsing and a testable resolved-service table.
//! This layer retains raw TXT/scoped addresses; it does not infer audio support,
//! logical group membership or which network interface an application should use.
use crate::{AIRPLAY, Error, RAOP};
use mdns_sd::ServiceDaemon;
pub use mdns_sd::{Receiver, ResolvedService, ServiceEvent};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub enum Kind {
    AirPlay,
    Raop,
}
impl Kind {
    fn service_type(self) -> &'static str {
        match self {
            Self::AirPlay => AIRPLAY,
            Self::Raop => RAOP,
        }
    }
}

pub struct Browser {
    daemon: ServiceDaemon,
    events: Receiver<ServiceEvent>,
    service_type: &'static str,
}
impl Browser {
    /// Starts network discovery only when explicitly called by the application.
    pub fn start(kind: Kind) -> Result<Self, Error> {
        let daemon = ServiceDaemon::new()?;
        let service_type = kind.service_type();
        let events = match daemon.browse(service_type) {
            Ok(events) => events,
            Err(error) => {
                let _ = daemon.shutdown();
                return Err(error.into());
            }
        };
        Ok(Self {
            daemon,
            events,
            service_type,
        })
    }
    /// Supports blocking, try_recv and recv_async through the installed library.
    pub fn events(&self) -> &Receiver<ServiceEvent> {
        &self.events
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(self.service_type);
        let _ = self.daemon.shutdown();
    }
}

pub struct ServiceTable {
    service_type: &'static str,
    services: BTreeMap<String, ResolvedService>,
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
            service_type: kind.service_type(),
            services: BTreeMap::new(),
        }
    }
    pub fn services(&self) -> impl Iterator<Item = &ResolvedService> {
        self.services.values()
    }
    /// True only when a resolved service changes or a known service disappears.
    /// Repeated advertisements/search notifications do not churn UI snapshots.
    /// A fixed256-service limit bounds retained state, without a model allowlist.
    pub fn apply(&mut self, event: ServiceEvent) -> bool {
        match event {
            ServiceEvent::ServiceResolved(service)
                if service.ty_domain.eq_ignore_ascii_case(self.service_type)
                    && service.is_valid()
                    && service.port != 0 =>
            {
                let key = service.fullname.to_ascii_lowercase();
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
            ServiceEvent::ServiceRemoved(kind, name)
                if kind.eq_ignore_ascii_case(self.service_type) =>
            {
                self.services.remove(&name.to_ascii_lowercase()).is_some()
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
