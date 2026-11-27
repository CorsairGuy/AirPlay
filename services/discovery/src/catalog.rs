//! Station's capability/declared-group discovery rules, without model filters.
//! Admission is not proof of AP2 realtime compatibility or group adoption.
pub use crate::snapshot::{OutputCatalog, OutputRoute, ReceiverEndpoint};
use anyhow::{Result, bail};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
};

pub struct Advertisement {
    pub name: String,
    pub endpoint: SocketAddr,
    pub txt: BTreeMap<String, String>,
}

fn key(id: &str) -> String {
    uuid::Uuid::parse_str(id)
        .ok()
        .filter(|_| id.len() == 36)
        .map(|id| id.to_string())
        .unwrap_or_else(|| id.into())
}
struct Physical {
    route: OutputRoute,
    group: String,
    leader: bool,
}

/// Caller selects the reachable interface/address explicitly. IPv6 scope is
/// retained in SocketAddr; this adapter does not pick an arbitrary first IP.
pub fn from_services(
    table: &mdns::discovery::ServiceTable,
    local_ids: &[String],
    mut endpoint: impl FnMut(&mdns::discovery::ResolvedService) -> Option<SocketAddr>,
) -> Result<OutputCatalog> {
    let mut advertisements = Vec::new();
    for service in table.services() {
        if !service.ty_domain.eq_ignore_ascii_case(mdns::AIRPLAY) {
            continue;
        }
        let Some(address) = endpoint(service) else {
            continue;
        };
        if address.port() != service.port {
            bail!("selected endpoint port differs from advertisement");
        }
        let suffix = format!(".{}", mdns::AIRPLAY);
        let fullname = &service.fullname;
        if !fullname.to_ascii_lowercase().ends_with(&suffix) {
            bail!("invalid AirPlay service name");
        }
        let name = fullname[..fullname.len() - suffix.len()].to_owned();
        let txt = ["features", "psi", "pi", "deviceid", "model", "gid", "igl"]
            .into_iter()
            .filter_map(|key| {
                service
                    .get_property_val_str(key)
                    .map(|value| (key.into(), value.into()))
            })
            .collect();
        advertisements.push(Advertisement {
            name,
            endpoint: address,
            txt,
        });
    }
    build(advertisements, local_ids)
}

pub fn build(advertisements: Vec<Advertisement>, local_ids: &[String]) -> Result<OutputCatalog> {
    if advertisements.len() > 256 {
        bail!("too many service advertisements");
    }
    let local: BTreeSet<_> = local_ids.iter().map(|id| key(id)).collect();
    let mut physical = BTreeMap::new();
    for ad in advertisements {
        let audio = ad
            .txt
            .get("features")
            .and_then(|text| text.split(',').next())
            .and_then(|word| {
                u64::from_str_radix(
                    word.trim()
                        .trim_start_matches("0x")
                        .trim_start_matches("0X"),
                    16,
                )
                .ok()
            })
            .is_some_and(|bits| bits & (1 << 9) != 0);
        if !audio {
            continue;
        }
        let Some(uid) = ad
            .txt
            .get("psi")
            .or_else(|| ad.txt.get("deviceid"))
            .cloned()
        else {
            continue;
        };
        let aliases: Vec<_> = ["psi", "pi", "deviceid"]
            .into_iter()
            .filter_map(|k| ad.txt.get(k).cloned())
            .collect();
        if aliases.iter().any(|id| local.contains(&key(id))) {
            continue;
        }
        let group = ad.txt.get("gid").cloned().unwrap_or_default();
        let leader = ad.txt.get("igl").is_some_and(|v| v == "1");
        let route = OutputRoute {
            uid: uid.clone(),
            name: ad.name,
            model: ad.txt.get("model").cloned().unwrap_or_default(),
            aliases,
            receivers: vec![ReceiverEndpoint {
                uid: uid.clone(),
                address: ad.endpoint,
            }],
        };
        // Two advertisements for one identity must not silently select an IP.
        if physical
            .insert(
                key(&uid),
                Physical {
                    route,
                    group,
                    leader,
                },
            )
            .is_some()
        {
            bail!("duplicate physical receiver identity");
        }
    }
    let mut routes: Vec<_> = physical.values().map(|p| p.route.clone()).collect();
    let mut groups = BTreeSet::new();
    for leader in physical
        .values()
        .filter(|p| p.leader && !p.group.is_empty())
    {
        let group = key(&leader.group);
        if leader.route.aliases.iter().any(|id| key(id) == group) {
            continue;
        }
        if local.contains(&group) {
            continue;
        }
        if !groups.insert(group.clone()) {
            bail!("multiple declared leaders for one group");
        }
        let mut members = leader.route.receivers.clone();
        for member in physical
            .values()
            .filter(|p| key(&p.group) == group && key(&p.route.uid) != key(&leader.route.uid))
        {
            members.extend(member.route.receivers.clone());
        }
        routes.push(OutputRoute {
            uid: leader.group.clone(),
            name: leader.route.name.clone(),
            model: leader.route.model.clone(),
            aliases: vec![],
            receivers: members,
        });
    }
    let catalog = OutputCatalog::new(routes)?;
    // Validate all aliases and cross-route endpoint identities before publication.
    let ids: Vec<_> = catalog.routes().iter().map(|r| r.uid.clone()).collect();
    catalog.receivers(&ids)?;
    for route in catalog.routes() {
        for alias in &route.aliases {
            catalog.resolve(alias)?;
        }
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ad(id: &str, port: u16, group: &str, leader: bool) -> Advertisement {
        Advertisement {
            name: id.into(),
            endpoint: ([127, 0, 0, 1], port).into(),
            txt: [
                ("psi", id),
                ("features", "0x200,0x0"),
                ("gid", group),
                ("igl", if leader { "1" } else { "0" }),
                ("model", "AnyAudioReceiver"),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect(),
        }
    }
    #[test]
    fn declared_group_expands_leader_and_all_speakers_without_model_filter() {
        let catalog = build(
            vec![
                ad("tv", 7000, "theater", true),
                ad("left", 7001, "theater", false),
                ad("right", 7002, "theater", false),
            ],
            &[],
        )
        .unwrap();
        let members = catalog
            .receivers(&["theater".into(), "left".into()])
            .unwrap();
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].uid, "tv");
        assert_eq!(catalog.resolve("left").unwrap().uid, "left");
        assert_eq!(catalog.resolve("theater").unwrap().name, "tv");
    }
    #[test]
    fn excludes_local_and_non_audio_but_does_not_invent_a_leader() {
        let mut video = ad("video", 7000, "", false);
        video.txt.insert("features".into(), "0x80".into());
        let catalog = build(
            vec![
                video,
                ad("local", 7001, "group", true),
                ad("speaker", 7002, "unled", false),
            ],
            &["local".into()],
        )
        .unwrap();
        assert_eq!(catalog.routes().len(), 1);
        assert!(catalog.resolve("unled").is_err());
    }
    #[test]
    fn duplicate_ids_and_multiple_group_leaders_are_rejected() {
        assert!(
            build(
                vec![ad("a", 7000, "", false), ad("a", 7001, "", false)],
                &[]
            )
            .is_err()
        );
        assert!(
            build(
                vec![ad("a", 7000, "group", true), ad("b", 7001, "group", true)],
                &[]
            )
            .is_err()
        );
    }

    #[test]
    fn ipv6_scope_and_port_are_preserved_per_receiver() {
        use std::net::{Ipv6Addr, SocketAddrV6};
        let mut v6 = ad("tv6", 7000, "", false);
        v6.endpoint = SocketAddr::V6(SocketAddrV6::new(
            "fe80::1234".parse::<Ipv6Addr>().unwrap(),
            7000,
            0,
            3,
        ));
        let catalog = build(vec![v6, ad("v4", 7001, "", false)], &[]).unwrap();
        assert_eq!(catalog.routes().len(), 2);

        let route = catalog.resolve("tv6").unwrap();
        match route.receivers[0].address {
            SocketAddr::V6(address) => {
                assert_eq!(address.scope_id(), 3);
                assert_eq!(address.port(), 7000);
            }
            other => panic!("ожидался IPv6, получен {other}"),
        }
        let SocketAddr::V4(v4) = catalog.resolve("v4").unwrap().receivers[0].address else {
            panic!("ожидался IPv4");
        };
        assert_eq!(v4.port(), 7001);
    }

    /// UUID сравниваются без учёта регистра, поэтому локальное устройство
    /// исключается, даже если TXT и список локальных ID записаны по-разному.
    #[test]
    fn local_uuid_is_excluded_case_insensitively() {
        let upper = "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE";
        let catalog = build(vec![ad(&upper.to_lowercase(), 7000, "", false)], &[upper.into()])
            .unwrap();
        assert!(catalog.routes().is_empty());
    }

    /// Один порт у разных идентичностей не означает одно устройство:
    /// маршруты остаются раздельными, адрес не подменяется.
    #[test]
    fn same_port_different_identities_stay_separate() {
        let mut left = ad("left", 7000, "", false);
        left.endpoint = ([127, 0, 0, 2], 7000).into();
        let mut right = ad("right", 7000, "", false);
        right.endpoint = ([127, 0, 0, 3], 7000).into();
        let catalog = build(vec![left, right], &[]).unwrap();
        assert_eq!(catalog.routes().len(), 2);
        assert_eq!(catalog.resolve("left").unwrap().receivers[0].uid, "left");
        assert_eq!(catalog.resolve("right").unwrap().receivers[0].uid, "right");
        let selected = catalog.receivers(&["left".into()]).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].address.ip().to_string(), "127.0.0.2");
    }

    /// Без `psi` идентичность берётся из `deviceid`; испорченные или
    /// отсутствующие `features` не делают устройство аудиоприёмником.
    #[test]
    fn deviceid_fallback_and_invalid_features() {
        let mut no_psi = ad("ignored", 7000, "", false);
        no_psi.txt.remove("psi");
        no_psi.txt.insert("deviceid".into(), "AA:BB:CC:DD:EE:FF".into());
        let catalog = build(vec![no_psi], &[]).unwrap();
        assert_eq!(catalog.resolve("AA:BB:CC:DD:EE:FF").unwrap().uid, "AA:BB:CC:DD:EE:FF");

        for features in ["", "zzz", "0x0", "0x100,0x200"] {
            let mut media = ad("media", 7000, "", false);
            media.txt.insert("features".into(), features.into());
            assert!(
                build(vec![media], &[]).unwrap().routes().is_empty(),
                "features={features}"
            );
        }
    }

    /// Исчезновение и повторное объявление строят каталог заново, без
    /// устаревших маршрутов и без кэша между вызовами.
    #[test]
    fn removal_and_readvertisement_leave_no_stale_routes() {
        let both = || vec![ad("a", 7000, "", false), ad("b", 7001, "", false)];
        assert_eq!(build(both(), &[]).unwrap().routes().len(), 2);
        let gone = build(vec![ad("a", 7000, "", false)], &[]).unwrap();
        assert!(gone.resolve("b").is_err());
        let back = build(both(), &[]).unwrap();
        assert_eq!(back.resolve("b").unwrap().uid, "b");
    }

    #[test]
    fn more_than_256_advertisements_are_rejected() {
        let ads: Vec<_> = (0..257)
            .map(|index| ad(&format!("id{index}"), 7000, "", false))
            .collect();
        assert!(build(ads, &[]).is_err());
    }

}
