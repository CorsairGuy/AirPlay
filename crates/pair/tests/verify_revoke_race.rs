//! Гонка pair-verify и отзыв доверия (C61).
//!
//! Синтетические ключи, общее `MemoryStore` за `Arc`. Проверяется, что при
//! конкуренции возможны только два исхода — verify до отзыва или отказ, — и что
//! после отзыва новые verify отклоняются. Поведение настоящего устройства
//! здесь не проверяется: см. docs/PAIRING-REVOKE-POLICY.md.

use std::sync::Arc;

use pair::hap::{
    Accessory, ControllerSetup, ControllerVerify, PairSetup, PairVerify, SetupOutcome, VerifyOutcome,
};
use pair::{Identity, MemoryStore, PairingStore, Revoke};

fn accessory() -> Identity {
    Identity::from_seed("acc-race", &[41; 32])
}

fn controller() -> Identity {
    Identity::from_seed("ctl-race", &[42; 32])
}

fn reply(o: SetupOutcome) -> Vec<u8> {
    match o {
        SetupOutcome::Continue(r) | SetupOutcome::Rejected(r) => r,
        _ => panic!("ожидался Continue"),
    }
}

fn pair(store: &MemoryStore) -> Accessory {
    let mut server = PairSetup::new(accessory(), "1234");
    let mut client = ControllerSetup::new(controller(), "1234", false);
    let m2 = reply(server.handle(&client.m1(), store).unwrap());
    let m4 = reply(server.handle(&client.m3(&m2).unwrap(), store).unwrap());
    assert!(client.check_m4(&m4).unwrap().is_none());
    let SetupOutcome::Paired { reply: m6, .. } = server.handle(&client.m5().unwrap(), store).unwrap()
    else {
        panic!("ожидалось Paired");
    };
    client.check_m6(&m6).unwrap()
}

fn clone_accessory(acc: &Accessory) -> Accessory {
    Accessory {
        id: acc.id.clone(),
        public_key: acc.public_key,
    }
}

fn verify(store: &MemoryStore, acc: &Accessory) -> Result<bool, String> {
    let mut server = PairVerify::new(accessory());
    let mut client = ControllerVerify::new(controller(), clone_accessory(acc));
    let m2 = match server.handle(&client.m1(), store).map_err(|e| e.to_string())? {
        VerifyOutcome::Continue(reply) => reply,
        VerifyOutcome::Rejected(_) => return Ok(false),
        VerifyOutcome::Verified { .. } => return Err("verify завершился до M3".into()),
    };
    let m3 = client.m3(&m2).map_err(|e| e.to_string())?;
    match server.handle(&m3, store).map_err(|e| e.to_string())? {
        VerifyOutcome::Verified { keys, .. } => {
            assert!(
                keys.read != [0; 32] || keys.write != [0; 32],
                "ключи не пустые"
            );
            Ok(true)
        }
        VerifyOutcome::Rejected(_) => Ok(false),
        VerifyOutcome::Continue(_) => Err("ожидалось завершение verify".into()),
    }
}

#[test]
fn a_new_verify_after_revoke_is_rejected() {
    let store = MemoryStore::default();
    let acc = pair(&store);
    assert_eq!(verify(&store, &acc).unwrap(), true);
    assert_eq!(store.remove(b"ctl-race"), Revoke::Revoked);
    assert_eq!(verify(&store, &acc).unwrap(), false);
    assert_eq!(verify(&store, &acc).unwrap(), false, "отказ повторяется");
}

#[test]
fn verify_racing_revoke_has_only_two_outcomes() {
    for round in 0..24 {
        let store = Arc::new(MemoryStore::default());
        let acc = pair(&store);

        let verifier = {
            let store = store.clone();
            let acc = clone_accessory(&acc);
            std::thread::spawn(move || verify(&store, &acc))
        };
        let revoke = store.remove(b"ctl-race");

        let outcome = verifier.join().unwrap().expect("verify без ошибок");
        match revoke {
            Revoke::Revoked => {
                // Ключи могли быть выведены до удаления, но новая проверка
                // обязана провалиться.
                assert_eq!(verify(&store, &acc).unwrap(), false, "раунд {round}");
                assert!(store.is_empty());
            }
            Revoke::NotFound => {
                assert!(outcome, "если отзыв опоздал, verify успел завершиться");
            }
            other => panic!("неожиданный результат отзыва: {other:?}"),
        }
    }
}

#[test]
fn a_revoked_controller_does_not_affect_another_one() {
    let store = MemoryStore::default();
    let acc = pair(&store);
    store.add(b"ctl-other", [9; 32]);
    assert_eq!(store.remove(b"ctl-race"), Revoke::Revoked);
    assert_eq!(store.get(b"ctl-other"), Some([9; 32]));
    assert_eq!(verify(&store, &acc).unwrap(), false);
    assert_eq!(store.len(), 1);
}
