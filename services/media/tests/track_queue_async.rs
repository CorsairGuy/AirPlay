//! Неблокирующий источник и отмена устаревших результатов (C63).
//!
//! Задержка имитируется tokio-таймером, каталог — синтетический. Проверяется,
//! что применение очереди разрешено ровно одному поколению загрузки, а
//! поздний ответ источника не перетирает более новый выбор пользователя.

use std::collections::HashMap;
use std::time::Duration;

use media::track_queue::{QueueError, SourceError, Track, TrackId, TrackQueue};

fn track(id: &str, title: &str, duration_ms: u64) -> Track {
    Track {
        id: TrackId::new(id).unwrap(),
        title: title.to_owned(),
        duration_ms: Some(duration_ms),
    }
}

async fn slow_resolve(track: Track, delay: Duration) -> Vec<Track> {
    tokio::time::sleep(delay).await;
    vec![track]
}

#[tokio::test]
async fn a_late_result_loses_to_a_selection_made_while_it_was_loading() {
    let mut queue = TrackQueue::new();
    let ids = [TrackId::new("a").unwrap(), TrackId::new("b").unwrap()];
    let mut catalog = HashMap::new();
    catalog.insert(ids[0].clone(), track("a", "A", 1000));
    catalog.insert(ids[1].clone(), track("b", "B", 1000));
    let mut source = Catalog(catalog);
    queue.load(&mut source, &ids).unwrap();

    let guard = queue.begin_load();
    let task = tokio::spawn(slow_resolve(
        track("c", "C", 500),
        Duration::from_millis(30),
    ));

    // Пользователь успевает переключиться, пока источник «думает».
    let second = queue.item_ids()[1];
    let pending = queue.begin_select_item(second).unwrap();
    queue.commit_select(pending).unwrap();

    let loaded = task.await.unwrap();
    assert_eq!(queue.apply_load(guard, loaded), Err(QueueError::Cancelled));
    assert_eq!(queue.current_item(), Some(second));
    assert_eq!(queue.len(), 2, "поздний ответ не меняет очередь");
}

#[tokio::test]
async fn only_the_newest_of_two_loads_is_applied() {
    let mut queue = TrackQueue::new();
    let older = queue.begin_load();
    let newer = queue.begin_load();

    let slow = tokio::spawn(slow_resolve(
        track("old", "Old", 10),
        Duration::from_millis(40),
    ));
    let fast = tokio::spawn(slow_resolve(
        track("new", "New", 10),
        Duration::from_millis(5),
    ));

    let fresh = fast.await.unwrap();
    queue.apply_load(newer, fresh).unwrap();
    assert_eq!(queue.current().unwrap().id.as_str(), "new");

    let stale = slow.await.unwrap();
    assert_eq!(queue.apply_load(older, stale), Err(QueueError::Cancelled));
    assert_eq!(queue.len(), 1);
    assert_eq!(queue.current().unwrap().title, "New");
}

#[tokio::test]
async fn an_aborted_source_leaves_the_queue_untouched() {
    let mut queue = TrackQueue::new();
    let mut source = Catalog(HashMap::from([(
        TrackId::new("a").unwrap(),
        track("a", "A", 1000),
    )]));
    queue
        .load(&mut source, &[TrackId::new("a").unwrap()])
        .unwrap();

    let guard = queue.begin_load();
    let task = tokio::spawn(slow_resolve(track("b", "B", 10), Duration::from_secs(30)));
    task.abort();
    assert!(task.await.is_err(), "отменённая задача не отдаёт результат");
    // Очередь не тронута, а прежняя загрузка остаётся актуальной до явной отмены.
    assert_eq!(queue.len(), 1);
    queue.cancel();
    let tracks = vec![track("b", "B", 10)];
    assert_eq!(queue.apply_load(guard, tracks), Err(QueueError::Cancelled));
}

#[tokio::test]
async fn the_source_error_is_reported_without_touching_the_queue() {
    let mut queue = TrackQueue::new();
    let mut source = Catalog(HashMap::new());
    assert_eq!(
        queue.load(&mut source, &[TrackId::new("missing").unwrap()]),
        Err(QueueError::Source(SourceError::NotFound))
    );
    assert!(queue.is_empty());
}

struct Catalog(HashMap<TrackId, Track>);

impl media::track_queue::Source for Catalog {
    fn resolve(&mut self, id: &TrackId) -> Result<Track, SourceError> {
        self.0.get(id).cloned().ok_or(SourceError::NotFound)
    }
}
