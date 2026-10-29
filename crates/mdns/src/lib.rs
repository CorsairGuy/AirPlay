//! Bonjour (mDNS/DNS-SD): объявление служб приёмника.

use mdns_sd::{ServiceDaemon, ServiceInfo};

pub const AIRPLAY: &str = "_airplay._tcp.local.";
pub const RAOP: &str = "_raop._tcp.local.";

#[derive(Debug, thiserror::Error)]
#[error("mDNS: {0}")]
pub struct Error(#[from] mdns_sd::Error);

/// Одна служба для объявления.
pub struct Service {
    pub service_type: &'static str,
    pub instance: String,
    pub port: u16,
    pub txt: Vec<(String, String)>,
}

/// Объявляет службы, пока живёт. При удалении снимает объявления.
pub struct Advertiser {
    daemon: ServiceDaemon,
    registered: Vec<String>,
}

impl Advertiser {
    /// `host` — имя узла без `.local.`, адреса берутся со всех интерфейсов.
    pub fn start(host: &str, services: Vec<Service>) -> Result<Self, Error> {
        let daemon = ServiceDaemon::new()?;
        let host = format!("{host}.local.");
        let mut registered = Vec::new();
        for s in services {
            let info =
                ServiceInfo::new(s.service_type, &s.instance, &host, "", s.port, &s.txt[..])?
                    .enable_addr_auto();
            registered.push(info.get_fullname().to_owned());
            daemon.register(info)?;
        }
        Ok(Self { daemon, registered })
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        for name in &self.registered {
            let _ = self.daemon.unregister(name);
        }
        let _ = self.daemon.shutdown();
    }
}
