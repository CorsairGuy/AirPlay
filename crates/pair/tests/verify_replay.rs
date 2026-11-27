//! Pair-verify: подмена идентичности, неверный ключ и повтор сообщений (C38).
//!
//! Используются только синтетические идентичности `Identity::from_seed`;
//! настоящие ключи устройств не участвуют.

use pair::hap::{Accessory, ControllerVerify, PairVerify, VerifyOutcome};
use pair::{Identity, MemoryStore, PairingStore};

fn accessory() -> Identity {
    Identity::from_seed("acc-1", &[11; 32])
}

fn controller(seed: u8, id: &str) -> Identity {
    Identity::from_seed(id, &[seed; 32])
}

fn expected_accessory() -> Accessory {
    Accessory {
        id: b"acc-1".to_vec(),
        public_key: accessory().public_key(),
    }
}

/// M3 для неизвестного приёмнику контроллера отклоняется, ключи не выдаются.
#[test]
fn unknown_controller_is_rejected_without_keys() {
    let store = MemoryStore::default();
    let mut server = PairVerify::new(accessory());
    let mut client = ControllerVerify::new(controller(2, "ctl-1"), expected_accessory());

    let VerifyOutcome::Continue(m2) = server.handle(&client.m1(), &store).unwrap() else {
        panic!("ожидался Continue на M1");
    };
    let m3 = client.m3(&m2).unwrap();
    assert!(matches!(
        server.handle(&m3, &store).unwrap(),
        VerifyOutcome::Rejected(_)
    ));
}

/// Идентификатор известен, но сохранён другой публичный ключ: подпись не
/// сходится, проверка обязана провалиться.
#[test]
fn key_identity_mismatch_is_rejected() {
    let store = MemoryStore::default();
    store.add(b"ctl-1", accessory().public_key());
    let mut server = PairVerify::new(accessory());
    let mut client = ControllerVerify::new(controller(3, "ctl-1"), expected_accessory());

    let VerifyOutcome::Continue(m2) = server.handle(&client.m1(), &store).unwrap() else {
        panic!();
    };
    let m3 = client.m3(&m2).unwrap();
    assert!(matches!(
        server.handle(&m3, &store).unwrap(),
        VerifyOutcome::Rejected(_)
    ));
}

/// M3 без предшествующего M1 — неверный порядок, а не новая сессия.
#[test]
fn m3_before_m1_is_a_state_error() {
    let store = MemoryStore::default();
    let client = ControllerVerify::new(controller(4, "ctl-1"), expected_accessory());
    // Даже корректно зашифрованный M3 не принимается без pending-сессии.
    let m3 = pair::tlv8::Tlv8::new()
        .with(pair::tlv8::STATE, [3])
        .with(pair::tlv8::ENCRYPTED_DATA, [0u8; 40])
        .encode();
    assert!(client.check_m4(&m3).is_err());
    let mut server = PairVerify::new(accessory());
    assert!(server.handle(&m3, &store).is_err());
}

/// Повтор M3 после успешной проверки не открывает второй сессии.
#[test]
fn replaying_m3_after_success_fails_closed() {
    let store = MemoryStore::default();
    store.add(b"ctl-1", controller(5, "ctl-1").public_key());

    let mut server = PairVerify::new(accessory());
    let mut client = ControllerVerify::new(controller(5, "ctl-1"), expected_accessory());
    let VerifyOutcome::Continue(m2) = server.handle(&client.m1(), &store).unwrap() else {
        panic!();
    };
    let m3 = client.m3(&m2).unwrap();
    let VerifyOutcome::Verified { keys, shared, .. } = server.handle(&m3, &store).unwrap() else {
        panic!("первая проверка должна пройти");
    };
    // Ключи канала выводятся из общего секрета; направление read/write разное.
    assert_eq!(keys.read, crypto::kdf::ControlKeys::accessory(&shared).read);
    assert_ne!(keys.read, keys.write);

    // Тот же M3 второй раз: pending уже израсходован.
    assert!(server.handle(&m3, &store).is_err());
}

/// Повтор M1 сбрасывает pending: M3 от прежнего обмена не принимается.
#[test]
fn replaying_m1_invalidates_the_previous_exchange() {
    let store = MemoryStore::default();
    let mut server = PairVerify::new(accessory());
    let mut first = ControllerVerify::new(controller(6, "ctl-1"), expected_accessory());
    let second = ControllerVerify::new(controller(7, "ctl-2"), expected_accessory());

    let VerifyOutcome::Continue(m2_first) = server.handle(&first.m1(), &store).unwrap() else {
        panic!();
    };
    // Новый M1 до завершения прежнего обмена.
    let VerifyOutcome::Continue(_) = server.handle(&second.m1(), &store).unwrap() else {
        panic!();
    };
    // M3 из первого обмена зашифрован ключом первой сессии.
    let stale = first.m3(&m2_first).unwrap();

    // Приёмник уже работает со вторым обменом: старый M3 не расшифруется
    // ключом второй сессии и не даст Verified.
    match server.handle(&stale, &store) {
        Ok(VerifyOutcome::Rejected(_)) => {}
        Ok(VerifyOutcome::Verified { .. }) => panic!("устаревший M3 принят"),
        Ok(VerifyOutcome::Continue(_)) => panic!("неожиданный Continue"),
        Err(_) => {}
    }
}

/// Клиент не получает ключи канала до M4; общий секрет DH существует уже после
/// M3 и документирован как таковой (задача C38).
#[test]
fn client_channel_keys_require_m4_even_though_shared_secret_exists_after_m3() {
    let store = MemoryStore::default();
    let mut server = PairVerify::new(accessory());
    let mut client = ControllerVerify::new(controller(8, "ctl-1"), expected_accessory());
    assert!(client.session_key().is_none());
    let VerifyOutcome::Continue(m2) = server.handle(&client.m1(), &store).unwrap() else {
        panic!();
    };
    let m3 = client.m3(&m2).unwrap();
    // Возможность канала шифруется только в ответе M4; `session_key()` отдаёт
    // сырой DH-секрет и не заменяет проверку.
    assert!(client.session_key().is_some());
    // Без M4 ключей канала у клиента нет: `check_m4` на мусоре отклоняется.
    assert!(client.check_m4(&[]).is_err());
    assert!(matches!(
        server.handle(&m3, &store).unwrap(),
        VerifyOutcome::Rejected(_)
    ));
    assert!(client.check_m4(&m3).is_err(), "M3 не может служить M4");
}

/// Подпись приёмника в M2 проверяется по сохранённому ключу: подмена
/// публичного ключа приёмника отклоняется клиентом.
#[test]
fn controller_rejects_foreign_accessory_signature() {
    let store = MemoryStore::default();
    let mut server = PairVerify::new(accessory());
    let mut client = ControllerVerify::new(
        controller(9, "ctl-1"),
        Accessory {
            id: b"acc-1".to_vec(),
            public_key: controller(10, "other").public_key(),
        },
    );
    let VerifyOutcome::Continue(m2) = server.handle(&client.m1(), &store).unwrap() else {
        panic!();
    };
    assert!(client.m3(&m2).is_err());
}
