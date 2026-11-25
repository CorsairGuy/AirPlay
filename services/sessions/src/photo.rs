//! Приём фотографий AirPlay: `PUT /photo` с JPEG/PNG в теле.
//!
//! Это отдельный профиль, не HLS: изображение проверяется, ограничивается по
//! размеру и отдаётся приложению через [`Sink`]. Кэш предзагрузки (`cacheOnly`
//! и `displayCached`) ограничен и по числу, и по суммарной памяти. Возможность
//! не объявляется в Bonjour: пока нет проверки с iPhone, `/info` не меняется.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Больше этого одно изображение не принимаем.
pub const MAX_PHOTO_BYTES: usize = 16 * 1024 * 1024;
/// Суммарный бюджет кэша предзагрузки.
pub const MAX_CACHE_BYTES: usize = 48 * 1024 * 1024;
/// Сколько изображений держим для перехода вперёд/назад.
pub const MAX_CACHED: usize = 4;
/// Предел стороны изображения.
const MAX_SIDE: u32 = 8192;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Photo {
    pub asset: String,
    pub mime: &'static str,
    pub transition: Option<String>,
    pub width: u32,
    pub height: u32,
    pub data: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PhotoError {
    TooLarge,
    MissingAssetKey,
    UnsupportedFormat,
    InvalidImage,
    UnknownAsset,
}

impl PhotoError {
    pub fn status(&self) -> u16 {
        match self {
            Self::TooLarge => 413,
            Self::UnknownAsset => 412,
            Self::MissingAssetKey | Self::UnsupportedFormat | Self::InvalidImage => 400,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            Self::TooLarge => "photo exceeds size limit",
            Self::MissingAssetKey => "missing X-Apple-AssetKey",
            Self::UnsupportedFormat => "only JPEG and PNG are accepted",
            Self::InvalidImage => "image header is invalid",
            Self::UnknownAsset => "cached photo is gone",
        }
    }
}

/// Куда приложение получает изображение. Вызывается синхронно, поэтому
/// реализация не должна блокировать надолго.
pub trait Sink: Send + Sync {
    fn display(&self, photo: &Photo);
    fn clear(&self);
}

struct Cache {
    entries: VecDeque<(String, Photo)>,
    bytes: usize,
}

impl Cache {
    fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
        }
    }

    fn remove(&mut self, asset: &str) -> Option<Photo> {
        let index = self.entries.iter().position(|(key, _)| key == asset)?;
        let (_, photo) = self.entries.remove(index)?;
        self.bytes = self.bytes.saturating_sub(photo.data.len());
        Some(photo)
    }

    fn insert(&mut self, photo: Photo) {
        self.remove(&photo.asset);
        while self.entries.len() >= MAX_CACHED || self.bytes + photo.data.len() > MAX_CACHE_BYTES {
            let Some((_, evicted)) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(evicted.data.len());
        }
        self.bytes += photo.data.len();
        self.entries.push_back((photo.asset.clone(), photo));
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

pub struct Service {
    sink: Arc<dyn Sink>,
    cache: Mutex<Cache>,
}

impl Service {
    pub fn new(sink: Arc<dyn Sink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            cache: Mutex::new(Cache::new()),
        })
    }

    /// `cacheOnly` — только в кэш, `displayCached` — взять из кэша,
    /// иначе проверить, сохранить и показать.
    pub fn put(
        &self,
        asset: &str,
        transition: Option<&str>,
        data: &[u8],
        cache_only: bool,
    ) -> Result<(), PhotoError> {
        let photo = self.decode(asset, transition, data)?;
        {
            let mut cache = self.lock();
            cache.insert(photo.clone());
        }
        if !cache_only {
            self.sink.display(&photo);
        }
        Ok(())
    }

    pub fn display_cached(&self, asset: &str, transition: Option<&str>) -> Result<(), PhotoError> {
        let mut photo = self.lock().remove(asset).ok_or(PhotoError::UnknownAsset)?;
        if transition.is_some() {
            photo.transition = transition.map(str::to_owned);
        }
        // Показанный кадр остаётся в кэше: следующий переход может вернуться.
        self.lock().insert(photo.clone());
        self.sink.display(&photo);
        Ok(())
    }

    pub fn remove(&self, asset: &str) -> bool {
        if self.lock().remove(asset).is_some() {
            self.sink.clear();
            true
        } else {
            false
        }
    }

    pub fn clear(&self) {
        let had = {
            let mut cache = self.lock();
            let had = !cache.entries.is_empty();
            cache.clear();
            had
        };
        let _ = had;
        self.sink.clear();
    }

    pub fn cached(&self) -> usize {
        self.lock().entries.len()
    }

    pub fn cached_bytes(&self) -> usize {
        self.lock().bytes
    }

    fn decode(
        &self,
        asset: &str,
        transition: Option<&str>,
        data: &[u8],
    ) -> Result<Photo, PhotoError> {
        let asset = asset.trim();
        if asset.is_empty() || asset.len() > 128 || asset.chars().any(char::is_control) {
            return Err(PhotoError::MissingAssetKey);
        }
        if data.is_empty() {
            return Err(PhotoError::InvalidImage);
        }
        if data.len() > MAX_PHOTO_BYTES {
            return Err(PhotoError::TooLarge);
        }
        let (mime, width, height) = image_info(data).ok_or(PhotoError::UnsupportedFormat)?;
        if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
            return Err(PhotoError::InvalidImage);
        }
        Ok(Photo {
            asset: asset.to_owned(),
            mime,
            transition: transition.map(str::to_owned),
            width,
            height,
            data: Arc::from(data),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// Тип и размеры по первым байтам: PNG IHDR или JPEG SOF.
pub fn image_info(data: &[u8]) -> Option<(&'static str, u32, u32)> {
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") && &data[12..16] == b"IHDR" {
        let width = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(data[20..24].try_into().ok()?);
        return Some(("image/png", width, height));
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
        let len = usize::from(u16::from_be_bytes([data[at + 2], data[at + 3]]));
        if len < 2 || at + 2 + len > data.len() {
            return None;
        }
        let is_sof = matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf);
        if is_sof && len >= 7 {
            let height = u32::from(u16::from_be_bytes([data[at + 5], data[at + 6]]));
            let width = u32::from(u16::from_be_bytes([data[at + 7], data[at + 8]]));
            return Some(("image/jpeg", width, height));
        }
        at += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);

    impl Sink for Log {
        fn display(&self, photo: &Photo) {
            self.0.lock().unwrap().push(format!(
                "display {} {}x{}",
                photo.asset, photo.width, photo.height
            ));
        }

        fn clear(&self) {
            self.0.lock().unwrap().push("clear".into());
        }
    }

    fn jpeg(width: u16, height: u16) -> Vec<u8> {
        vec![
            0xff,
            0xd8,
            0xff,
            0xe0,
            0,
            4,
            0,
            0,
            0xff,
            0xc0,
            0,
            11,
            8,
            (height >> 8) as u8,
            height as u8,
            (width >> 8) as u8,
            width as u8,
            3,
            1,
            1,
            0,
        ]
    }

    #[test]
    fn valid_jpeg_and_png_are_measured_and_delivered() {
        let log = Arc::new(Log::default());
        let service = Service::new(log.clone());
        service
            .put("asset-a", Some("Dissolve"), &jpeg(640, 480), false)
            .unwrap();
        assert_eq!(*log.0.lock().unwrap(), ["display asset-a 640x480"]);
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(320u32.to_be_bytes());
        png.extend(240u32.to_be_bytes());
        service.put("asset-b", None, &png, false).unwrap();
        assert_eq!(
            *log.0.lock().unwrap(),
            ["display asset-a 640x480", "display asset-b 320x240"]
        );
    }

    #[test]
    fn damaged_or_oversized_images_are_rejected_without_display() {
        let log = Arc::new(Log::default());
        let service = Service::new(log.clone());
        assert_eq!(
            service.put("bad", None, b"GIF89a", false),
            Err(PhotoError::UnsupportedFormat)
        );
        assert_eq!(
            service.put("truncated", None, &[0xff, 0xd8, 0xff], false),
            Err(PhotoError::UnsupportedFormat)
        );
        assert_eq!(
            service.put("huge", None, &vec![0; MAX_PHOTO_BYTES + 1], false),
            Err(PhotoError::TooLarge)
        );
        assert!(log.0.lock().unwrap().is_empty());
        assert_eq!(service.cached(), 0);
    }

    #[test]
    fn cache_only_then_display_cached_and_clear_on_session_end() {
        let log = Arc::new(Log::default());
        let service = Service::new(log.clone());
        let photo = jpeg(100, 100);
        service.put("next", None, &photo, true).unwrap();
        assert!(log.0.lock().unwrap().is_empty());
        service.display_cached("next", Some("SlideLeft")).unwrap();
        assert_eq!(*log.0.lock().unwrap(), ["display next 100x100"]);
        assert_eq!(
            service.display_cached("gone", None),
            Err(PhotoError::UnknownAsset)
        );
        service.clear();
        assert_eq!(service.cached(), 0);
        assert_eq!(*log.0.lock().unwrap(), ["display next 100x100", "clear"]);
    }

    #[test]
    fn cache_is_bounded_by_count_and_bytes() {
        let log = Arc::new(Log::default());
        let service = Service::new(log);
        for index in 0..(MAX_CACHED + 2) {
            service
                .put(&format!("a{index}"), None, &jpeg(10, 10), true)
                .unwrap();
        }
        assert_eq!(service.cached(), MAX_CACHED);
        let big = vec![0xff; MAX_CACHE_BYTES];
        // Больше бюджета одного файла: не принимаем как изображение, но и не
        // вытесняем кэш.
        assert!(service.put("big", None, &big, true).is_err());
        assert_eq!(service.cached(), MAX_CACHED);
        assert!(service.cached_bytes() <= MAX_CACHE_BYTES);
    }
}
