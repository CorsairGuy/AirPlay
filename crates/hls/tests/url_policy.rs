//! Политика URL и redirect для HLS (C35).
//!
//! Доступ к локальным адресам реальным устройствам бывает нужен, поэтому
//! политика явная: литеральные частные/локальные адреса отклоняются на входе,
//! а redirect проверяется отдельно (`validate_redirect`). Разрешение имён
//! проверяется в fetch-слое; здесь фиксируется граница разбора URL.
//! Тесты не ходят в сеть.

use hls::{Error, Url, validate_redirect};

#[test]
fn only_http_and_https_are_accepted() {
    for ok in ["http://8.8.8.8/p.m3u8", "https://8.8.8.8/p.m3u8"] {
        assert!(Url::parse(ok).is_ok(), "{ok}");
    }
    for bad in [
        "file:///etc/passwd",
        "ftp://192.0.2.1/p.m3u8",
        "rtsp://192.0.2.1/p",
        "data:text/plain,hi",
        "not a url",
    ] {
        assert!(Url::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn credentials_are_rejected() {
    assert_eq!(
        Url::parse("https://user:pass@8.8.8.8/p.m3u8"),
        Err(Error::Credentials)
    );
    assert_eq!(
        Url::parse("https://user@8.8.8.8/p.m3u8"),
        Err(Error::Credentials)
    );
    // Пустой пароль (только `:`) тоже считается учётными данными.
    assert_eq!(
        Url::parse("https://:pass@8.8.8.8/p.m3u8"),
        Err(Error::Credentials)
    );
}

#[test]
fn non_public_literal_addresses_are_rejected() {
    for bad in [
        "http://127.0.0.1/p.m3u8",
        "http://10.1.2.3/p.m3u8",
        "http://192.168.1.1/p.m3u8",
        "http://172.16.0.1/p.m3u8",
        "http://169.254.1.1/p.m3u8",
        "http://100.64.0.1/p.m3u8",
        "http://0.0.0.0/p.m3u8",
        "http://224.0.0.1/p.m3u8",
        "http://[::1]/p.m3u8",
        "http://[fe80::1]/p.m3u8",
        "http://[fd00::1]/p.m3u8",
        "http://[::ffff:192.168.0.1]/p.m3u8",
    ] {
        assert_eq!(
            Url::parse(bad),
            Err(Error::PrivateAddress),
            "{bad} должен отклоняться"
        );
    }
    for ok in [
        "http://8.8.8.8/p.m3u8",
        "https://1.1.1.1/p.m3u8",
        "https://[2001:4860::1]/p.m3u8",
        // Имя разрешается позже: политика литералов его не блокирует.
        "https://media.example.com/p.m3u8",
    ] {
        assert!(Url::parse(ok).is_ok(), "{ok}");
    }
}

#[test]
fn relative_references_are_revalidated_after_join() {
    let base = Url::parse("https://192.0.2.1/a/p.m3u8").unwrap();
    let next = base.join("next.m3u8").unwrap();
    assert_eq!(next.as_str(), "https://192.0.2.1/a/next.m3u8");
    // Ссылка на локальный адрес отклоняется при соединении, а не только на входе.
    assert!(base.join("http://127.0.0.1/x.m3u8").is_err());
    assert!(base.join("http://10.0.0.1/x.m3u8").is_err());
}

#[test]
fn redirect_policy_blocks_https_downgrade_only() {
    let https = Url::parse("https://1.1.1.1/p").unwrap();
    let http = Url::parse("http://1.1.1.1/p").unwrap();
    let other_https = Url::parse("https://9.9.9.9/q").unwrap();
    let other_http = Url::parse("http://9.9.9.9/q").unwrap();

    assert_eq!(validate_redirect(&https, &other_https), Ok(()));
    assert_eq!(validate_redirect(&https, &http), Err(Error::UnsafeRedirect));
    assert_eq!(validate_redirect(&http, &https), Ok(()));
    assert_eq!(validate_redirect(&http, &other_http), Ok(()));
}

#[test]
fn redirect_target_itself_must_already_be_a_safe_url() {
    // validate_redirect получает уже разобранные Url: локальный адрес до неё
    // не доходит, потому что Url::parse отклоняет его раньше.
    assert!(Url::parse("http://127.0.0.1/redirected").is_err());
    assert!(Url::parse("http://[::1]:8080/x").is_err());
}

#[test]
fn query_and_fragment_do_not_bypass_the_policy() {
    assert_eq!(
        Url::parse("http://127.0.0.1/p.m3u8?token=abc#frag"),
        Err(Error::PrivateAddress)
    );
    assert_eq!(
        Url::parse("https://user:pass@127.0.0.1/p.m3u8"),
        Err(Error::Credentials)
    );
}
