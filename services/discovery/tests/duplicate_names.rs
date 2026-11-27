//! Одинаковые имена при разных идентичностях (C82).
//!
//! Синтетические объявления, никаких реальных устройств и пользовательских
//! идентификаторов. Идентичность выводится из psi/pi/deviceid, но не из имени.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use discovery::catalog::{Advertisement, build};

fn address(port: u16) -> SocketAddr {
    SocketAddr::from(([192, 0, 2, 10], port))
}

fn advertisement(name: &str, port: u16, txt: &[(&str, &str)]) -> Advertisement {
    Advertisement {
        name: name.into(),
        endpoint: address(port),
        txt: txt
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// Первый word features с битом 9 (audio) — иначе объявление не маршрут.
const AUDIO: &str = "0x5A7FFFF7,0x1E";

#[test]
fn two_receivers_with_one_name_stay_separate_outputs() {
    let catalog = build(
        vec![
            advertisement("Kitchen", 7000, &[("features", AUDIO), ("psi", "UID-A")]),
            advertisement("Kitchen", 7001, &[("features", AUDIO), ("deviceid", "UID-B")]),
        ],
        &[],
    )
    .unwrap();
    let routes = catalog.routes();
    assert_eq!(routes.len(), 2, "имя не объединяет две идентичности");
    assert!(routes.iter().all(|route| route.name == "Kitchen"));
    assert_ne!(routes[0].uid, routes[1].uid);
    assert_eq!(catalog.resolve("UID-A").unwrap().uid, "UID-A");
    assert_eq!(catalog.resolve("UID-B").unwrap().uid, "UID-B");
}

#[test]
fn the_same_identity_announced_twice_is_rejected_not_merged() {
    // Один psi с разными портами: выбирать адрес молча нельзя.
    let error = match build(
        vec![
            advertisement("Desk", 7000, &[("features", AUDIO), ("psi", "SAME")]),
            advertisement("Desk", 7001, &[("features", AUDIO), ("psi", "SAME")]),
        ],
        &[],
    ) {
        Ok(_) => panic!("дубликат идентичности должен отклоняться"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("duplicate"), "{error}");
}

#[test]
fn a_uuid_identity_is_compared_case_insensitively() {
    let id = "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE";
    let catalog = build(
        vec![advertisement("Hall", 7000, &[("features", AUDIO), ("psi", id)])],
        &[],
    )
    .unwrap();
    assert_eq!(catalog.routes().len(), 1);
    assert_eq!(catalog.resolve(&id.to_lowercase()).unwrap().uid, id);
}

#[test]
fn aliases_of_one_receiver_are_not_treated_as_two() {
    // psi и deviceid одной записи — псевдонимы, а не два устройства.
    let catalog = build(
        vec![advertisement(
            "Study",
            7000,
            &[
                ("features", AUDIO),
                ("psi", "UID-C"),
                ("deviceid", "UID-C-ALIAS"),
            ],
        )],
        &[],
    )
    .unwrap();
    assert_eq!(catalog.routes().len(), 1);
    let route = &catalog.routes()[0];
    assert!(route.aliases.contains(&"UID-C".to_string()));
    assert_eq!(catalog.resolve("UID-C").unwrap().uid, "UID-C");
}

#[test]
fn the_same_name_in_different_groups_keeps_groups_apart() {
    let catalog = build(
        vec![
            advertisement(
                "Living",
                7000,
                &[
                    ("features", AUDIO),
                    ("psi", "LEADER"),
                    ("gid", "GROUP-1"),
                    ("igl", "1"),
                ],
            ),
            advertisement(
                "Living",
                7001,
                &[("features", AUDIO), ("psi", "MEMBER"), ("gid", "GROUP-2")],
            ),
        ],
        &[],
    )
    .unwrap();
    let routes = catalog.routes();
    let groups: Vec<_> = routes
        .iter()
        .filter(|route| route.uid.starts_with("GROUP"))
        .map(|route| route.uid.as_str())
        .collect();
    assert_eq!(groups, ["GROUP-1"], "вторая группа без лидера не создаётся");
    assert_eq!(
        catalog.resolve("GROUP-1").unwrap().receivers.len(),
        1,
        "в группу не подмешивается одинаково названный член другой группы"
    );
}

#[test]
fn a_receiver_without_audio_feature_is_not_a_route() {
    let catalog = build(
        vec![advertisement(
            "Speaker",
            7000,
            &[("features", "0x0,0x1E"), ("psi", "VIDEO-ONLY")],
        )],
        &[],
    )
    .unwrap();
    assert!(catalog.routes().is_empty());
}

#[test]
fn a_local_identity_is_never_listed_as_a_remote() {
    let catalog = build(
        vec![advertisement(
            "Mine",
            7000,
            &[("features", AUDIO), ("psi", "SELF")],
        )],
        &["SELF".into()],
    )
    .unwrap();
    assert!(catalog.routes().is_empty());
}
