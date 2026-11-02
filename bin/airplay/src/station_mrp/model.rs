use crate::station_mrp::{schema, wire::Node};
use plist::{Dictionary, Uid, Value};
use std::{
    io::Cursor,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
/// Имя устройства. На станции это константа «Станция Мини»; здесь — имя приёмника.
pub static NAME_CELL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
fn device_name() -> &'static str {
    NAME_CELL.get().map(String::as_str).unwrap_or("AirPlay")
}
pub fn f(message: &str, name: &str) -> u32 {
    schema::field(message, name)
}
pub fn kind(name: &str) -> u32 {
    schema::kind(name)
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn cocoa() -> f64 {
    now() - 978307200.0
}
#[derive(Clone, Debug, PartialEq)]
pub struct Cover {
    pub data: Arc<Vec<u8>>,
    pub id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Feed {
    pub source: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub content_id: String,
    pub position: f64,
    pub position_sampled: Instant,
    pub duration: Option<f64>,
    pub playing: bool,
    pub volume: f32,
    pub control_available: bool,
    pub cover: Option<Cover>,
    pub route: Option<Route>,
}
impl Default for Feed {
    fn default() -> Self {
        Self {
            source: "native".into(),
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            content_id: String::new(),
            position: 0.0,
            position_sampled: Instant::now(),
            duration: None,
            playing: false,
            volume: 0.2,
            control_available: false,
            cover: None,
            route: None,
        }
    }
}
impl Feed {
    pub fn position_at(&self, now: Instant) -> f64 {
        let value = self.position
            + if self.playing {
                now.saturating_duration_since(self.position_sampled)
                    .as_secs_f64()
            } else {
                0.0
            };
        let value = value.max(0.0);
        self.duration
            .filter(|d| d.is_finite() && *d > 0.0)
            .map_or(value, |duration| value.min(duration))
    }
    pub fn set_playing(&mut self, playing: bool) {
        if self.playing != playing {
            let now = Instant::now();
            self.position = self.position_at(now);
            self.position_sampled = now;
            self.playing = playing;
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub group: String,
    pub active: bool,
    pub leader: bool,
    pub contains_leader: bool,
}
#[derive(Clone, Debug)]
pub struct Identity {
    pub device: String,
    pub group: String,
}
pub fn active_group<'a>(identity: &'a Identity, feed: &'a Feed) -> &'a str {
    feed.route
        .as_ref()
        .filter(|r| r.active)
        .map(|r| r.group.as_str())
        .unwrap_or(&identity.group)
}
pub fn receiver_active(feed: &Feed) -> bool {
    feed.route.as_ref().is_some_and(|r| r.active)
}
pub fn receiver_leader(feed: &Feed) -> bool {
    feed.route
        .as_ref()
        .filter(|r| r.active)
        .map(|r| r.leader)
        .unwrap_or(true)
}
pub fn contains_leader(feed: &Feed) -> bool {
    feed.route
        .as_ref()
        .filter(|r| r.active)
        .map(|r| r.contains_leader)
        .unwrap_or(true)
}
pub fn same_uid(a: &str, b: &str) -> bool {
    a == b || matches!((Uuid::parse_str(a),Uuid::parse_str(b)),(Ok(a),Ok(b)) if a==b)
}
pub fn dict(items: Vec<(&str, Value)>) -> Value {
    Value::Dictionary(
        items
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<Dictionary>(),
    )
}
pub fn binary(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    value.to_writer_binary(&mut out).expect("in-memory plist");
    out
}
pub fn queue_archive(width: u32, height: u32) -> Vec<u8> {
    let keys = [
        "kMRMediaRemoteNowPlayingInfoArtworkDataWidth",
        "kMRMediaRemoteNowPlayingInfoTotalQueueCount",
        "kMRMediaRemoteNowPlayingInfoArtworkDataHeight",
        "kMRMediaRemoteNowPlayingInfoQueueIndex",
    ];
    let mut objects = vec![
        Value::String("$null".into()),
        Value::Dictionary(Dictionary::new()),
    ];
    let key_refs: Vec<_> = keys
        .iter()
        .map(|key| {
            let at = objects.len();
            objects.push(Value::String((*key).into()));
            Value::Uid(Uid::new(at as u64))
        })
        .collect();
    let value_refs: Vec<_> = [width, 1, height, 0]
        .iter()
        .map(|n| {
            let at = objects.len();
            objects.push(Value::Integer((*n as u64).into()));
            Value::Uid(Uid::new(at as u64))
        })
        .collect();
    let class = objects.len();
    objects.push(dict(vec![
        ("$classname", Value::String("NSDictionary".into())),
        (
            "$classes",
            Value::Array(vec![
                Value::String("NSDictionary".into()),
                Value::String("NSObject".into()),
            ]),
        ),
    ]));
    objects[1] = dict(vec![
        ("NS.keys", Value::Array(key_refs)),
        ("NS.objects", Value::Array(value_refs)),
        ("$class", Value::Uid(Uid::new(class as u64))),
    ]);
    binary(&dict(vec![
        ("$version", Value::Integer(100000u64.into())),
        ("$archiver", Value::String("NSKeyedArchiver".into())),
        ("$top", dict(vec![("root", Value::Uid(Uid::new(1)))])),
        ("$objects", Value::Array(objects)),
    ]))
}
pub fn envelope(name: &str, body: Node, identifier: Option<&str>) -> Node {
    let kind = kind(name);
    let mut msg = Node::default();
    msg.int(f("ProtocolMessage", "type"), kind as u64)
        .int(f("ProtocolMessage", "errorCode"), 0)
        .text(
            f("ProtocolMessage", "uniqueIdentifier"),
            Uuid::new_v4().to_string().to_uppercase(),
        );
    if let Some(id) = identifier {
        msg.text(f("ProtocolMessage", "identifier"), id);
    }
    let tag = schema::extension(kind);
    if tag != 0 {
        msg.node(tag, &body);
    }
    msg
}
pub fn client(feed: &Feed) -> Node {
    let mut n = Node::default();
    n.int(f("NowPlayingClient", "processIdentifier"), 1)
        .int(f("NowPlayingClient", "processUserIdentifier"), 0)
        .text(
            f("NowPlayingClient", "bundleIdentifier"),
            format!("mini2.{}", feed.source),
        )
        .text(f("NowPlayingClient", "displayName"), device_name());
    n
}
pub fn path(identity: &Identity, feed: &Feed, local: bool) -> Node {
    let mut origin = Node::default();
    origin
        .int(f("Origin", "identifier"), 1280262988)
        .text(f("Origin", "displayName"), device_name());
    if local {
        origin.boolean(f("Origin", "isLocallyHosted"), true);
    }
    let mut player = Node::default();
    player
        .text(
            f("NowPlayingPlayer", "identifier"),
            Uuid::new_v5(
                &Uuid::NAMESPACE_URL,
                format!("mini2-mrp:{}:mini2.{}", identity.device, feed.source).as_bytes(),
            )
            .to_string(),
        )
        .text(f("NowPlayingPlayer", "displayName"), device_name())
        .int(f("NowPlayingPlayer", "audioSessionID"), 0);
    let mut n = Node::default();
    n.node(f("PlayerPath", "origin"), &origin)
        .node(f("PlayerPath", "client"), &client(feed))
        .node(f("PlayerPath", "player"), &player);
    n
}
pub fn device_info(identity: &Identity, feed: &Feed, reply: Option<&str>) -> Node {
    let m = "DeviceInfoMessage";
    let mut n = Node::default();
    for (key, value) in [
        ("deviceUID", identity.device.as_str()),
        ("uniqueIdentifier", identity.device.as_str()),
        ("name", device_name()),
        ("modelID", "AudioAccessory5,1"),
        ("localizedModelName", "HomePod mini"),
        ("applicationBundleIdentifier", "com.apple.mediaremoted"),
        ("groupUID", active_group(identity, feed)),
        ("airPlayGroupID", identity.group.as_str()),
        ("groupName", device_name()),
        ("systemMediaApplication", "mini2.native"),
        ("computerName", device_name()),
    ] {
        n.text(f(m, key), value);
    }
    for (key, value) in [
        ("protocolVersion", 1),
        ("lastSupportedMessageType", 139),
        ("deviceClass", 7),
        ("logicalDeviceCount", 1),
        ("clusterType", 0),
        ("configuredClusterSize", 0),
        (
            "groupContainsDiscoverableGroupLeader",
            if contains_leader(feed) { 1 } else { 0 },
        ),
    ] {
        n.int(f(m, key), value);
    }
    for (key, value) in [
        ("supportsSystemPairing", true),
        ("allowsPairing", true),
        ("supportsACL", true),
        ("supportsSharedQueue", false),
        ("isGroupLeader", receiver_leader(feed)),
        ("isProxyGroupPlayer", false),
        ("isClusterAware", true),
        ("isAirplayActive", receiver_active(feed)),
    ] {
        n.boolean(f(m, key), value);
    }
    envelope("DEVICE_INFO_MESSAGE", n, reply)
}
pub fn state(
    identity: &Identity,
    feed: &Feed,
    request: Option<&Node>,
    reply: Option<&str>,
) -> Node {
    let mut n = Node::default();
    let p = path(identity, feed, false);
    n.node(f("SetStateMessage", "playerPath"), &p).node(
        f("SetStateMessage", "playbackQueueCapabilities"),
        &Node::default(),
    );
    if request.is_none() {
        n.text(f("SetStateMessage", "displayName"), device_name())
            .int(
                f("SetStateMessage", "playbackState"),
                if feed.playing {
                    1
                } else if feed.title.is_empty() {
                    0
                } else {
                    2
                },
            )
            .double(f("SetStateMessage", "playbackStateTimestamp"), cocoa());
    }
    if request.is_none() {
        let mut supported = Node::default();
        for command in 1..=6 {
            let mut info = Node::default();
            info.int(f("CommandInfo", "command"), command).boolean(
                f("CommandInfo", "enabled"),
                feed.control_available && (command <= 4 || !feed.title.is_empty()),
            );
            supported.node(f("SupportedCommands", "supportedCommands"), &info);
        }
        if matches!(feed.source.as_str(), "native" | "airplay") && feed.duration.is_some() {
            let mut seek = Node::default();
            seek.int(f("CommandInfo", "command"), 45)
                .boolean(f("CommandInfo", "enabled"), feed.control_available)
                .int(f("CommandInfo", "canScrub"), 1);
            supported.node(f("SupportedCommands", "supportedCommands"), &seek);
        }
        n.node(f("SetStateMessage", "supportedCommands"), &supported);
    }
    let mut queue = Node::default();
    queue.int(f("PlaybackQueue", "location"), 0);
    let include = request
        .map(|r| {
            r.number(f("PlaybackQueueRequestMessage", "location"))
                .unwrap_or(0)
                == 0
                && r.number(f("PlaybackQueueRequestMessage", "length")) != Some(0)
        })
        .unwrap_or(true);
    if !feed.title.is_empty() && include {
        let m = "ContentItemMetadata";
        let mut md = Node::default();
        md.text(f(m, "title"), &feed.title)
            .text(f(m, "trackArtistName"), &feed.artist)
            .text(f(m, "albumName"), &feed.album)
            .double(f(m, "elapsedTime"), feed.position_at(Instant::now()))
            .float(f(m, "playbackRate"), if feed.playing { 1.0 } else { 0.0 })
            .float(f(m, "defaultPlaybackRate"), 1.0)
            .double(f(m, "elapsedTimeTimestamp"), cocoa())
            .boolean(f(m, "artworkAvailable"), feed.cover.is_some());
        if let Some(duration) = feed.duration.filter(|n| n.is_finite() && *n > 0.0) {
            md.double(f(m, "duration"), duration);
        }
        let (mut width, mut height) = (0, 0);
        let mut item = Node::default();
        item.text(f("ContentItem", "identifier"), &feed.content_id);
        if let Some(cover) = &feed.cover {
            width = cover.width;
            height = cover.height;
            md.text(f(m, "artworkMIMEType"), &cover.mime)
                .text(f(m, "artworkIdentifier"), &cover.id);
            if request
                .map(|r| {
                    r.real(f("PlaybackQueueRequestMessage", "artworkWidth"))
                        .is_some()
                        || r.real(f("PlaybackQueueRequestMessage", "artworkHeight"))
                            .is_some()
                        || r.number(f(
                            "PlaybackQueueRequestMessage",
                            "returnContentItemAssetsInUserCompletion",
                        )) == Some(1)
                })
                .unwrap_or(false)
            {
                item.bytes(f("ContentItem", "artworkData"), cover.data.as_slice())
                    .int(f("ContentItem", "artworkDataWidth"), width as u64)
                    .int(f("ContentItem", "artworkDataHeight"), height as u64);
            }
        }
        md.bytes(f(m, "nowPlayingInfoData"), queue_archive(width, height));
        item.node(f("ContentItem", "metadata"), &md);
        queue.node(f("PlaybackQueue", "contentItems"), &item);
    }
    if let Some(request) = request {
        n.node(f("SetStateMessage", "request"), request);
        queue.node(
            f("PlaybackQueue", "resolvedPlayerPath"),
            &path(identity, feed, true),
        );
        if let Some(id) = request.string(f("PlaybackQueueRequestMessage", "requestID")) {
            queue.text(f("PlaybackQueue", "requestId"), id);
        }
    }
    n.node(f("SetStateMessage", "playbackQueue"), &queue);
    envelope("SET_STATE_MESSAGE", n, reply)
}
pub fn volume(identity: &Identity, feed: &Feed) -> Node {
    let m = "VolumeDidChangeMessage";
    let mut n = Node::default();
    n.float(f(m, "volume"), feed.volume)
        .text(f(m, "endpointUID"), active_group(identity, feed))
        .text(f(m, "outputDeviceUID"), &identity.device);
    envelope("VOLUME_DID_CHANGE_MESSAGE", n, None)
}
pub fn device_info_update(identity: &Identity, feed: &Feed) -> Node {
    let mut n = device_info(identity, feed, None);
    n.0.retain(|(tag, _)| *tag != f("ProtocolMessage", "type"));
    n.int(
        f("ProtocolMessage", "type"),
        kind("DEVICE_INFO_UPDATE_MESSAGE") as u64,
    );
    n
}
pub fn announcement(identity: &Identity, feed: &Feed) -> Vec<Node> {
    let mut updated = Node::default();
    updated.node(f("UpdateClientMessage", "client"), &client(feed));
    let mut active = Node::default();
    active.node(f("SetNowPlayingClientMessage", "client"), &client(feed));
    let mut player = Node::default();
    player.node(
        f("SetNowPlayingPlayerMessage", "playerPath"),
        &path(identity, feed, false),
    );
    vec![
        envelope("UPDATE_CLIENT_MESSAGE", updated, None),
        envelope("SET_NOW_PLAYING_CLIENT_MESSAGE", active, None),
        state(identity, feed, None, None),
        envelope("SET_NOW_PLAYING_PLAYER_MESSAGE", player, None),
    ]
}
pub fn topology(identity: &Identity, feed: &Feed) -> Vec<Node> {
    let m = "AVOutputDeviceDescriptor";
    let mut device = Node::default();
    for (key, value) in [
        ("name", device_name()),
        ("uniqueIdentifier", identity.device.as_str()),
        ("primaryUID", identity.device.as_str()),
        ("groupID", active_group(identity, feed)),
        ("airPlayGroupID", identity.group.as_str()),
        ("modelID", "AudioAccessory5,1"),
    ] {
        device.text(f(m, key), value);
    }
    for (key, value) in [
        ("deviceType", 1),
        ("deviceSubType", 12),
        ("hostDeviceClass", 7),
        ("clusterType", 0),
        ("configuredClusterSize", 0),
        ("transportType", 1),
    ] {
        device.int(f(m, key), value);
    }
    for (key, value) in [
        ("isGroupLeader", receiver_leader(feed)),
        ("groupContainsGroupLeader", contains_leader(feed)),
        ("isGroupable", false),
        ("requiresAuthorization", true),
        ("isRemoteControllable", true),
        ("canRelayCommunicationChannel", false),
        ("supportsBufferedAirPlay", false),
        ("isVolumeControlAvailable", feed.control_available),
        ("pickable", true),
        ("isAirPlayReceiverSessionActive", receiver_active(feed)),
    ] {
        device.boolean(f(m, key), value);
    }
    if receiver_active(feed) {
        device.text(f(m, "parentGroupIdentifier"), active_group(identity, feed));
    }
    let mut output = Node::default();
    output.node(
        f("UpdateOutputDeviceMessage", "clusterAwareOutputDevices"),
        &device,
    );
    let mut capability = Node::default();
    capability
        .boolean(
            f("VolumeControlAvailabilityMessage", "volumeControlAvailable"),
            feed.control_available,
        )
        .int(
            f("VolumeControlAvailabilityMessage", "volumeCapabilities"),
            if feed.control_available { 2 } else { 0 },
        );
    let mut caps = Node::default();
    caps.text(
        f("VolumeControlCapabilitiesDidChangeMessage", "endpointUID"),
        active_group(identity, feed),
    )
    .text(
        f(
            "VolumeControlCapabilitiesDidChangeMessage",
            "outputDeviceUID",
        ),
        &identity.device,
    )
    .node(
        f("VolumeControlCapabilitiesDidChangeMessage", "capabilities"),
        &capability,
    );
    vec![
        envelope("UPDATE_OUTPUT_DEVICE_MESSAGE", output, None),
        envelope("VOLUME_CONTROL_AVAILABILITY_MESSAGE", capability, None),
        envelope("VOLUME_CONTROL_CAPABILITIES_DID_CHANGE_MESSAGE", caps, None),
        volume(identity, feed),
    ]
}
pub fn decode_plist(bytes: &[u8]) -> anyhow::Result<Value> {
    Ok(Value::from_reader(Cursor::new(bytes))?)
}

/// State/timeline-only patch for an already announced content item. Do not
/// invalidate artwork or re-announce the queue on ordinary Pause/Play updates.
pub fn playback_update(identity: &Identity, feed: &Feed) -> Node {
    let mut body = Node::default();
    body.node(
        f("SetStateMessage", "playerPath"),
        &path(identity, feed, false),
    )
    .int(
        f("SetStateMessage", "playbackState"),
        if feed.playing {
            1
        } else if feed.title.is_empty() {
            0
        } else {
            2
        },
    )
    .double(f("SetStateMessage", "playbackStateTimestamp"), cocoa());
    if !feed.content_id.is_empty() && !feed.title.is_empty() {
        let mut metadata = Node::default();
        metadata
            .double(
                f("ContentItemMetadata", "elapsedTime"),
                feed.position_at(Instant::now()),
            )
            .float(
                f("ContentItemMetadata", "playbackRate"),
                if feed.playing { 1.0 } else { 0.0 },
            )
            .double(f("ContentItemMetadata", "elapsedTimeTimestamp"), cocoa());
        let mut item = Node::default();
        item.text(f("ContentItem", "identifier"), &feed.content_id)
            .node(f("ContentItem", "metadata"), &metadata);
        let mut queue = Node::default();
        queue
            .int(f("PlaybackQueue", "location"), 0)
            .node(f("PlaybackQueue", "contentItems"), &item);
        body.node(f("SetStateMessage", "playbackQueue"), &queue);
    }
    envelope("SET_STATE_MESSAGE", body, None)
}

pub fn state_change(identity: &Identity, previous: &Feed, current: &Feed) -> Option<Node> {
    if current.content_id != previous.content_id
        || current.title != previous.title
        || current.artist != previous.artist
        || current.album != previous.album
        || current.cover != previous.cover
        || current.duration != previous.duration
        || current.control_available != previous.control_available
    {
        Some(state(identity, current, None, None))
    } else if current.playing != previous.playing
        || (current.position - previous.position).abs() >= 1.0
    {
        Some(playback_update(identity, current))
    } else {
        None
    }
}

#[cfg(test)]
mod playback_patch_tests {
    use super::*;
    #[test]
    fn pause_play_patches_preserve_artwork_but_late_join_requests_still_return_it() {
        let id = Identity {
            device: "device".into(),
            group: "group".into(),
        };
        let before = Feed {
            content_id: "track".into(),
            title: "Song".into(),
            playing: true,
            cover: Some(Cover {
                data: Arc::new(vec![1, 2, 3]),
                id: "cover".into(),
                mime: "image/png".into(),
                width: 2,
                height: 2,
            }),
            ..Default::default()
        };
        for playing in [false, true] {
            let mut current = before.clone();
            current.playing = playing;
            current.position = 12.0;
            let message = state_change(&id, &before, &current).unwrap();
            message.validate("ProtocolMessage").unwrap();
            let body = message
                .nested(schema::extension(kind("SET_STATE_MESSAGE")))
                .unwrap();
            let queue = body.nested(f("SetStateMessage", "playbackQueue")).unwrap();
            let item = queue.nested(f("PlaybackQueue", "contentItems")).unwrap();
            assert_eq!(item.string(f("ContentItem", "identifier")), Some("track"));
            assert!(item.get(f("ContentItem", "artworkData")).is_none());
            let metadata = item.nested(f("ContentItem", "metadata")).unwrap();
            assert_eq!(metadata.0.len(), 3);
            assert_eq!(
                metadata.real(f("ContentItemMetadata", "playbackRate")),
                Some(if playing { 1.0 } else { 0.0 })
            );
            for name in [
                "artworkAvailable",
                "artworkIdentifier",
                "nowPlayingInfoData",
                "title",
            ] {
                assert!(metadata.get(f("ContentItemMetadata", name)).is_none());
            }
        }
        let mut request = Node::default();
        request.float(f("PlaybackQueueRequestMessage", "artworkWidth"), 2.0);
        let full = state(&id, &before, Some(&request), Some("late-join"));
        let body = full
            .nested(schema::extension(kind("SET_STATE_MESSAGE")))
            .unwrap();
        let queue = body.nested(f("SetStateMessage", "playbackQueue")).unwrap();
        let item = queue.nested(f("PlaybackQueue", "contentItems")).unwrap();
        assert_eq!(
            item.raw(f("ContentItem", "artworkData")),
            Some([1, 2, 3].as_slice())
        );
    }
}
