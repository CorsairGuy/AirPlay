//! Карточка «Сейчас играет» самого приёмника: служба `mediaremote` внутри
//! процесса, без отдельной программы.
//!
//! Трек, обложку, позицию и громкость приёмник и так получает от
//! отправителя (`Events`), команды карточки уходят отправителю через
//! [`Remote`]. Так iPhone, Mac и другие контроллеры в сети видят, что
//! играет, и управляют этим через MRP, как HomePod.

use std::sync::Arc;

pub use crate::mrp_service::model::Profile as CardProfile;
use mrp::messages::Device;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use crate::events::{Events, Metadata, Progress};
use crate::mrp_service::{
    control::Control,
    model::{Cover, Feed, Identity, Profile, Route},
    server,
};
use crate::remote::Remote;

/// Источник в терминах станции: у неё «airplay» — входящий AirPlay.
const SOURCE: &str = "airplay";

pub struct Card {
    identity: Identity,
    feed: watch::Sender<Feed>,
    remote: Remote,
    /// Программа, которая встраивает приёмник, получает события как обычно.
    inner: Arc<dyn Events>,
    outputs: Option<crate::output_control::OutputControl>,
    groups: Arc<crate::mrp_service::control::GroupControlState>,
    source: crate::continuous_control::SourceControl,
}

/// SHA-256 в hex, как `hash()` в sources.rs станции.
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl Card {
    /// Explicit host playback observations use the same Card/SourceControl
    /// machinery as receiver events. Keep identity, profile, output topology
    /// and routes under their existing owners. No queue-acceptance is invented.
    pub fn observe_host(&self, observed: &Feed) -> anyhow::Result<()> {
        anyhow::ensure!(
            !observed.source.is_empty()
                && observed.source.len() <= 256
                && observed.content_id.len() <= 1024
                && [&observed.title, &observed.artist, &observed.album]
                    .iter()
                    .all(|v| v.len() <= 4096)
                && observed.position.is_finite()
                && observed.position >= 0.0
                && observed.duration.is_none_or(|v| v.is_finite() && v >= 0.0)
                && observed.volume.is_finite()
                && (0.0..=1.0).contains(&observed.volume)
                && observed
                    .cover
                    .as_ref()
                    .is_none_or(|v| v.data.len() <= 2 * 1024 * 1024),
            "invalid host playback observation"
        );
        self.update(|feed| {
            let changed_source =
                feed.source != observed.source || feed.content_id != observed.content_id;
            feed.source.clone_from(&observed.source);
            feed.title.clone_from(&observed.title);
            feed.artist.clone_from(&observed.artist);
            feed.album.clone_from(&observed.album);
            feed.content_id.clone_from(&observed.content_id);
            // Repeated identical observations must not move the sample anchor.
            if changed_source
                || feed.position != observed.position
                || feed.playing != observed.playing
            {
                feed.position_sampled = std::time::Instant::now();
            }
            feed.position = observed.position;
            feed.duration = observed.duration;
            feed.playing = observed.playing;
            feed.volume = observed.volume;
            feed.control_available = observed.control_available;
            feed.cover.clone_from(&observed.cover);
        });
        Ok(())
    }
    /// Publish output-owner verified descriptors. Running audio alone does not
    /// prove receiver group adoption. Preserve reported group/cluster IDs; never
    /// manufacture membership from a requested selection. Advertise only volume
    /// capabilities the owner actually routes (remote volume routing is separate).
    pub fn set_outputs(&self, descriptors: Vec<mrp::wire::Node>) -> anyhow::Result<()> {
        use crate::mrp_service::{model::RemoteOutput, wire::Node};
        use mrp::schema::field::AVOutputDeviceDescriptor as fields;
        anyhow::ensure!(descriptors.len() <= 64, "too many output descriptors");
        let mut outputs = std::collections::BTreeMap::new();
        for mut descriptor in descriptors {
            descriptor.validate("AVOutputDeviceDescriptor")?;
            let uid = descriptor
                .string(fields::uniqueIdentifier)
                .ok_or_else(|| anyhow::anyhow!("missing output UID"))?
                .to_owned();
            anyhow::ensure!(
                !uid.is_empty() && uid.len() <= 256 && !uid.chars().any(char::is_control),
                "invalid output UID"
            );
            anyhow::ensure!(
                !crate::mrp_service::model::same_uid(&uid, &self.identity.device),
                "remote descriptor duplicates local output"
            );
            let volume = if descriptor.get(fields::volume).is_some() {
                let value = descriptor.real(fields::volume).unwrap_or(f64::NAN);
                anyhow::ensure!(
                    value.is_finite() && (0.0..=1.0).contains(&value),
                    "invalid output volume"
                );
                Some(value as f32)
            } else {
                None
            };
            descriptor.remove(fields::volume);
            let key = uuid::Uuid::parse_str(&uid)
                .map(|v| v.to_string())
                .unwrap_or(uid);
            let remote = RemoteOutput {
                descriptor: Node::parse(&descriptor.encode())?,
                volume,
            };
            anyhow::ensure!(
                outputs.insert(key, remote).is_none(),
                "duplicate output UID"
            );
        }
        self.update(|feed| feed.outputs = outputs.into_values().collect());
        Ok(())
    }
    pub fn new(device: Device, remote: Remote, inner: Arc<dyn Events>) -> Arc<Self> {
        Self::create(device, remote, inner, None, None)
    }

    pub fn new_with_outputs(
        device: Device,
        remote: Remote,
        inner: Arc<dyn Events>,
        outputs: crate::output_control::OutputControl,
    ) -> Arc<Self> {
        Self::create(device, remote, inner, Some(outputs), None)
    }

    /// Explicit presentation/namespace profile; device adapters can preserve
    /// previously deployed identifiers without device-specific core defaults.
    pub fn new_with_profile(
        device: Device,
        remote: Remote,
        inner: Arc<dyn Events>,
        outputs: Option<crate::output_control::OutputControl>,
        profile: Profile,
    ) -> anyhow::Result<Arc<Self>> {
        profile.validate()?;
        Ok(Self::create(device, remote, inner, outputs, Some(profile)))
    }

    fn create(
        device: Device,
        remote: Remote,
        inner: Arc<dyn Events>,
        outputs: Option<crate::output_control::OutputControl>,
        profile: Option<Profile>,
    ) -> Arc<Self> {
        // ВРЕМЕННЫЙ ОПЫТ: подмена данных карточки переменными окружения,
        // чтобы найти, чем она отличается от станции. Убрать после опыта.
        let debug = profile.is_none();
        let env = |k: &str| {
            if debug {
                std::env::var(k).ok().filter(|v| !v.is_empty())
            } else {
                None
            }
        };
        let mut profile = profile.unwrap_or_else(|| Profile::from_device(&device));
        profile.name = env("X_MRP_NAME").unwrap_or(profile.name);
        // Станция: пульт доступен, пока работает её управление, то есть всегда.
        let (feed, _) = watch::channel(Feed {
            profile,
            source: env("X_MRP_SOURCE").unwrap_or(SOURCE.into()),
            volume: env("X_MRP_VOLUME")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.2),
            control_available: true,
            ..Default::default()
        });
        Arc::new(Self {
            identity: Identity {
                device: env("X_MRP_DEVICE").unwrap_or(device.uid),
                group: env("X_MRP_GROUP").unwrap_or(device.group),
            },
            feed,
            remote,
            inner,
            outputs,
            groups: Default::default(),
            source: Default::default(),
        })
    }

    /// Описание устройства для MRP по описанию приёмника.
    pub fn device(info: &device::DeviceInfo) -> Device {
        let localized = match info.model.as_str() {
            "AudioAccessory5,1" => "HomePod mini",
            m if m.starts_with("AudioAccessory") => "HomePod",
            m if m.starts_with("AppleTV") => "Apple TV",
            _ => "AirPlay",
        };
        Device {
            name: info.name.clone(),
            model: info.model.clone(),
            localized_model: localized.into(),
            uid: info.pairing_id.clone(),
            group: info.group_id(),
            system_app: "com.github.corsairguy.airplay".into(),
        }
    }

    /// Одно соединение MRP (канал потока 130 уже расшифрован): сервер станции.
    pub async fn serve<S>(self: Arc<Self>, stream: S) -> std::io::Result<()>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let mut control = Control::new(self.feed.clone(), self.remote.clone())
            .with_continuous_source(self.source.clone());
        control.groups = self.groups.clone();
        if let Some(outputs) = &self.outputs {
            control = control.with_outputs(outputs.clone());
        }
        server::connection(
            stream,
            self.identity.clone(),
            self.feed.subscribe(),
            control,
        )
        .await
        .map_err(std::io::Error::other)
    }

    /// Группа сессии отправителя (`gid`), как маршрут станции.
    pub fn route(&self, group: Option<String>) {
        self.update(|feed| {
            feed.route = group.map(|group| Route {
                group,
                active: true,
                leader: true,
                contains_leader: true,
            })
        });
    }

    /// Opt-in continuous control receiver. The application also awaits
    /// follow_continuous_source with the SAME group stop watch.
    pub fn continuous_controls(&self) -> watch::Receiver<sender::continuous_source::State> {
        self.source.subscribe()
    }
    pub async fn follow_continuous_source(
        &self,
        stop: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        self.source.follow(self.feed.subscribe(), stop).await
    }
    fn update(&self, f: impl FnOnce(&mut Feed)) {
        self.feed.send_if_modified(|feed| {
            let before = feed.clone();
            f(feed);
            *feed != before
        });
    }
}

/// Громкость AirPlay (дБ, -30..0, -144 — без звука) в 0..1.
fn volume_unit(db: f32) -> f32 {
    if db <= -144.0 {
        0.0
    } else {
        ((db + 30.0) / 30.0).clamp(0.0, 1.0)
    }
}

impl Events for Card {
    fn volume(&self, db: f32) {
        self.update(|feed| feed.volume = volume_unit(db));
        self.inner.volume(db);
    }

    fn playing(&self, playing: bool) {
        self.update(|feed| feed.set_playing(playing));
        self.inner.playing(playing);
    }

    fn metadata(&self, m: &Metadata) {
        self.update(|feed| {
            // DMAP updates may omit fields. A duration-only update is not a
            // request to erase the current track. A different title starts a
            // new item, so its missing fields must not inherit the old item.
            if m.title.as_ref().is_some_and(|title| *title != feed.title) {
                feed.artist.clear();
                feed.album.clear();
                feed.duration = None;
                feed.cover = None;
                feed.position = 0.0;
                feed.position_sampled = std::time::Instant::now();
            }
            if let Some(title) = &m.title {
                feed.title.clone_from(title);
            }
            if let Some(artist) = &m.artist {
                feed.artist.clone_from(artist);
            }
            if let Some(album) = &m.album {
                feed.album.clone_from(album);
            }
            if let Some(d) = m.duration {
                feed.duration = Some(d.as_secs_f64());
            }
            feed.content_id = hash(
                format!(
                    "{}\0{}\0{}\0{}",
                    feed.source, feed.title, feed.artist, feed.album
                )
                .as_bytes(),
            );
        });
        self.inner.metadata(m);
    }

    fn artwork(&self, mime: &str, data: &[u8]) {
        // Как sources.rs станции: только картинка с известными размерами.
        let cover = image_info(data)
            .filter(|(_, w, h)| (1..=8192).contains(w) && (1..=8192).contains(h))
            .map(|(mime, width, height)| Cover {
                id: hash(data),
                data: Arc::new(data.to_vec()),
                mime: mime.into(),
                width,
                height,
            });
        self.update(|feed| feed.cover = cover);
        self.inner.artwork(mime, data);
    }

    fn progress(&self, p: Progress) {
        self.update(|feed| {
            feed.position = p.position.as_secs_f64();
            feed.position_sampled = std::time::Instant::now();
            if !p.duration.is_zero() {
                feed.duration = Some(p.duration.as_secs_f64());
            }
        });
        self.inner.progress(p);
    }
}

/// Тип и размер картинки по первым байтам: JPEG (маркер SOF) или PNG (IHDR).
fn image_info(data: &[u8]) -> Option<(&'static str, u32, u32)> {
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") && &data[12..16] == b"IHDR" {
        let w = u32::from_be_bytes(data[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(data[20..24].try_into().unwrap());
        return Some(("image/png", w, h));
    }
    if !data.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let mut at = 2;
    while at + 9 <= data.len() {
        if data[at] != 0xff {
            return None;
        }
        let marker = data[at + 1];
        if marker == 0xff {
            at += 1;
            continue;
        }
        let len = u16::from_be_bytes([data[at + 2], data[at + 3]]) as usize;
        // SOF0..SOF15, кроме DHT (C4), JPG (C8) и DAC (CC).
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            let h = u16::from_be_bytes([data[at + 5], data[at + 6]]);
            let w = u16::from_be_bytes([data[at + 7], data[at + 8]]);
            return Some(("image/jpeg", w.into(), h.into()));
        }
        if len < 2 {
            return None;
        }
        at += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_are_per_card_and_legacy_player_ids_can_be_explicitly_preserved() {
        let device = |name: &str| Device {
            name: name.into(),
            model: "GenericAudio".into(),
            localized_model: "Audio receiver".into(),
            uid: name.into(),
            group: format!("{name}-group"),
            system_app: "example.player".into(),
        };
        let a = Card::new(
            device("one"),
            Remote::new(),
            Arc::new(crate::events::NoEvents),
        );
        let b = Card::new(
            device("two"),
            Remote::new(),
            Arc::new(crate::events::NoEvents),
        );
        let info = |card: &Card| {
            crate::mrp_service::model::device_info(&card.identity, &card.feed.borrow(), None)
                .nested(crate::mrp_service::schema::extension(
                    crate::mrp_service::model::kind("DEVICE_INFO_MESSAGE"),
                ))
                .unwrap()
        };
        assert_eq!(
            info(&a).string(crate::mrp_service::model::f("DeviceInfoMessage", "name")),
            Some("one")
        );
        assert_eq!(
            info(&b).string(crate::mrp_service::model::f("DeviceInfoMessage", "name")),
            Some("two")
        );
        assert_eq!(
            info(&a).string(crate::mrp_service::model::f("DeviceInfoMessage", "modelID")),
            Some("GenericAudio")
        );
        let mut profile = Profile::from_device(&device("compat"));
        profile.client_prefix = "mini2".into();
        profile.player_namespace = "mini2-mrp".into();
        profile.system_app = "mini2.native".into();
        let compat = Card::new_with_profile(
            device("compat"),
            Remote::new(),
            Arc::new(crate::events::NoEvents),
            None,
            profile,
        )
        .unwrap();
        let feed = compat.feed.borrow();
        let path = crate::mrp_service::model::path(&compat.identity, &feed, false);
        let player = path
            .nested(crate::mrp_service::model::f("PlayerPath", "player"))
            .unwrap();
        let expected = uuid::Uuid::new_v5(
            &uuid::Uuid::NAMESPACE_URL,
            b"mini2-mrp:compat:mini2.airplay",
        )
        .to_string();
        assert_eq!(
            player.string(crate::mrp_service::model::f(
                "NowPlayingPlayer",
                "identifier"
            )),
            Some(expected.as_str())
        );
        let mut invalid = Profile::default();
        invalid.name.clear();
        assert!(
            Card::new_with_profile(
                device("bad"),
                Remote::new(),
                Arc::new(crate::events::NoEvents),
                None,
                invalid
            )
            .is_err()
        );
    }

    #[test]
    fn output_descriptors_update_atomically_and_volume_does_not_rebuild_topology() {
        let card = Card::new(
            Device {
                name: "test".into(),
                model: "AirPlay".into(),
                localized_model: "AirPlay".into(),
                uid: "local".into(),
                group: "group".into(),
                system_app: "test".into(),
            },
            Remote::new(),
            Arc::new(crate::events::NoEvents),
        );
        let remote = |volume: f32| {
            let mut n = mrp::wire::Node::default();
            n.text(1, "Remote")
                .text(2, "remote")
                .text(3, "reported-group")
                .float(24, volume);
            n
        };
        card.set_outputs(vec![remote(0.2)]).unwrap();
        let previous = card.feed.borrow().clone();
        card.set_outputs(vec![remote(0.3)]).unwrap();
        let current = card.feed.borrow().clone();
        let changes =
            crate::mrp_service::model::topology_updates(&card.identity, &previous, &current);
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].number(1),
            Some(crate::mrp_service::model::kind("VOLUME_DID_CHANGE_MESSAGE") as u64)
        );
        assert_eq!(
            current.outputs[0].descriptor.string(3),
            Some("reported-group")
        );
        assert!(card.set_outputs(vec![remote(0.4), remote(0.5)]).is_err());
        assert_eq!(*card.feed.borrow(), current);
        let mut invalid = remote(0.4);
        invalid.text(2, "local");
        assert!(card.set_outputs(vec![invalid]).is_err());
        assert_eq!(*card.feed.borrow(), current);
        card.set_outputs(vec![]).unwrap();
        let removed = crate::mrp_service::model::topology_updates(
            &card.identity,
            &current,
            &card.feed.borrow(),
        );
        assert_eq!(removed[0].number(1), Some(66));
        assert_eq!(removed[0].nested(70).unwrap().string(1), Some("remote"));
        for message in removed {
            mrp::wire::Node::parse(&message.encode())
                .unwrap()
                .validate("ProtocolMessage")
                .unwrap();
        }
    }

    #[test]
    fn image_sizes() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(600u32.to_be_bytes());
        png.extend(400u32.to_be_bytes());
        assert_eq!(image_info(&png), Some(("image/png", 600, 400)));

        // SOI, APP0 (длина 4), SOF0: точность, высота 300, ширина 500.
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0x01, 0x2c, 0x01, 0xf4, 3,
        ];
        assert_eq!(image_info(&jpeg), Some(("image/jpeg", 500, 300)));
        assert_eq!(image_info(&jpeg[..10]), None);
        assert_eq!(image_info(b"GIF89a"), None);
        assert_eq!(image_info(&[]), None);
    }

    #[test]
    fn volume_scale() {
        assert_eq!(volume_unit(-144.0), 0.0);
        assert_eq!(volume_unit(0.0), 1.0);
        assert_eq!(volume_unit(-15.0), 0.5);
    }

    #[test]
    fn events_fill_the_card() {
        let card = Card::new(
            Device {
                name: "a".into(),
                model: "AudioAccessory5,1".into(),
                localized_model: "HomePod mini".into(),
                uid: "u".into(),
                group: "g".into(),
                system_app: "x".into(),
            },
            Remote::new(),
            Arc::new(crate::events::NoEvents),
        );
        let rx = card.feed.subscribe();
        card.metadata(&Metadata {
            title: Some("Песня".into()),
            ..Default::default()
        });
        card.playing(true);
        card.progress(Progress {
            position: std::time::Duration::from_secs(3),
            duration: std::time::Duration::from_secs(10),
        });
        let now = rx.borrow().clone();
        assert_eq!(now.title, "Песня");
        assert!(now.playing && !now.content_id.is_empty());
        assert_eq!((now.position, now.duration), (3.0, Some(10.0)));
        assert!(
            now.control_available,
            "как на станции: пульт доступен и без отправителя"
        );
    }

    #[test]
    fn partial_metadata_preserves_track_and_pause_but_new_track_clears_old_assets() {
        let card = Card::new(
            Device {
                name: "test".into(),
                model: "AudioAccessory5,1".into(),
                localized_model: "HomePod".into(),
                uid: "partial".into(),
                group: "g".into(),
                system_app: "test".into(),
            },
            Remote::new(),
            Arc::new(crate::events::NoEvents),
        );
        card.metadata(&Metadata {
            title: Some("First".into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            ..Default::default()
        });
        card.progress(Progress {
            position: std::time::Duration::from_secs(25),
            duration: std::time::Duration::from_secs(180),
        });
        card.playing(false);
        let before = card.feed.borrow().clone();
        card.metadata(&Metadata {
            duration: Some(std::time::Duration::from_secs(181)),
            ..Default::default()
        });
        let partial = card.feed.borrow().clone();
        assert_eq!(partial.content_id, before.content_id);
        assert_eq!(
            (
                partial.title.as_str(),
                partial.artist.as_str(),
                partial.album.as_str()
            ),
            ("First", "Artist", "Album")
        );
        assert_eq!(partial.position, 25.0);
        assert!(!partial.playing);
        card.playing(true);
        card.metadata(&Metadata {
            title: Some("Next".into()),
            ..Default::default()
        });
        let next = card.feed.borrow().clone();
        assert_ne!(next.content_id, before.content_id);
        assert!(next.artist.is_empty() && next.album.is_empty() && next.cover.is_none());
        assert_eq!(
            (next.position, next.duration, next.playing),
            (0.0, None, true)
        );
    }
}
