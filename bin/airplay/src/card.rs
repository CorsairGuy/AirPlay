//! Карточка «Сейчас играет» самого приёмника: служба `mediaremote` внутри
//! процесса, без отдельной программы.
//!
//! Трек, обложку, позицию и громкость приёмник и так получает от
//! отправителя (`Events`), команды карточки уходят отправителю через
//! [`Remote`]. Так iPhone, Mac и другие контроллеры в сети видят, что
//! играет, и управляют этим через MRP, как HomePod.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mediaremote::{Artwork, Command as CardCommand, Control, Reply};
use mrp::messages::{Device, NowPlaying};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use crate::events::{Events, Metadata, Progress};
use crate::remote::{Command, Remote};

pub struct Card {
    device: Device,
    now: watch::Sender<NowPlaying>,
    remote: Remote,
    /// Программа, которая встраивает приёмник, получает события как обычно.
    inner: Arc<dyn Events>,
    artwork: AtomicU64,
}

impl Card {
    pub fn new(device: Device, remote: Remote, inner: Arc<dyn Events>) -> Arc<Self> {
        let (now, _) = watch::channel(NowPlaying {
            can_seek: true,
            ..Default::default()
        });
        Arc::new(Self {
            device,
            now,
            remote,
            inner,
            artwork: AtomicU64::new(0),
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
            group: info.pairing_id.clone(),
            system_app: "com.github.corsairguy.airplay".into(),
        }
    }

    /// Одно соединение MRP (канал потока 130 уже расшифрован).
    pub async fn serve<S>(self: Arc<Self>, stream: S) -> std::io::Result<()>
    where
        S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let control = Arc::new(CardControl(self.clone()));
        mediaremote::serve(stream, self.device.clone(), self.now.subscribe(), control).await
    }

    fn update(&self, f: impl FnOnce(&mut NowPlaying)) {
        let connected = self.remote.connected();
        self.now.send_if_modified(|now| {
            let before = now.clone();
            f(now);
            now.control_available = connected;
            *now != before
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
        self.update(|now| now.volume = volume_unit(db));
        self.inner.volume(db);
    }

    fn playing(&self, playing: bool) {
        self.update(|now| now.playing = playing);
        self.inner.playing(playing);
    }

    fn metadata(&self, m: &Metadata) {
        self.update(|now| {
            now.title = m.title.clone().unwrap_or_default();
            now.artist = m.artist.clone().unwrap_or_default();
            now.album = m.album.clone().unwrap_or_default();
            if let Some(d) = m.duration {
                now.duration = Some(d.as_secs_f64());
            }
            let id = Sha256::digest(format!("{}\n{}\n{}", now.title, now.artist, now.album));
            now.content_id = hex::encode(&id[..8]);
        });
        self.inner.metadata(m);
    }

    fn artwork(&self, mime: &str, data: &[u8]) {
        if let Some((mime, width, height)) = image_info(data).or_else(|| {
            // Неизвестный формат: доверяем Content-Type, размер неизвестен.
            mime.starts_with("image/")
                .then(|| (mime_static(mime), 0, 0))
        }) {
            let n = self.artwork.fetch_add(1, Ordering::Relaxed) + 1;
            let art = Artwork {
                data: Arc::new(data.to_vec()),
                id: format!("art-{n}"),
                mime: mime.into(),
                width,
                height,
            };
            self.update(|now| now.artwork = Some(art));
        } else {
            // Пустая картинка — отправитель убрал обложку.
            self.update(|now| now.artwork = None);
        }
        self.inner.artwork(mime, data);
    }

    fn progress(&self, p: Progress) {
        self.update(|now| {
            now.position = p.position.as_secs_f64();
            if !p.duration.is_zero() {
                now.duration = Some(p.duration.as_secs_f64());
            }
        });
        self.inner.progress(p);
    }
}

fn mime_static(mime: &str) -> &'static str {
    match mime {
        "image/png" => "image/png",
        _ => "image/jpeg",
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

struct CardControl(Arc<Card>);

impl Control for CardControl {
    fn command(&self, command: CardCommand) -> Reply<'_> {
        let command = match command {
            CardCommand::Play => Command::Play,
            CardCommand::Pause => Command::Pause,
            CardCommand::PlayPause => Command::PlayPause,
            CardCommand::Stop => Command::Stop,
            CardCommand::Next => Command::Next,
            CardCommand::Previous => Command::Previous,
            CardCommand::Seek(secs) => Command::Seek(secs),
        };
        let sent = self.0.remote.send(command);
        Box::pin(async move { sent })
    }

    fn volume(&self, volume: f32) -> Reply<'_> {
        let sent = self.0.remote.send(Command::Volume(volume));
        if sent {
            self.0.update(|now| now.volume = volume);
        }
        Box::pin(async move { sent })
    }
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
        let rx = card.now.subscribe();
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
        assert!(!now.control_available, "отправителя нет");
    }
}
