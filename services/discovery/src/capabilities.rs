//! Разбор объявленных возможностей приёмника и выбор профиля отправки.
//!
//! Источник данных — уже существующая таблица служб (`_airplay._tcp` и
//! `_raop._tcp`), а не второй каталог устройств. Записи одного устройства
//! сопоставляются по MAC из `deviceid` / префикса имени RAOP; всё, что нельзя
//! прочитать или что противоречит друг другу, остаётся `None` и отдельным
//! конфликтом, а не превращается в «подтверждённую» возможность.
use crate::snapshot::OutputCatalog;
use mdns::discovery::{ResolvedService, ServiceTable};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
};

const AUDIO: u64 = 1 << 9;
const SCREEN: u64 = 1 << 7;
const FAIRPLAY: u64 = 1 << 14;
const BUFFERED_AUDIO: u64 = 1 << 40;
const PTP: u64 = 1 << 41;
const HK_PAIRING: u64 = 1 << 46;
const SYSTEM_PAIRING: u64 = 1 << 47;
const COREUTILS_PAIRING: u64 = 1 << 48;

/// Что именно отправляет вызывающий. Профили берутся из возможностей
/// получателя, а не из его модели или имени.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// Потоковый звук AirPlay 2 (realtime).
    Audio,
    /// Зеркалирование экрана.
    Screen,
}

/// Профиль, который действительно реализован этим ядром.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    /// AirPlay 2 audio, SETUP 96/101: единственный профиль отправки звука,
    /// который есть в `media::tx` на этой ветке.
    AirPlay2RealtimeAudio,
}

impl Profile {
    pub fn label(self) -> &'static str {
        match self {
            Self::AirPlay2RealtimeAudio => "AirPlay 2 realtime audio",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalCode {
    /// В таблице нет записи `_airplay._tcp` для этого устройства.
    NoAirPlayRecord,
    /// Обязательное поле отсутствует или не разобрано.
    CapabilitiesUnknown,
    /// Записи одного устройства противоречат друг другу.
    ConflictingDeclarations,
    /// Явно не объявлен звук.
    AudioUnsupported,
    /// Явно не объявлен экран.
    ScreenUnsupported,
    /// Нужен пароль RAOP, которого отправитель не умеет.
    PasswordRequired,
    /// Подходящий профиль известен, но его отправитель экспериментальный и не
    /// включён в выбор: совместимость с устройствами не подтверждена.
    SenderUnavailable,
    /// Каталог не разрешил идентификатор получателя (неизвестен или неоднозначен).
    OutputNotResolved,
    /// Несколько обнаруженных устройств объявляют один и тот же идентификатор.
    AmbiguousMatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefusalCode,
    /// Понятная причина для журнала и интерфейса.
    pub reason: String,
}

/// Результат выбора: либо конкретный поддерживаемый профиль, либо отказ,
/// после которого нельзя открывать сессию «на удачу».
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Start {
        profile: Profile,
        notes: Vec<String>,
    },
    Refuse(Refusal),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairingSupport {
    /// FairPlay-аутентификация (бит 14).
    pub fairplay: Option<bool>,
    /// HomeKit-сопряжение (бит 46).
    pub hk: Option<bool>,
    /// Системное сопряжение (бит 47).
    pub system: Option<bool>,
    /// CoreUtils-сопряжение и шифрование (бит 48).
    pub coreutils: Option<bool>,
    /// RAOP `pw`: `Some(true)` только если ключ объявлен непустым.
    pub password_required: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RaopCapabilities {
    pub channels: Option<u16>,
    pub sample_rate: Option<u32>,
    /// `ft` как объявлено; значение не переводится в список кодеков.
    pub formats: Option<u64>,
    /// `et` как объявлено: биты типов шифрования.
    pub encryption: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    /// Приёмник объявил бит PTP.
    Ptp,
    /// Features известны, бит PTP не выставлен: остаётся NTP.
    Ntp,
}

/// Разобранные возможности. `None` означает «неизвестно», а не «нет».
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub model: Option<String>,
    pub source_version: Option<String>,
    pub accessory_type: Option<u64>,
    /// Полное 64-битное слово `features` (два TXT-слова).
    pub features: Option<u64>,
    pub flags: Option<u32>,
    pub audio: Option<bool>,
    /// Из TXT не выводится: realtime подтверждается только SETUP.
    pub realtime: Option<bool>,
    pub buffered_audio: Option<bool>,
    pub screen: Option<bool>,
    pub timing: Option<Timing>,
    pub pairing: PairingSupport,
    pub raop: Option<RaopCapabilities>,
    /// Имена полей, по которым записи одного устройства разошлись.
    pub conflicts: Vec<&'static str>,
}

impl Capabilities {
    pub fn conflict(&self, field: &str) -> bool {
        self.conflicts.contains(&field)
    }
}

/// Одно физическое устройство, собранное из его записей `_airplay`/`_raop`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// `mac:...` при известном MAC, иначе тип и полное имя записи.
    pub identity: String,
    pub names: Vec<String>,
    /// Все объявленные адреса с портом, IPv4 и IPv6, без выбора «первого».
    pub endpoints: Vec<SocketAddr>,
    pub has_airplay: bool,
    pub has_raop: bool,
    pub capabilities: Capabilities,
}

impl Device {
    pub fn endpoint(&self, want_ipv6: bool) -> Option<SocketAddr> {
        self.endpoints
            .iter()
            .copied()
            .find(|endpoint| endpoint.is_ipv6() == want_ipv6)
    }
}

fn mac(value: &str) -> Option<String> {
    let hex: String = value
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>();
    let only_mac_chars = value
        .chars()
        .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '-');
    (only_mac_chars && hex.len() == 12).then(|| hex.to_ascii_uppercase())
}

fn instance_name(service: &ResolvedService) -> String {
    let suffix = format!(".{}", service.ty_domain);
    service
        .fullname
        .strip_suffix(&suffix)
        .unwrap_or(&service.fullname)
        .to_owned()
}

fn proposed_identity(service: &ResolvedService) -> String {
    if service.ty_domain.eq_ignore_ascii_case(mdns::AIRPLAY) {
        if let Some(id) = service.get_property_val_str("deviceid") {
            return mac(id).map_or_else(
                || format!("deviceid:{}", id.to_ascii_lowercase()),
                |mac| format!("mac:{mac}"),
            );
        }
        if let Some(pi) = service.get_property_val_str("pi") {
            return format!("pi:{}", pi.to_ascii_lowercase());
        }
        if let Some(psi) = service.get_property_val_str("psi") {
            return format!("psi:{}", psi.to_ascii_lowercase());
        }
    } else {
        if let Some(id) = service.get_property_val_str("deviceid") {
            if let Some(mac) = mac(id) {
                return format!("mac:{mac}");
            }
        }
        let instance = instance_name(service);
        if let Some((prefix, _)) = instance.split_once('@') {
            if let Some(mac) = mac(prefix) {
                return format!("mac:{mac}");
            }
        }
    }
    format!(
        "{}:{}",
        service.ty_domain.to_ascii_lowercase(),
        service.fullname.to_ascii_lowercase()
    )
}

fn parse_hex(value: &str) -> Option<u64> {
    let value = value.trim();
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"));
    match digits {
        Some(digits) => u64::from_str_radix(digits, 16).ok(),
        None => value.parse::<u64>().ok(),
    }
}

/// `features` — это `0xНИЖНИЕ32,0xВЕРХНИЕ32`; биты 40/41/46..48 лежат во
/// втором слове, поэтому читать только первое слово нельзя.
fn parse_features(value: &str) -> Option<u64> {
    let mut words = value.split(',');
    let low = parse_hex(words.next()?)?;
    let high = match words.next() {
        Some(word) => parse_hex(word)?,
        None => 0,
    };
    if words.next().is_some() || low > u64::from(u32::MAX) || high > u64::from(u32::MAX) {
        return None;
    }
    Some(low | (high << 32))
}

fn one<T: PartialEq>(values: impl IntoIterator<Item = T>, conflict: &mut bool) -> Option<T> {
    let mut values = values.into_iter();
    let first = values.next()?;
    if values.any(|other| other != first) {
        *conflict = true;
        None
    } else {
        Some(first)
    }
}

fn bool_bit(features: Option<u64>, bit: u64) -> Option<bool> {
    features.map(|features| features & bit != 0)
}

fn build_capabilities(records: &[&ResolvedService]) -> Capabilities {
    let mut caps = Capabilities::default();
    let airplay: Vec<_> = records
        .iter()
        .copied()
        .filter(|service| service.ty_domain.eq_ignore_ascii_case(mdns::AIRPLAY))
        .collect();
    let raop: Vec<_> = records
        .iter()
        .copied()
        .filter(|service| service.ty_domain.eq_ignore_ascii_case(mdns::RAOP))
        .collect();

    let mut model_conflict = false;
    caps.model = one(
        records
            .iter()
            .filter_map(|service| service.get_property_val_str("model"))
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        &mut model_conflict,
    );
    if model_conflict {
        caps.conflicts.push("model");
    }

    let mut version_conflict = false;
    caps.source_version = one(
        records
            .iter()
            .filter_map(|service| service.get_property_val_str("srcvers"))
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        &mut version_conflict,
    );
    if version_conflict {
        caps.conflicts.push("srcvers");
    }

    let mut accessory_conflict = false;
    caps.accessory_type = one(
        airplay
            .iter()
            .filter_map(|service| service.get_property_val_str("at"))
            .filter_map(parse_hex),
        &mut accessory_conflict,
    );
    if accessory_conflict {
        caps.conflicts.push("at");
    }

    let mut features_conflict = false;
    caps.features = one(
        airplay
            .iter()
            .filter_map(|service| service.get_property_val_str("features"))
            .filter_map(parse_features),
        &mut features_conflict,
    );
    if features_conflict {
        caps.conflicts.push("features");
    }

    let mut flags_conflict = false;
    caps.flags = one(
        airplay
            .iter()
            .filter_map(|service| service.get_property_val_str("flags"))
            .filter_map(parse_hex)
            .map(|flags| flags as u32),
        &mut flags_conflict,
    );
    if flags_conflict {
        caps.conflicts.push("flags");
    }

    caps.audio = bool_bit(caps.features, AUDIO);
    caps.buffered_audio = bool_bit(caps.features, BUFFERED_AUDIO);
    caps.screen = bool_bit(caps.features, SCREEN);
    // Realtime в TXT не объявляется: SETUP может ответить только buffered.
    caps.realtime = None;
    caps.timing = caps.features.map(|features| {
        if features & PTP != 0 {
            Timing::Ptp
        } else {
            Timing::Ntp
        }
    });

    caps.pairing.fairplay = bool_bit(caps.features, FAIRPLAY);
    caps.pairing.hk = bool_bit(caps.features, HK_PAIRING);
    caps.pairing.system = bool_bit(caps.features, SYSTEM_PAIRING);
    caps.pairing.coreutils = bool_bit(caps.features, COREUTILS_PAIRING);
    caps.pairing.password_required = one(
        raop.iter()
            .filter_map(|service| service.get_property_val_str("pw"))
            .map(|value| {
                let value = value.trim();
                !(value.is_empty()
                    || value.eq_ignore_ascii_case("false")
                    || value.eq_ignore_ascii_case("no")
                    || value == "0")
            }),
        &mut false,
    );

    if !raop.is_empty() {
        let mut raop_conflict = false;
        let capabilities = RaopCapabilities {
            channels: one(
                raop.iter()
                    .filter_map(|service| service.get_property_val_str("ch"))
                    .filter_map(parse_hex)
                    .map(|channels| channels as u16),
                &mut raop_conflict,
            ),
            sample_rate: one(
                raop.iter()
                    .filter_map(|service| service.get_property_val_str("sr"))
                    .filter_map(parse_hex)
                    .map(|rate| rate as u32),
                &mut raop_conflict,
            ),
            formats: one(
                raop.iter()
                    .filter_map(|service| service.get_property_val_str("ft"))
                    .filter_map(parse_hex),
                &mut raop_conflict,
            ),
            encryption: one(
                raop.iter()
                    .filter_map(|service| service.get_property_val_str("et"))
                    .filter_map(parse_hex),
                &mut raop_conflict,
            ),
        };
        if raop_conflict {
            caps.conflicts.push("raop");
        }
        caps.raop = Some(capabilities);
    }
    caps
}

fn endpoint_of(service: &ResolvedService, address: &mdns::discovery::ScopedIp) -> SocketAddr {
    let mut endpoint = SocketAddr::new(address.to_ip_addr(), service.port);
    if let (SocketAddr::V6(v6), mdns::discovery::ScopedIp::V6(scoped)) = (&mut endpoint, address) {
        v6.set_scope_id(scoped.scope_id().index);
    }
    endpoint
}

fn records_by_device(table: &ServiceTable) -> BTreeMap<String, Vec<&ResolvedService>> {
    let mut devices: BTreeMap<String, Vec<&ResolvedService>> = BTreeMap::new();
    for service in table.services() {
        if !service.ty_domain.eq_ignore_ascii_case(mdns::AIRPLAY)
            && !service.ty_domain.eq_ignore_ascii_case(mdns::RAOP)
        {
            continue;
        }
        devices
            .entry(proposed_identity(service))
            .or_default()
            .push(service);
    }
    devices
}

fn device(identity: String, records: &[&ResolvedService]) -> Device {
    let mut names = Vec::new();
    let mut endpoints = BTreeSet::new();
    let mut has_airplay = false;
    let mut has_raop = false;
    for service in records {
        let airplay = service.ty_domain.eq_ignore_ascii_case(mdns::AIRPLAY);
        has_airplay |= airplay;
        has_raop |= !airplay;
        let name = instance_name(service);
        let name = if airplay {
            name
        } else {
            name.split_once('@')
                .map_or(name.clone(), |(_, suffix)| suffix.to_owned())
        };
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
        for address in &service.addresses {
            endpoints.insert(endpoint_of(service, address));
        }
    }
    Device {
        identity,
        names,
        endpoints: endpoints.into_iter().collect(),
        has_airplay,
        has_raop,
        capabilities: build_capabilities(records),
    }
}

/// Все устройства из существующей таблицы служб, без отдельного хранилища.
pub fn devices(table: &ServiceTable) -> Vec<Device> {
    records_by_device(table)
        .into_iter()
        .map(|(identity, records)| device(identity, &records))
        .collect()
}

/// Идентификаторы, которые запись объявляет о себе: `psi`, `pi`, `deviceid`.
fn declared_ids(service: &ResolvedService) -> [Option<&str>; 3] {
    [
        service.get_property_val_str("psi"),
        service.get_property_val_str("pi"),
        service.get_property_val_str("deviceid"),
    ]
}

/// Сравнение объявленного идентификатора с запрошенным: MAC сравнивается без
/// разделителей и в любом регистре, остальные значения — как есть.
fn same_id(declared: &str, id: &str) -> bool {
    declared.eq_ignore_ascii_case(id)
        || mac(declared)
            .zip(mac(id))
            .is_some_and(|(declared, wanted)| declared == wanted)
}

/// Все устройства, к которым относится идентификатор. Каталог выходов выбирает
/// `psi`, а идентичность устройства строится по `deviceid`/`pi`, поэтому
/// совпадение ищется и по объявленным полям записи, а не только по строке
/// идентичности. Несколько совпадений — это неоднозначность, а не «первое».
fn matching_devices(table: &ServiceTable, id: &str) -> Vec<Device> {
    let mac_id = mac(id).map(|mac| format!("mac:{mac}"));
    records_by_device(table)
        .into_iter()
        .filter_map(|(identity, records)| {
            let device = device(identity, &records);
            let identity_matches = device.identity.eq_ignore_ascii_case(id)
                || mac_id
                    .as_deref()
                    .is_some_and(|key| device.identity.eq_ignore_ascii_case(key))
                || device
                    .identity
                    .eq_ignore_ascii_case(&format!("deviceid:{id}"))
                || device.identity.eq_ignore_ascii_case(&format!("pi:{id}"))
                || device.identity.eq_ignore_ascii_case(&format!("psi:{id}"));
            let matches = identity_matches
                || device.names.iter().any(|name| name == id)
                || records.iter().any(|service| {
                    declared_ids(service)
                        .into_iter()
                        .flatten()
                        .any(|declared| same_id(declared, id))
                });
            matches.then_some(device)
        })
        .collect()
}

/// Устройство по идентификатору записи: `deviceid`, `psi`, `pi`, полное имя
/// или имя экземпляра в каталоге выходов. Неоднозначный идентификатор
/// возвращает `None`: угадывать одно из устройств нельзя.
pub fn device_by_id(table: &ServiceTable, id: &str) -> Option<Device> {
    let mut matches = matching_devices(table, id);
    if matches.len() != 1 {
        return None;
    }
    matches.pop()
}

/// Выбор профиля для устройства из каталога: каталог остаётся владельцем
/// списка получателей, возможности берутся из тех же записей discovery.
pub fn select_for_output(
    table: &ServiceTable,
    catalog: &OutputCatalog,
    uid: &str,
    want: Want,
) -> Selection {
    let route = match catalog.resolve(uid) {
        Ok(route) => route,
        Err(error) => {
            return refuse(
                RefusalCode::OutputNotResolved,
                &format!("получатель не разрешён каталогом: {error}"),
            );
        }
    };
    match matching_devices(table, &route.uid).as_slice() {
        [device] => select(device, want),
        [] => refuse(
            RefusalCode::CapabilitiesUnknown,
            "для получателя нет resolved-записи; профиль неизвестен",
        ),
        _ => refuse(
            RefusalCode::AmbiguousMatch,
            "несколько обнаруженных устройств объявляют этот идентификатор; профиль не выбран",
        ),
    }
}

fn refuse(code: RefusalCode, reason: &str) -> Selection {
    Selection::Refuse(Refusal {
        code,
        reason: reason.into(),
    })
}

/// Единственное место, где возможности превращаются в решение о профиле.
/// Неизвестное или противоречивое поле — отказ, а не оптимистичный запуск.
pub fn select(device: &Device, want: Want) -> Selection {
    if !device.has_airplay {
        return refuse(
            RefusalCode::NoAirPlayRecord,
            "устройство объявляет только _raop._tcp; экспериментальный RAOP-отправитель не подключён к выбору профиля",
        );
    }
    let caps = &device.capabilities;
    if caps.conflict("features") || caps.conflict("flags") {
        return refuse(
            RefusalCode::ConflictingDeclarations,
            "записи _airplay одного устройства объявляют разные features/flags",
        );
    }
    let Some(features) = caps.features else {
        return refuse(
            RefusalCode::CapabilitiesUnknown,
            "TXT features отсутствует или не разобран; профиль отправки неизвестен",
        );
    };
    match want {
        Want::Audio => {
            if features & AUDIO == 0 {
                return refuse(
                    RefusalCode::AudioUnsupported,
                    "приёмник не объявляет бит звука (9) в features",
                );
            }
            if caps.pairing.password_required == Some(true) {
                return refuse(
                    RefusalCode::PasswordRequired,
                    "RAOP требует пароль; отправитель поддерживает только PIN-сопряжение",
                );
            }
            let mut notes = Vec::new();
            if caps.timing == Some(Timing::Ptp) {
                notes.push(
                    "объявлен PTP; SETUP согласует NTP-тайминг, PTP-движок не настраивается"
                        .to_string(),
                );
            }
            if caps.realtime.is_none() {
                notes.push("realtime из TXT не подтверждён: режим проверяется в SETUP".into());
            }
            if caps.buffered_audio == Some(true) {
                notes.push("приёмник объявляет buffered audio".into());
            }
            Selection::Start {
                profile: Profile::AirPlay2RealtimeAudio,
                notes,
            }
        }
        Want::Screen => {
            if features & SCREEN == 0 {
                return refuse(
                    RefusalCode::ScreenUnsupported,
                    "приёмник не объявляет бит экрана (7) в features",
                );
            }
            refuse(
                RefusalCode::SenderUnavailable,
                "экран объявлен, но экспериментальный отправитель зеркалирования не подключён к выбору профиля",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdns::discovery::{ServiceEvent, ServiceInfo};

    fn record(
        service_type: &str,
        name: &str,
        host: &str,
        addresses: &str,
        port: u16,
        txt: &[(&str, &str)],
    ) -> ResolvedService {
        ServiceInfo::new(service_type, name, host, addresses, port, txt)
            .unwrap()
            .as_resolved_service()
    }

    fn table(records: Vec<ResolvedService>) -> ServiceTable {
        let mut table = ServiceTable::new(mdns::discovery::Kind::Both);
        for service in records {
            assert!(table.apply(ServiceEvent::ServiceResolved(Box::new(service))));
        }
        table
    }

    /// TXT по структуре близок к настоящему Apple TV: features двумя словами,
    /// high word несёт биты 40/41/46/47/48.
    fn apple_tv() -> ResolvedService {
        record(
            mdns::AIRPLAY,
            "Гостиная",
            "appletv.local.",
            "192.0.2.10",
            7000,
            &[
                ("deviceid", "AA:BB:CC:DD:EE:FF"),
                ("features", "0x4A7FDFD5,0x3C177FDE"),
                ("flags", "0x4"),
                ("model", "AppleTV14,1"),
                ("srcvers", "960.13.1"),
                ("vv", "2"),
                ("at", "1"),
            ],
        )
    }

    #[test]
    fn apple_tv_txt_is_read_without_inventing_realtime() {
        let table = table(vec![apple_tv()]);
        let devices = devices(&table);
        assert_eq!(devices.len(), 1);
        let device = &devices[0];
        let caps = &device.capabilities;
        assert_eq!(caps.model.as_deref(), Some("AppleTV14,1"));
        assert_eq!(caps.source_version.as_deref(), Some("960.13.1"));
        assert_eq!(caps.accessory_type, Some(1));
        assert_eq!(caps.audio, Some(true));
        assert_eq!(caps.buffered_audio, Some(true));
        assert_eq!(caps.screen, Some(true));
        assert_eq!(caps.timing, Some(Timing::Ptp));
        // Realtime в TXT не объявляется — остаётся неизвестным.
        assert_eq!(caps.realtime, None);
        assert_eq!(caps.pairing.fairplay, Some(true));
        assert_eq!(caps.pairing.hk, Some(true));
        // В этом реальном наборе бит 47 (system pairing) не выставлен.
        assert_eq!(caps.pairing.system, Some(false));
        assert_eq!(caps.pairing.coreutils, Some(true));
        assert_eq!(caps.pairing.password_required, None);
        match select(device, Want::Audio) {
            Selection::Start { profile, notes } => {
                assert_eq!(profile, Profile::AirPlay2RealtimeAudio);
                assert!(notes.iter().any(|note| note.contains("PTP")));
                assert!(notes.iter().any(|note| note.contains("realtime")));
            }
            other => panic!("unexpected selection: {other:?}"),
        }
    }

    #[test]
    fn airplay_and_raop_records_of_one_device_merge_and_keep_both_families() {
        let table = table(vec![
            record(
                mdns::AIRPLAY,
                "Кухня",
                "speaker.local.",
                "192.0.2.10,2001:db8::10",
                7000,
                &[
                    ("deviceid", "AA:BB:CC:DD:EE:FF"),
                    ("features", "0x200,0x0"),
                    ("model", "ThirdParty1,1"),
                ],
            ),
            record(
                mdns::RAOP,
                "AABBCCDDEEFF@Кухня",
                "speaker.local.",
                "192.0.2.10,2001:db8::10",
                7000,
                &[("ch", "2"), ("sr", "44100"), ("et", "0x4"), ("ft", "0x4")],
            ),
        ]);
        let devices = devices(&table);
        assert_eq!(devices.len(), 1);
        let device = &devices[0];
        assert_eq!(device.identity, "mac:AABBCCDDEEFF");
        assert!(device.has_airplay && device.has_raop);
        assert_eq!(device.names, ["Кухня"]);
        assert_eq!(device.endpoints.len(), 2);
        assert!(device.endpoint(false).unwrap().is_ipv4());
        assert!(device.endpoint(true).unwrap().is_ipv6());
        let raop = device.capabilities.raop.as_ref().unwrap();
        assert_eq!(raop.channels, Some(2));
        assert_eq!(raop.sample_rate, Some(44_100));
        assert_eq!(raop.formats, Some(4));
        assert_eq!(raop.encryption, Some(4));
        assert!(matches!(
            select(device, Want::Audio),
            Selection::Start { .. }
        ));
    }

    #[test]
    fn incomplete_txt_refuses_instead_of_assuming_audio() {
        let table = table(vec![record(
            mdns::AIRPLAY,
            "Колонка",
            "speaker.local.",
            "192.0.2.11",
            7000,
            &[("deviceid", "11:22:33:44:55:66"), ("model", "Mystery1,1")],
        )]);
        let device = &devices(&table)[0];
        assert_eq!(device.capabilities.audio, None);
        assert_eq!(device.capabilities.timing, None);
        match select(device, Want::Audio) {
            Selection::Refuse(refusal) => {
                assert_eq!(refusal.code, RefusalCode::CapabilitiesUnknown);
                assert!(refusal.reason.contains("features"));
            }
            other => panic!("unexpected selection: {other:?}"),
        }
    }

    #[test]
    fn contradictory_features_are_unknown_not_optimistic() {
        let table = table(vec![
            record(
                mdns::AIRPLAY,
                "Кухня",
                "one.local.",
                "192.0.2.10",
                7000,
                &[("deviceid", "AA:BB:CC:DD:EE:FF"), ("features", "0x200,0x0")],
            ),
            record(
                mdns::AIRPLAY,
                "Кухня",
                "two.local.",
                "192.0.2.12",
                7000,
                &[("deviceid", "AA:BB:CC:DD:EE:FF"), ("features", "0x0,0x0")],
            ),
        ]);
        let device = &devices(&table)[0];
        assert!(device.capabilities.conflict("features"));
        assert_eq!(device.capabilities.features, None);
        assert_eq!(device.capabilities.audio, None);
        match select(device, Want::Audio) {
            Selection::Refuse(refusal) => {
                assert_eq!(refusal.code, RefusalCode::ConflictingDeclarations)
            }
            other => panic!("unexpected selection: {other:?}"),
        }
    }

    #[test]
    fn raop_only_device_is_refused_naming_the_existing_experimental_sender() {
        let table = table(vec![record(
            mdns::RAOP,
            "AABBCCDDEEFF@Старая колонка",
            "old.local.",
            "192.0.2.13",
            5000,
            &[("ch", "2"), ("sr", "44100")],
        )]);
        let device = &devices(&table)[0];
        assert!(!device.has_airplay && device.has_raop);
        match select(device, Want::Audio) {
            Selection::Refuse(refusal) => {
                assert_eq!(refusal.code, RefusalCode::NoAirPlayRecord);
                assert!(refusal.reason.contains("_raop"));
                // Отказ должен сообщать о статусе, не раскрывая имя модуля пользователю.
                assert!(refusal.reason.contains("не подключён"));
            }
            other => panic!("unexpected selection: {other:?}"),
        }
    }

    #[test]
    fn password_protected_raop_and_missing_screen_support_are_refused() {
        let table = table(vec![record(
            mdns::AIRPLAY,
            "Кухня",
            "speaker.local.",
            "192.0.2.10",
            7000,
            &[("deviceid", "AA:BB:CC:DD:EE:FF"), ("features", "0x200,0x0")],
        )]);
        let mut device = devices(&table).remove(0);
        device.capabilities.pairing.password_required = Some(true);
        match select(&device, Want::Audio) {
            Selection::Refuse(refusal) => {
                assert_eq!(refusal.code, RefusalCode::PasswordRequired)
            }
            other => panic!("unexpected selection: {other:?}"),
        }
        match select(&device, Want::Screen) {
            Selection::Refuse(refusal) => {
                assert_eq!(refusal.code, RefusalCode::ScreenUnsupported)
            }
            other => panic!("unexpected selection: {other:?}"),
        }
        device.capabilities.pairing.password_required = Some(false);
        device.capabilities.features = Some(SCREEN | AUDIO);
        match select(&device, Want::Screen) {
            Selection::Refuse(refusal) => {
                assert_eq!(refusal.code, RefusalCode::SenderUnavailable);
                assert!(refusal.reason.contains("зеркалирования"));
            }
            other => panic!("unexpected selection: {other:?}"),
        }
    }

    /// Одинаково названные колонки на разных хостах — два устройства, а не
    /// одно с двумя адресами.
    #[test]
    fn identically_named_devices_stay_separate() {
        let table = table(vec![
            record(
                mdns::AIRPLAY,
                "Кухня",
                "one.local.",
                "192.0.2.10",
                7000,
                &[("deviceid", "AA:BB:CC:DD:EE:01"), ("features", "0x300,0x0")],
            ),
            record(
                mdns::AIRPLAY,
                "Кухня",
                "two.local.",
                "192.0.2.11",
                7000,
                &[("deviceid", "AA:BB:CC:DD:EE:02"), ("features", "0x300,0x0")],
            ),
        ]);
        let devices = devices(&table);
        assert_eq!(devices.len(), 2);
        assert!(devices.iter().all(|device| device.names == ["Кухня"]));
        assert!(
            devices
                .iter()
                .all(|device| matches!(select(device, Want::Audio), Selection::Start { .. }))
        );
    }
    #[test]
    fn catalog_uid_resolves_to_the_same_device_and_profile() {
        use crate::catalog::{self, Advertisement};
        use std::collections::BTreeMap;

        let table = table(vec![apple_tv()]);
        let txt: BTreeMap<String, String> = [
            ("deviceid", "AA:BB:CC:DD:EE:FF"),
            ("features", "0x4A7FDFD5,0x3C177FDE"),
            ("model", "AppleTV14,1"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        let catalog = catalog::build(
            vec![Advertisement {
                name: "Гостиная".into(),
                endpoint: ([192, 0, 2, 10], 7000).into(),
                txt,
            }],
            &[],
        )
        .unwrap();
        let uid = catalog.routes()[0].uid.clone();
        assert_eq!(uid, "AA:BB:CC:DD:EE:FF");
        assert!(matches!(
            select_for_output(&table, &catalog, &uid, Want::Audio),
            Selection::Start {
                profile: Profile::AirPlay2RealtimeAudio,
                ..
            }
        ));
    }

    /// Каталог выбирает `psi`, а идентичность устройства строится по
    /// `deviceid`. Выбор профиля обязан находить то же устройство по обоим
    /// идентификаторам, а не отказывать из-за разных ключей.
    #[test]
    fn psi_identifier_from_catalog_resolves_to_the_device_identified_by_mac() {
        use crate::catalog::{self, Advertisement};
        use std::collections::BTreeMap;

        let psi = "9F1C2F0A-1111-2222-3333-444455556666";
        let table = table(vec![record(
            mdns::AIRPLAY,
            "Гостиная",
            "appletv.local.",
            "192.0.2.10",
            7000,
            &[
                ("deviceid", "AA:BB:CC:DD:EE:FF"),
                ("psi", psi),
                ("features", "0x200,0x0"),
            ],
        )]);
        let txt: BTreeMap<String, String> = [
            ("deviceid", "AA:BB:CC:DD:EE:FF"),
            ("psi", psi),
            ("features", "0x200,0x0"),
        ]
        .into_iter()
        .map(|(key, value)| (key.into(), value.into()))
        .collect();
        let catalog = catalog::build(
            vec![Advertisement {
                name: "Гостиная".into(),
                endpoint: ([192, 0, 2, 10], 7000).into(),
                txt,
            }],
            &[],
        )
        .unwrap();
        let uid = catalog.routes()[0].uid.clone();
        assert_eq!(uid, psi);
        assert!(matches!(
            select_for_output(&table, &catalog, &uid, Want::Audio),
            Selection::Start {
                profile: Profile::AirPlay2RealtimeAudio,
                ..
            }
        ));
        assert!(matches!(
            select_for_output(&table, &catalog, "AA:BB:CC:DD:EE:FF", Want::Audio),
            Selection::Start { .. }
        ));
        assert!(device_by_id(&table, &uid.to_ascii_lowercase()).is_some());
    }

    /// Записи одного устройства на разных портах сохраняют оба порта: порт
    /// `_raop` не подменяет порт `_airplay` и наоборот.
    #[test]
    fn records_on_different_ports_keep_both_endpoints() {
        let table = table(vec![
            record(
                mdns::AIRPLAY,
                "Кухня",
                "speaker.local.",
                "192.0.2.10",
                7000,
                &[("deviceid", "AA:BB:CC:DD:EE:FF"), ("features", "0x200,0x0")],
            ),
            record(
                mdns::RAOP,
                "AABBCCDDEEFF@Кухня",
                "speaker.local.",
                "192.0.2.10",
                5000,
                &[("ch", "2"), ("sr", "44100")],
            ),
        ]);
        let devices = devices(&table);
        assert_eq!(devices.len(), 1);
        let device = &devices[0];
        let ports: Vec<u16> = device
            .endpoints
            .iter()
            .map(|endpoint| endpoint.port())
            .collect();
        assert_eq!(ports, [5000, 7000]);
        assert!(matches!(
            select(device, Want::Audio),
            Selection::Start { .. }
        ));
    }

    /// Семейство адреса и объявленный порт не перепутываются между записями.
    #[test]
    fn each_address_family_keeps_its_own_declared_port() {
        let txt: &[(&str, &str)] = &[("deviceid", "AA:BB:CC:DD:EE:FF"), ("features", "0x200,0x0")];
        let table = table(vec![
            record(
                mdns::AIRPLAY,
                "Кухня",
                "speaker.local.",
                "192.0.2.10",
                7000,
                txt,
            ),
            record(
                mdns::AIRPLAY,
                "Кухня-ipv6",
                "speaker6.local.",
                "2001:db8::10",
                7001,
                txt,
            ),
        ]);
        let device = &devices(&table)[0];
        assert_eq!(
            device.endpoint(false),
            Some("192.0.2.10:7000".parse::<SocketAddr>().unwrap())
        );
        assert_eq!(
            device.endpoint(true),
            Some("[2001:db8::10]:7001".parse::<SocketAddr>().unwrap())
        );
    }

    /// Отсутствующее семейство адресов не подменяется адресом другого.
    #[test]
    fn a_missing_address_family_has_no_endpoint() {
        let table = table(vec![record(
            mdns::AIRPLAY,
            "Кухня",
            "speaker.local.",
            "2001:db8::10",
            7000,
            &[("deviceid", "AA:BB:CC:DD:EE:FF"), ("features", "0x200,0x0")],
        )]);
        let device = &devices(&table)[0];
        assert!(device.endpoint(false).is_none());
        assert!(device.endpoint(true).is_some());
    }

    /// Неизвестный каталогу идентификатор — свой отказ, а не «нет _airplay».
    #[test]
    fn unresolved_catalog_identifier_has_its_own_refusal() {
        use crate::catalog;

        let table = table(vec![apple_tv()]);
        let catalog = catalog::build(vec![], &[]).unwrap();
        match select_for_output(&table, &catalog, "неизвестный", Want::Audio) {
            Selection::Refuse(refusal) => {
                assert_eq!(refusal.code, RefusalCode::OutputNotResolved)
            }
            other => panic!("unexpected selection: {other:?}"),
        }
    }

    /// Общее имя у двух устройств не должно превращаться в выбор «первого».
    #[test]
    fn ambiguous_name_is_not_resolved_to_the_first_device() {
        let table = table(vec![
            record(
                mdns::AIRPLAY,
                "Кухня",
                "one.local.",
                "192.0.2.10",
                7000,
                &[("deviceid", "AA:BB:CC:DD:EE:01"), ("features", "0x200,0x0")],
            ),
            record(
                mdns::AIRPLAY,
                "Кухня",
                "two.local.",
                "192.0.2.11",
                7000,
                &[("deviceid", "AA:BB:CC:DD:EE:02"), ("features", "0x200,0x0")],
            ),
        ]);
        assert_eq!(devices(&table).len(), 2);
        assert!(device_by_id(&table, "Кухня").is_none());
        assert!(device_by_id(&table, "AA:BB:CC:DD:EE:01").is_some());
    }
}
