//! Ограничение памяти кэша фотографий и отсутствие устаревшего кадра (C79).
//!
//! Синтетические PNG-заголовки, никаких пользовательских файлов. `Sink`
//! записывает вызовы, поэтому видно, что именно показано и когда очищено.

use std::sync::{Arc, Mutex};

use sessions::photo::{
    MAX_CACHE_BYTES, MAX_CACHED, MAX_PHOTO_BYTES, Photo, PhotoError, Service, Sink,
};

#[derive(Default)]
struct Log(Mutex<Vec<String>>);

impl Sink for Log {
    fn display(&self, photo: &Photo) {
        self.0
            .lock()
            .unwrap()
            .push(format!("display {} {}", photo.asset, photo.data.len()));
    }
    fn clear(&self) {
        self.0.lock().unwrap().push("clear".into());
    }
}

impl Log {
    fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// Минимальный PNG: заголовок и IHDR достаточны для `image_info`.
fn png(width: u32, height: u32, padding: usize) -> Vec<u8> {
    let mut body = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    body.extend(width.to_be_bytes());
    body.extend(height.to_be_bytes());
    body.resize(body.len() + padding, 0);
    body
}

#[test]
fn the_cache_keeps_only_the_newest_images() {
    let log = Arc::new(Log::default());
    let service = Service::new(log.clone());
    for i in 0..MAX_CACHED + 2 {
        service
            .put(&format!("asset-{i}"), None, &png(64, 64, 0), true)
            .unwrap();
    }
    assert_eq!(service.cached(), MAX_CACHED, "число записей ограничено");
    assert!(
        service.display_cached("asset-0", None).is_err(),
        "самый старый вытеснен"
    );
    assert!(
        service
            .display_cached(&format!("asset-{}", MAX_CACHED + 1), None)
            .is_ok()
    );
    assert_eq!(log.calls().len(), 1, "показан только запрошенный кадр");
}

#[test]
fn the_total_cache_budget_is_enforced() {
    let log = Arc::new(Log::default());
    let service = Service::new(log);
    // Картинки по четверти бюджета: пятая обязана вытеснить самую старую.
    let chunk = MAX_CACHE_BYTES / 4;
    assert!(chunk < MAX_PHOTO_BYTES, "каждая картинка проходит лимит");
    for i in 0..5 {
        service
            .put(&format!("big-{i}"), None, &png(64, 64, chunk), true)
            .unwrap();
    }
    assert!(
        service.cached_bytes() <= MAX_CACHE_BYTES,
        "кэш {} байт превышает бюджет",
        service.cached_bytes()
    );
    assert!(service.cached() <= 4, "кэш {} записей", service.cached());
    assert!(
        service.cached() >= 1,
        "вытеснение не должно выбрасывать всё"
    );
}

#[test]
fn one_oversized_photo_is_rejected_without_touching_the_cache() {
    let log = Arc::new(Log::default());
    let service = Service::new(log);
    service.put("ok", None, &png(64, 64, 0), true).unwrap();
    let before = service.cached_bytes();
    let error = service
        .put("huge", None, &png(64, 64, MAX_PHOTO_BYTES), true)
        .unwrap_err();
    assert_eq!(error, PhotoError::TooLarge);
    assert_eq!(error.status(), 413);
    assert_eq!(service.cached(), 1);
    assert_eq!(service.cached_bytes(), before);
}

#[test]
fn replacing_an_asset_shows_the_new_image_only() {
    let log = Arc::new(Log::default());
    let service = Service::new(log.clone());
    service.put("same", None, &png(10, 10, 0), false).unwrap();
    service.put("same", None, &png(20, 20, 0), false).unwrap();
    let calls = log.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].contains("display same"));
    // Старое изображение не возвращается: в кэше одно вхождение.
    assert_eq!(service.cached(), 1);
}

#[test]
fn removing_the_displayed_photo_clears_the_screen() {
    let log = Arc::new(Log::default());
    let service = Service::new(log.clone());
    service.put("shown", None, &png(32, 32, 0), false).unwrap();
    assert!(service.remove("shown"));
    assert_eq!(log.calls().last().unwrap(), "clear");
    assert!(!service.remove("shown"), "повторное удаление — false");
    assert!(service.display_cached("shown", None).is_err());
}

#[test]
fn an_unknown_asset_does_not_show_anything() {
    let log = Arc::new(Log::default());
    let service = Service::new(log.clone());
    assert_eq!(
        service.display_cached("missing", None),
        Err(PhotoError::UnknownAsset)
    );
    assert!(log.calls().is_empty());
}

#[test]
fn teardown_clears_the_cache_and_the_sink_once() {
    let log = Arc::new(Log::default());
    let service = Service::new(log.clone());
    service.put("a", None, &png(16, 16, 0), true).unwrap();
    service.put("b", None, &png(16, 16, 0), true).unwrap();
    service.clear();
    assert_eq!(service.cached(), 0);
    assert_eq!(service.cached_bytes(), 0);
    assert_eq!(log.calls(), ["clear"]);
    service.clear();
    assert_eq!(
        log.calls(),
        ["clear", "clear"],
        "повторный teardown безвреден"
    );
}
