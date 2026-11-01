//! Карточка «Сейчас играет» самого приёмника: служба `mediaremote` внутри
//! процесса, без отдельной программы.
//!
//! Трек, обложку, позицию и громкость приёмник и так получает от
//! отправителя (`Events`), команды карточки уходят отправителю через
//! [`Remote`]. Так iPhone, Mac и другие контроллеры в сети видят, что
//! играет, и управляют этим через MRP, как HomePod.

use std::sync::Arc;

use mrp::messages::Device;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use crate::events::{Events, Metadata, Progress};
use crate::remote::Remote;
use crate::station_mrp::{
    control::Control,
    model::{Cover, Feed, Identity, NAME_CELL, Route},
    server,
};

/// Источник в терминах станции: у неё «airplay» — входящий AirPlay.
const SOURCE: &str = "airplay";

pub struct Card {
    identity: Identity,
    feed: watch::Sender<Feed>,
    remote: Remote,
    /// Программа, которая встраивает приёмник, получает события как обычно.
    inner: Arc<dyn Events>,
}

/// SHA-256 в hex, как `hash()` в sources.rs станции.
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

impl Card {
    pub fn new(device: Device, remote: Remote, inner: Arc<dyn Events>) -> Arc<Self> {
        // ВРЕМЕННЫЙ ОПЫТ: подмена данных карточки переменными окружения,
        // чтобы найти, чем она отличается от станции. Убрать после опыта.
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let _ = NAME_CELL.set(env("X_MRP_NAME").unwrap_or(device.name.clone()));
        // Станция: пульт доступен, пока работает её управление, то есть всегда.
        let (feed, _) = watch::channel(Feed {
            source: env("X_MRP_SOURCE").unwrap_or(SOURCE.into()),
            volume: env("X_MRP_VOLUME").and_then(|v| v.parse().ok()).unwrap_or(0.2),
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
        let control = Control::new(self.feed.clone(), self.remote.clone());
        server::connection(stream, self.identity.clone(), self.feed.subscribe(), control)
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
        self.update(|feed| feed.playing = playing);
        self.inner.playing(playing);
    }

    fn metadata(&self, m: &Metadata) {
        self.update(|feed| {
            feed.title = m.title.clone().unwrap_or_default();
            feed.artist = m.artist.clone().unwrap_or_default();
            feed.album = m.album.clone().unwrap_or_default();
            if let Some(d) = m.duration {
                feed.duration = Some(d.as_secs_f64());
            }
            feed.content_id = hash(
                format!("{}\0{}\0{}\0{}", feed.source, feed.title, feed.artist, feed.album)
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
        assert!(now.control_available, "как на станции: пульт доступен и без отправителя");
    }
}
