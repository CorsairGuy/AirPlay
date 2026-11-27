//! Отозвание доверия контроллера (C43).
//!
//! После отзыва повторный pair-verify с прежним идентификатором обязан
//! отклоняться, а чужие записи остаются нетронутыми. Доверие хранится в
//! синтетическом `MemoryStore`; реальные ключи устройств не участвуют.

use std::sync::Arc;

use pair::hap::{
    ControllerSetup, ControllerVerify, PairSetup, PairVerify, SetupOutcome, VerifyOutcome,
};
use pair::{Identity, MemoryStore, PairingStore};

fn accessory() -> Identity {
    Identity::from_seed("acc-1", &[21; 32])
}

fn controller(seed: u8, id: &str) -> Identity {
    Identity::from_seed(id, &[seed; 32])
}

fn reply(o: SetupOutcome) -> Vec<u8> {
    match o {
        SetupOutcome::Continue(r) | SetupOutcome::Rejected(r) => r,
        _ => panic!("ожидался Continue"),
    }
}

/// Полный PIN-pair-setup: возвращает приёмник, с которым можно проверить связь.
fn pair_controller(store: &MemoryStore, ctl: Identity) -> pair::hap::Accessory {
    let mut server = PairSetup::new(accessory(), "1234");
    let mut client = ControllerSetup::new(ctl, "1234", false);
    let m2 = reply(server.handle(&client.m1(), store).unwrap());
    let m4 = reply(server.handle(&client.m3(&m2).unwrap(), store).unwrap());
    assert!(client.check_m4(&m4).unwrap().is_none());
    let SetupOutcome::Paired { reply: m6, .. } =
        server.handle(&client.m5().unwrap(), store).unwrap()
    else {
        panic!("ожидалось Paired");
    };
    client.check_m6(&m6).unwrap()
}

fn verify_once(store: &MemoryStore, ctl: Identity, acc: &pair::hap::Accessory) -> bool {
    let mut server = PairVerify::new(accessory());
    let mut client = ControllerVerify::new(
        ctl,
        pair::hap::Accessory {
            id: acc.id.clone(),
            public_key: acc.public_key,
        },
    );
    let VerifyOutcome::Continue(m2) = server.handle(&client.m1(), store).unwrap() else {
        panic!();
    };
    let m3 = client.m3(&m2).unwrap();
    matches!(
        server.handle(&m3, store).unwrap(),
        VerifyOutcome::Verified { .. }
    )
}

#[test]
fn add_get_remove_roundtrip() {
    let store = MemoryStore::default();
    assert!(store.is_empty());
    store.add(b"ctl-1", [1; 32]);
    assert_eq!(store.len(), 1);
    assert_eq!(store.get(b"ctl-1"), Some([1; 32]));

    assert!(store.remove(b"ctl-1").unwrap());
    assert_eq!(store.len(), 0);
    assert_eq!(store.get(b"ctl-1"), None);
    // Повторный отзыв — не успех.
    assert!(!store.remove(b"ctl-1").unwrap());
}

#[test]
fn revoking_one_controller_keeps_the_other() {
    let store = MemoryStore::default();
    store.add(b"ctl-1", [1; 32]);
    store.add(b"ctl-2", [2; 32]);
    assert!(store.remove(b"ctl-1").unwrap());
    assert_eq!(store.len(), 1);
    assert_eq!(store.get(b"ctl-1"), None);
    assert_eq!(store.get(b"ctl-2"), Some([2; 32]));
}

#[test]
fn verify_after_revoke_is_rejected() {
    let store = MemoryStore::default();
    let acc = pair_controller(&store, controller(31, "ctl-1"));
    assert_eq!(store.len(), 1);
    assert!(verify_once(&store, controller(31, "ctl-1"), &acc));

    assert!(store.remove(b"ctl-1").unwrap());
    assert!(
        !verify_once(&store, controller(31, "ctl-1"), &acc),
        "отозванный контроллер не должен проходить pair-verify"
    );
}

#[test]
fn revoking_unknown_id_does_not_change_state() {
    let store = MemoryStore::default();
    let acc = pair_controller(&store, controller(32, "ctl-1"));
    assert!(!store.remove(b"nobody").unwrap());
    assert_eq!(store.len(), 1);
    assert!(verify_once(&store, controller(32, "ctl-1"), &acc));
}

#[test]
fn failed_pair_setup_writes_nothing() {
    let store = MemoryStore::default();
    let mut server = PairSetup::new(accessory(), "1234");
    let mut client = ControllerSetup::new(controller(33, "ctl-bad"), "9999", false);
    let m2 = reply(server.handle(&client.m1(), &store).unwrap());
    reply(server.handle(&client.m3(&m2).unwrap(), &store).unwrap());
    assert!(store.is_empty(), "неудачный PIN не должен сохранять ключ");
}

#[test]
fn concurrent_add_and_remove_keep_the_store_consistent() {
    let store = Arc::new(MemoryStore::default());
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let store = store.clone();
            std::thread::spawn(move || {
                for round in 0..50u8 {
                    let id = format!("ctl-{i}-{round}").into_bytes();
                    store.add(&id, [i as u8; 32]);
                    assert!(store.remove(&id).unwrap());
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert!(store.is_empty());
}
