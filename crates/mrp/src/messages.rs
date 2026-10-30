//! Сообщения приёмника: описание устройства, выход в списке устройств,
//! карточка «Сейчас играет» с очередью и обложкой, громкость.
//!
//! Поля и их значения — как у airplay_mrp на станции: с ними iPhone
//! показывает карточку, обложку, управление и громкость (проверено на
//! iPhone). Поля HomePod, которые не проверены, не объявляются.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use plist::{Dictionary, Uid, Value};
use sha2::{Digest, Sha256};

use crate::schema::{field as f, kind, value};
use crate::wire::Node;

/// Кем приёмник представляется в MRP.
#[derive(Clone, Debug)]
pub struct Device {
    pub name: String,
    /// `AudioAccessory5,1` — HomePod mini.
    pub model: String,
    pub localized_model: String,
    /// UID устройства (`psi` в `/info`).
    pub uid: String,
    /// Своя группа AirPlay (`pi`).
    pub group: String,
    /// Bundle ID «системного» приложения: в нём играет то, что не назвало
    /// себя.
    pub system_app: String,
}

/// Обложка трека.
#[derive(Clone, Debug, PartialEq)]
pub struct Artwork {
    pub data: Arc<Vec<u8>>,
    /// Меняется вместе с картинкой: по нему iPhone решает, качать ли её.
    pub id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
}

/// Где играет приёмник: группа сессии отправителя и роль в ней.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub group: String,
    pub active: bool,
    pub leader: bool,
    pub contains_leader: bool,
}

/// Что показывает карточка.
#[derive(Clone, Debug, PartialEq)]
pub struct NowPlaying {
    /// Bundle ID источника: свой плеер у каждого источника.
    pub app: String,
    pub app_name: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Меняется с треком.
    pub content_id: String,
    /// Секунды от начала трека.
    pub position: f64,
    pub duration: Option<f64>,
    pub playing: bool,
    /// 0..1.
    pub volume: f32,
    /// Команды и громкость дойдут до источника.
    pub control_available: bool,
    pub can_seek: bool,
    pub artwork: Option<Artwork>,
    pub route: Option<Route>,
}

impl Default for NowPlaying {
    fn default() -> Self {
        Self {
            app: String::new(),
            app_name: String::new(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            content_id: String::new(),
            position: 0.0,
            duration: None,
            playing: false,
            volume: 0.5,
            control_available: false,
            can_seek: false,
            artwork: None,
            route: None,
        }
    }
}

impl NowPlaying {
    fn active_route(&self) -> Option<&Route> {
        self.route.as_ref().filter(|r| r.active)
    }

    /// Группа, в которой сейчас играет приёмник: сессии отправителя или своя.
    pub fn group<'a>(&'a self, device: &'a Device) -> &'a str {
        self.active_route()
            .map(|r| r.group.as_str())
            .unwrap_or(&device.group)
    }

    fn receiving(&self) -> bool {
        self.active_route().is_some()
    }

    fn leader(&self) -> bool {
        self.active_route().is_none_or(|r| r.leader)
    }

    fn contains_leader(&self) -> bool {
        self.active_route().is_none_or(|r| r.contains_leader)
    }
}

/// UID в разном регистре и с фигурными скобками — один и тот же.
pub fn same_uid(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim_matches(['{', '}']).to_ascii_lowercase();
    a == b || norm(a) == norm(b)
}

/// Секунды от 2001-01-01, как у NSDate.
fn cocoa_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        - 978_307_200.0
}

fn uuid(bytes: [u8; 16]) -> String {
    let h = hex::encode_upper(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

fn random_uuid() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    uuid(b)
}

/// Постоянный UUID из строки: один плеер на источник.
fn stable_uuid(name: &str) -> String {
    let d = Sha256::digest(name.as_bytes());
    let mut b: [u8; 16] = d[..16].try_into().unwrap();
    b[6] = (b[6] & 0x0f) | 0x50;
    b[8] = (b[8] & 0x3f) | 0x80;
    uuid(b)
}

/// `ProtocolMessage` типа `kind` с телом `body`; `reply` — идентификатор
/// запроса, на который это ответ.
pub fn envelope(kind: u32, body: &Node, reply: Option<&str>) -> Node {
    use f::ProtocolMessage as p;
    let mut msg = Node::default();
    msg.int(p::r#type, kind.into())
        .int(p::errorCode, 0)
        .text(p::uniqueIdentifier, random_uuid());
    if let Some(id) = reply {
        msg.text(p::identifier, id);
    }
    if let Some(tag) = crate::schema::extension(kind) {
        msg.node(tag, body);
    }
    msg
}

/// Ответ без тела с кодом ошибки (`value::ErrorCode`).
pub fn error_reply(reply: Option<&str>, error: u64) -> Node {
    use f::ProtocolMessage as p;
    let mut msg = envelope(kind::UNKNOWN_MESSAGE, &Node::default(), reply);
    msg.remove(p::errorCode).int(p::errorCode, error);
    msg
}

/// Ответ на `SEND_COMMAND`: `value::SendError`.
pub fn command_reply(reply: Option<&str>, error: u64) -> Node {
    use f::SendCommandResultMessage as m;
    use value::HandlerReturnStatus as status;
    let mut body = Node::default();
    body.int(m::sendError, error).int(
        m::handlerReturnStatus,
        if error == 0 {
            status::Success
        } else {
            status::CommandFailed
        },
    );
    envelope(kind::SEND_COMMAND_RESULT_MESSAGE, &body, reply)
}

fn client(now: &NowPlaying, device: &Device) -> Node {
    use f::NowPlayingClient as c;
    let mut n = Node::default();
    n.int(c::processIdentifier, 1)
        .int(c::processUserIdentifier, 0)
        .text(c::bundleIdentifier, app(now, device))
        .text(c::displayName, display_name(now, device));
    n
}

fn app<'a>(now: &'a NowPlaying, device: &'a Device) -> &'a str {
    if now.app.is_empty() {
        &device.system_app
    } else {
        &now.app
    }
}

fn display_name<'a>(now: &'a NowPlaying, device: &'a Device) -> &'a str {
    if now.app_name.is_empty() {
        &device.name
    } else {
        &now.app_name
    }
}

fn player_path(device: &Device, now: &NowPlaying, local: bool) -> Node {
    let mut origin = Node::default();
    origin
        // 'LOCL': так помечает себя сам mediaremoted.
        .int(f::Origin::identifier, 1_280_262_988)
        .text(f::Origin::displayName, &device.name);
    if local {
        origin.boolean(f::Origin::isLocallyHosted, true);
    }
    let mut player = Node::default();
    player
        .text(
            f::NowPlayingPlayer::identifier,
            stable_uuid(&format!("mrp:{}:{}", device.uid, app(now, device))),
        )
        .text(f::NowPlayingPlayer::displayName, display_name(now, device))
        .int(f::NowPlayingPlayer::audioSessionID, 0);
    let mut n = Node::default();
    n.node(f::PlayerPath::origin, &origin)
        .node(f::PlayerPath::client, &client(now, device))
        .node(f::PlayerPath::player, &player);
    n
}

/// Ответ на `DEVICE_INFO`.
pub fn device_info(device: &Device, now: &NowPlaying, reply: Option<&str>) -> Node {
    use f::DeviceInfoMessage as d;
    let mut n = Node::default();
    for (field, text) in [
        (d::deviceUID, device.uid.as_str()),
        (d::uniqueIdentifier, &device.uid),
        (d::name, &device.name),
        (d::modelID, &device.model),
        (d::localizedModelName, &device.localized_model),
        (d::applicationBundleIdentifier, "com.apple.mediaremoted"),
        (d::groupUID, now.group(device)),
        (d::airPlayGroupID, &device.group),
        (d::groupName, &device.name),
        (d::systemMediaApplication, &device.system_app),
        (d::computerName, &device.name),
    ] {
        n.text(field, text);
    }
    for (field, number) in [
        (d::protocolVersion, 1),
        (d::lastSupportedMessageType, 139),
        (d::deviceClass, 7),
        (d::logicalDeviceCount, 1),
        (d::clusterType, 0),
        (d::configuredClusterSize, 0),
        (
            d::groupContainsDiscoverableGroupLeader,
            now.contains_leader().into(),
        ),
    ] {
        n.int(field, number);
    }
    for (field, flag) in [
        (d::supportsSystemPairing, true),
        (d::allowsPairing, true),
        (d::supportsACL, true),
        (d::supportsSharedQueue, false),
        (d::isGroupLeader, now.leader()),
        (d::isProxyGroupPlayer, false),
        (d::isClusterAware, true),
        (d::isAirplayActive, now.receiving()),
    ] {
        n.boolean(field, flag);
    }
    envelope(kind::DEVICE_INFO_MESSAGE, &n, reply)
}

/// То же описание при смене группы.
pub fn device_info_update(device: &Device, now: &NowPlaying) -> Node {
    use f::ProtocolMessage as p;
    let mut n = device_info(device, now, None);
    n.remove(p::r#type)
        .int(p::r#type, kind::DEVICE_INFO_UPDATE_MESSAGE.into());
    n
}

/// Карточка. `request` — `PLAYBACK_QUEUE_REQUEST`, на который отвечаем:
/// тогда без состояния и команд, но с обложкой, если о ней спросили.
pub fn state(
    device: &Device,
    now: &NowPlaying,
    request: Option<&Node>,
    reply: Option<&str>,
) -> Node {
    use f::SetStateMessage as s;
    let mut n = Node::default();
    n.node(s::playerPath, &player_path(device, now, false))
        .node(s::playbackQueueCapabilities, &Node::default());
    if request.is_none() {
        let playback = if now.playing {
            value::PlaybackState::Playing
        } else if now.title.is_empty() {
            value::PlaybackState::Unknown
        } else {
            value::PlaybackState::Paused
        };
        n.text(s::displayName, display_name(now, device))
            .int(s::playbackState, playback)
            .double(s::playbackStateTimestamp, cocoa_now());
        n.node(s::supportedCommands, &supported_commands(now));
    }

    let mut queue = Node::default();
    queue.int(f::PlaybackQueue::location, 0);
    let wanted = request.is_none_or(|r| {
        use f::PlaybackQueueRequestMessage as q;
        r.number(q::location).unwrap_or(0) == 0 && r.number(q::length) != Some(0)
    });
    if !now.title.is_empty() && wanted {
        queue.node(f::PlaybackQueue::contentItems, &content_item(now, request));
    }
    if let Some(request) = request {
        n.node(s::request, request);
        queue.node(
            f::PlaybackQueue::resolvedPlayerPath,
            &player_path(device, now, true),
        );
        if let Some(id) = request.string(f::PlaybackQueueRequestMessage::requestID) {
            queue.text(f::PlaybackQueue::requestId, id);
        }
    }
    n.node(s::playbackQueue, &queue);
    envelope(kind::SET_STATE_MESSAGE, &n, reply)
}

fn supported_commands(now: &NowPlaying) -> Node {
    use f::CommandInfo as c;
    use value::Command as cmd;
    let mut supported = Node::default();
    for command in [
        cmd::Play,
        cmd::Pause,
        cmd::TogglePlayPause,
        cmd::Stop,
        cmd::NextTrack,
        cmd::PreviousTrack,
    ] {
        // Переключение треков — только когда есть трек.
        let enabled = now.control_available && (command <= cmd::Stop || !now.title.is_empty());
        let mut info = Node::default();
        info.int(c::command, command).boolean(c::enabled, enabled);
        supported.node(f::SupportedCommands::supportedCommands, &info);
    }
    if now.can_seek && now.duration.is_some() {
        let mut seek = Node::default();
        seek.int(c::command, cmd::SeekToPlaybackPosition)
            .boolean(c::enabled, now.control_available)
            .int(c::canScrub, 1);
        supported.node(f::SupportedCommands::supportedCommands, &seek);
    }
    supported
}

fn content_item(now: &NowPlaying, request: Option<&Node>) -> Node {
    use f::ContentItemMetadata as m;
    let mut md = Node::default();
    md.text(m::title, &now.title)
        .text(m::trackArtistName, &now.artist)
        .text(m::albumName, &now.album)
        .double(m::elapsedTime, now.position.max(0.0))
        .float(m::playbackRate, if now.playing { 1.0 } else { 0.0 })
        .float(m::defaultPlaybackRate, 1.0)
        .double(m::elapsedTimeTimestamp, cocoa_now())
        .boolean(m::artworkAvailable, now.artwork.is_some());
    if let Some(duration) = now.duration.filter(|d| d.is_finite() && *d > 0.0) {
        md.double(m::duration, duration);
    }
    let mut item = Node::default();
    item.text(f::ContentItem::identifier, &now.content_id);
    let (mut width, mut height) = (0, 0);
    if let Some(art) = &now.artwork {
        (width, height) = (art.width, art.height);
        md.text(m::artworkMIMEType, &art.mime)
            .text(m::artworkIdentifier, &art.id);
        let asked = request.is_some_and(|r| {
            use f::PlaybackQueueRequestMessage as q;
            r.real(q::artworkWidth).is_some()
                || r.real(q::artworkHeight).is_some()
                || r.number(q::returnContentItemAssetsInUserCompletion) == Some(1)
        });
        if asked {
            item.bytes(f::ContentItem::artworkData, art.data.as_slice())
                .int(f::ContentItem::artworkDataWidth, width.into())
                .int(f::ContentItem::artworkDataHeight, height.into());
        }
    }
    md.bytes(m::nowPlayingInfoData, now_playing_info(width, height));
    item.node(f::ContentItem::metadata, &md);
    item
}

/// `nowPlayingInfoData`: NSKeyedArchiver со словарём размеров обложки и
/// места в очереди (без него iOS не показывает обложку).
pub fn now_playing_info(width: u32, height: u32) -> Vec<u8> {
    let entries = [
        ("kMRMediaRemoteNowPlayingInfoArtworkDataWidth", width),
        ("kMRMediaRemoteNowPlayingInfoTotalQueueCount", 1),
        ("kMRMediaRemoteNowPlayingInfoArtworkDataHeight", height),
        ("kMRMediaRemoteNowPlayingInfoQueueIndex", 0),
    ];
    let mut objects = vec![Value::String("$null".into()), Value::Boolean(false)];
    let mut keys = Vec::new();
    let mut values = Vec::new();
    for (key, _) in entries {
        keys.push(Value::Uid(Uid::new(objects.len() as u64)));
        objects.push(Value::String(key.into()));
    }
    for (_, n) in entries {
        values.push(Value::Uid(Uid::new(objects.len() as u64)));
        objects.push(Value::Integer(n.into()));
    }
    let mut class = Dictionary::new();
    class.insert("$classname".into(), "NSDictionary".into());
    class.insert(
        "$classes".into(),
        Value::Array(vec!["NSDictionary".into(), "NSObject".into()]),
    );
    let class_ref = objects.len() as u64;
    objects.push(class.into());
    let mut dict = Dictionary::new();
    dict.insert("NS.keys".into(), Value::Array(keys));
    dict.insert("NS.objects".into(), Value::Array(values));
    dict.insert("$class".into(), Value::Uid(Uid::new(class_ref)));
    objects[1] = dict.into();

    let mut top = Dictionary::new();
    top.insert("root".into(), Value::Uid(Uid::new(1)));
    let mut root = Dictionary::new();
    root.insert("$version".into(), Value::Integer(100_000.into()));
    root.insert("$archiver".into(), "NSKeyedArchiver".into());
    root.insert("$top".into(), top.into());
    root.insert("$objects".into(), Value::Array(objects));
    let mut out = Vec::new();
    Value::Dictionary(root)
        .to_writer_binary(&mut out)
        .expect("запись plist в память не может завершиться ошибкой");
    out
}

pub fn volume(device: &Device, now: &NowPlaying) -> Node {
    use f::VolumeDidChangeMessage as v;
    let mut n = Node::default();
    n.float(v::volume, now.volume)
        .text(v::endpointUID, now.group(device))
        .text(v::outputDeviceUID, &device.uid);
    envelope(kind::VOLUME_DID_CHANGE_MESSAGE, &n, None)
}

/// Новый источник: клиент, плеер и карточка.
pub fn announcement(device: &Device, now: &NowPlaying) -> Vec<Node> {
    let mut updated = Node::default();
    updated.node(f::UpdateClientMessage::client, &client(now, device));
    let mut active = Node::default();
    active.node(f::SetNowPlayingClientMessage::client, &client(now, device));
    let mut player = Node::default();
    player.node(
        f::SetNowPlayingPlayerMessage::playerPath,
        &player_path(device, now, false),
    );
    vec![
        envelope(kind::UPDATE_CLIENT_MESSAGE, &updated, None),
        envelope(kind::SET_NOW_PLAYING_CLIENT_MESSAGE, &active, None),
        state(device, now, None, None),
        envelope(kind::SET_NOW_PLAYING_PLAYER_MESSAGE, &player, None),
    ]
}

/// Приёмник как выход в списке устройств и его громкость.
pub fn topology(device: &Device, now: &NowPlaying) -> Vec<Node> {
    use f::AVOutputDeviceDescriptor as o;
    let mut out = Node::default();
    for (field, text) in [
        (o::name, device.name.as_str()),
        (o::uniqueIdentifier, &device.uid),
        (o::primaryUID, &device.uid),
        (o::groupID, now.group(device)),
        (o::airPlayGroupID, &device.group),
        (o::modelID, &device.model),
    ] {
        out.text(field, text);
    }
    for (field, number) in [
        (o::deviceType, value::DeviceType::AirPlay),
        (o::deviceSubType, value::DeviceSubType::HomePod),
        (o::hostDeviceClass, value::DeviceClass::Accessory),
        (o::clusterType, 0),
        (o::configuredClusterSize, 0),
        (o::transportType, 1),
    ] {
        out.int(field, number);
    }
    for (field, flag) in [
        (o::isGroupLeader, now.leader()),
        (o::groupContainsGroupLeader, now.contains_leader()),
        // Группы через MRP не собираются: их собирает отправитель.
        (o::isGroupable, false),
        (o::requiresAuthorization, true),
        (o::isRemoteControllable, true),
        (o::canRelayCommunicationChannel, false),
        (o::supportsBufferedAirPlay, false),
        (o::isVolumeControlAvailable, now.control_available),
        (o::pickable, true),
        (o::isAirPlayReceiverSessionActive, now.receiving()),
    ] {
        out.boolean(field, flag);
    }
    if now.receiving() {
        out.text(o::parentGroupIdentifier, now.group(device));
    }
    let mut update = Node::default();
    update.node(
        f::UpdateOutputDeviceMessage::clusterAwareOutputDevices,
        &out,
    );

    use f::VolumeControlAvailabilityMessage as a;
    let mut available = Node::default();
    available
        .boolean(a::volumeControlAvailable, now.control_available)
        .int(
            a::volumeCapabilities,
            if now.control_available {
                value::VolumeCapabilities::Absolute
            } else {
                value::VolumeCapabilities::None
            },
        );
    use f::VolumeControlCapabilitiesDidChangeMessage as c;
    let mut caps = Node::default();
    caps.text(c::endpointUID, now.group(device))
        .text(c::outputDeviceUID, &device.uid)
        .node(c::capabilities, &available);
    vec![
        envelope(kind::UPDATE_OUTPUT_DEVICE_MESSAGE, &update, None),
        envelope(kind::VOLUME_CONTROL_AVAILABILITY_MESSAGE, &available, None),
        envelope(
            kind::VOLUME_CONTROL_CAPABILITIES_DID_CHANGE_MESSAGE,
            &caps,
            None,
        ),
        volume(device, now),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn device() -> Device {
        Device {
            name: "Кухня".into(),
            model: "AudioAccessory5,1".into(),
            localized_model: "HomePod mini".into(),
            uid: "12345678-1234-1234-1234-123456789ABC".into(),
            group: "87654321-4321-4321-4321-CBA987654321".into(),
            system_app: "com.example.receiver".into(),
        }
    }

    fn playing() -> NowPlaying {
        NowPlaying {
            title: "Песня".into(),
            artist: "Исполнитель".into(),
            content_id: "1".into(),
            duration: Some(200.0),
            playing: true,
            control_available: true,
            can_seek: true,
            artwork: Some(Artwork {
                data: Arc::new(vec![0xff, 0xd8, 0xff]),
                id: "a1".into(),
                mime: "image/jpeg".into(),
                width: 600,
                height: 600,
            }),
            route: Some(Route {
                group: "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE".into(),
                active: true,
                leader: false,
                contains_leader: false,
            }),
            ..Default::default()
        }
    }

    /// Как `--self-test` у airplay_mrp: всё собранное сходится со схемой.
    #[test]
    fn messages_match_schema() {
        let d = device();
        for now in [NowPlaying::default(), playing()] {
            let mut all = topology(&d, &now);
            all.extend(announcement(&d, &now));
            all.push(device_info(&d, &now, Some("1")));
            all.push(device_info_update(&d, &now));
            let mut request = Node::default();
            request.double(f::PlaybackQueueRequestMessage::artworkWidth, 600.0);
            all.push(state(&d, &now, Some(&request), Some("2")));
            all.push(command_reply(Some("3"), value::SendError::NotSupported));
            all.push(error_reply(Some("4"), value::ErrorCode::UnknownError));
            for m in all {
                m.validate("ProtocolMessage").unwrap();
            }
        }
    }

    #[test]
    fn card_and_artwork() {
        let d = device();
        let now = playing();
        let card = state(&d, &now, None, None);
        let body = card.nested(f::ProtocolMessage::setStateMessage).unwrap();
        assert_eq!(
            body.number(f::SetStateMessage::playbackState),
            Some(value::PlaybackState::Playing)
        );
        let queue = body.nested(f::SetStateMessage::playbackQueue).unwrap();
        let item = queue.nested(f::PlaybackQueue::contentItems).unwrap();
        assert!(item.raw(f::ContentItem::artworkData).is_none());
        let md = item.nested(f::ContentItem::metadata).unwrap();
        assert_eq!(md.string(f::ContentItemMetadata::title), Some("Песня"));
        let commands = body.nested(f::SetStateMessage::supportedCommands).unwrap();
        assert_eq!(commands.0.len(), 7, "шесть кнопок и перемотка");

        // Ответ на запрос очереди с размером обложки — с картинкой.
        let mut request = Node::default();
        request
            .double(f::PlaybackQueueRequestMessage::artworkWidth, 600.0)
            .text(f::PlaybackQueueRequestMessage::requestID, "r1");
        let answer = state(&d, &now, Some(&request), Some("x"));
        assert_eq!(answer.string(f::ProtocolMessage::identifier), Some("x"));
        let body = answer.nested(f::ProtocolMessage::setStateMessage).unwrap();
        let queue = body.nested(f::SetStateMessage::playbackQueue).unwrap();
        assert_eq!(queue.string(f::PlaybackQueue::requestId), Some("r1"));
        let item = queue.nested(f::PlaybackQueue::contentItems).unwrap();
        assert_eq!(
            item.raw(f::ContentItem::artworkData),
            Some(&[0xff, 0xd8, 0xff][..])
        );
    }

    #[test]
    fn route_and_uids() {
        let d = device();
        let now = playing();
        assert_eq!(now.group(&d), "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE");
        assert_eq!(NowPlaying::default().group(&d), d.group);
        let info = device_info(&d, &now, None);
        let body = info.nested(f::ProtocolMessage::deviceInfoMessage).unwrap();
        assert_eq!(body.number(f::DeviceInfoMessage::isGroupLeader), Some(0));
        assert!(same_uid(
            "{12345678-1234-1234-1234-123456789abc}",
            "12345678-1234-1234-1234-123456789ABC"
        ));
        assert_eq!(stable_uuid("a"), stable_uuid("a"));
        assert_ne!(random_uuid(), random_uuid());
        let info = plist::Value::from_reader(std::io::Cursor::new(now_playing_info(600, 600)));
        assert!(info.unwrap().as_dictionary().is_some());
    }
}
