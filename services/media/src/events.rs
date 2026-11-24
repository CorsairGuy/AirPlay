//! Что приёмник сообщает наружу: громкость с iPhone, игра и пауза,
//! метаданные трека, обложка, позиция. Программа, которая встраивает
//! приёмник, реализует [`Events`] (MPRIS, карточка MRP, свой UI) и отдаёт
//! его в `serve::Options::events`.
//!
//! Метаданные AirPlay 2 приходят как у AirPlay 1: SET_PARAMETER с DMAP
//! (`application/x-dmap-tagged`), обложка — SET_PARAMETER с картинкой,
//! позиция — `progress: начало/сейчас/конец` в метках RTP.

use std::time::Duration;

/// Частота меток в `progress`: у AirPlay 2 всегда 44,1 кГц, как и у
/// shairport-sync.
const PROGRESS_RATE: u64 = 44_100;

/// Все методы по умолчанию ничего не делают. Вызовы приходят из задач
/// RTSP: долгую работу уносите в свой поток.
pub trait Events: Send + Sync {
    /// Громкость с iPhone, дБ AirPlay: от -30 до 0, -144 — без звука.
    fn volume(&self, _db: f32) {}
    /// Отправитель начал или остановил воспроизведение.
    fn playing(&self, _playing: bool) {}
    fn metadata(&self, _metadata: &Metadata) {}
    /// Обложка: байты картинки и тип из `Content-Type` (часто неточный:
    /// JPEG или PNG лучше определять по первым байтам).
    fn artwork(&self, _mime: &str, _data: &[u8]) {}
    fn progress(&self, _progress: Progress) {}
}

/// Ничего не сообщает.
pub struct NoEvents;

impl Events for NoEvents {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub position: Duration,
    pub duration: Duration,
}

impl Progress {
    /// `начало/сейчас/конец` в метках RTP.
    pub fn parse(text: &str) -> Option<Progress> {
        let mut it = text.trim().split('/').map(|v| v.trim().parse::<u32>());
        let (start, now, end) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
        let frames = |a: u32, b: u32| {
            Duration::from_micros(b.wrapping_sub(a) as u64 * 1_000_000 / PROGRESS_RATE)
        };
        Some(Progress {
            position: frames(start, now),
            duration: frames(start, end),
        })
    }
}

impl Metadata {
    /// DMAP: элементы `тег(4) длина(4, BE) данные`, трек — внутри `mlit`.
    pub fn from_dmap(data: &[u8]) -> Metadata {
        let mut m = Metadata::default();
        m.read(data, 0);
        m
    }

    fn read(&mut self, mut data: &[u8], depth: u8) {
        while data.len() >= 8 {
            let tag = &data[..4];
            let len = u32::from_be_bytes(data[4..8].try_into().unwrap()) as usize;
            let Some(value) = data.get(8..8 + len) else {
                return;
            };
            let text =
                || Some(String::from_utf8_lossy(value).into_owned()).filter(|s| !s.is_empty());
            match tag {
                b"mlit" if depth < 4 => self.read(value, depth + 1),
                b"minm" => self.title = text(),
                b"asar" => self.artist = text(),
                b"asal" => self.album = text(),
                b"asgn" => self.genre = text(),
                b"astm" if len == 4 => {
                    let ms = u32::from_be_bytes(value.try_into().unwrap());
                    self.duration = (ms > 0).then(|| Duration::from_millis(ms.into()));
                }
                _ => {}
            }
            data = &data[8 + len..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(tag: &[u8; 4], value: &[u8]) -> Vec<u8> {
        let mut out = tag.to_vec();
        out.extend_from_slice(&(value.len() as u32).to_be_bytes());
        out.extend_from_slice(value);
        out
    }

    #[test]
    fn dmap_track() {
        let mut track = item(b"minm", "Песня".as_bytes());
        track.extend(item(b"asar", b"Artist"));
        track.extend(item(b"asal", b"Album"));
        track.extend(item(b"asgn", b""));
        track.extend(item(b"astm", &215_000u32.to_be_bytes()));
        track.extend(item(b"mper", &[1; 8]));
        let m = Metadata::from_dmap(&item(b"mlit", &track));
        assert_eq!(m.title.as_deref(), Some("Песня"));
        assert_eq!(m.artist.as_deref(), Some("Artist"));
        assert_eq!(m.album.as_deref(), Some("Album"));
        assert_eq!(m.genre, None);
        assert_eq!(m.duration, Some(Duration::from_secs(215)));

        // Обрезанный элемент — не паника.
        let cut = item(b"mlit", &track);
        let _ = Metadata::from_dmap(&cut[..cut.len() - 3]);
    }

    #[test]
    fn progress_from_rtp() {
        let p = Progress::parse("1000/45100/442000\r\n").unwrap();
        assert_eq!(p.position, Duration::from_secs(1));
        assert_eq!(p.duration, Duration::from_secs(10));
        // Через переполнение меток.
        let p = Progress::parse(&format!("{}/43100/100", u32::MAX - 999)).unwrap();
        assert_eq!(p.position, Duration::from_secs(1));
        assert!(Progress::parse("1/2").is_none());
    }
}
