//! D22: фоновые запросы источника ограничены, отмена не теряет permit,
//! а revision guard не даёт позднему результату сменить очередь.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use media::track_queue::{
    QueueError, Source, SourceError, Track, TrackId, TrackLoader, TrackQueue,
};

fn track(id: &str) -> Track {
    Track {
        id: TrackId::new(id).unwrap(),
        title: id.to_owned(),
        duration_ms: Some(1000),
    }
}

struct SlowSource {
    id: TrackId,
    delay: Duration,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

impl Source for SlowSource {
    fn resolve(&mut self, id: &TrackId) -> Result<Track, SourceError> {
        if id != &self.id {
            return Err(SourceError::NotFound);
        }
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(track(id.as_str()))
    }
}

struct SimpleSource;

impl Source for SimpleSource {
    fn resolve(&mut self, id: &TrackId) -> Result<Track, SourceError> {
        Ok(track(id.as_str()))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_resolves_are_capped_at_one_source_request() {
    let loader = Arc::new(TrackLoader::new(1).unwrap());
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for i in 0..4 {
        let id = TrackId::new(format!("t{i}")).unwrap();
        let source = SlowSource {
            id: id.clone(),
            delay: Duration::from_millis(80),
            active: active.clone(),
            max_active: max_active.clone(),
        };
        let loader = loader.clone();
        tasks.push(tokio::spawn(async move {
            loader.resolve(source, vec![id]).await.unwrap()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().len(), 1);
    }
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        1,
        "одновременно выполняется не больше одного запроса"
    );
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_the_caller_does_not_leak_or_bypass_the_permit() {
    let loader = Arc::new(TrackLoader::new(1).unwrap());
    let first_id = TrackId::new("slow").unwrap();
    let source = SlowSource {
        id: first_id.clone(),
        delay: Duration::from_millis(300),
        active: Arc::new(AtomicUsize::new(0)),
        max_active: Arc::new(AtomicUsize::new(0)),
    };
    let first = tokio::spawn({
        let loader = loader.clone();
        async move { loader.resolve(source, vec![first_id]).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    let second_id = TrackId::new("next").unwrap();
    let started = std::time::Instant::now();
    let result = loader
        .resolve(
            SlowSource {
                id: second_id.clone(),
                delay: Duration::from_millis(1),
                active: Arc::new(AtomicUsize::new(0)),
                max_active: Arc::new(AtomicUsize::new(0)),
            },
            vec![second_id],
        )
        .await
        .unwrap();
    assert_eq!(result[0].id.as_str(), "next");
    assert!(
        started.elapsed() >= Duration::from_millis(200),
        "permit держится, пока блокирующий источник не завершился"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_loader_result_cannot_replace_a_newer_selection() {
    let mut queue = TrackQueue::new();
    let loader = TrackLoader::new(1).unwrap();
    queue
        .load(
            &mut SimpleSource,
            &[TrackId::new("a").unwrap(), TrackId::new("b").unwrap()],
        )
        .unwrap();
    let guard = queue.begin_load();
    let late = loader
        .resolve(
            SlowSource {
                id: TrackId::new("late").unwrap(),
                delay: Duration::from_millis(100),
                active: Arc::new(AtomicUsize::new(0)),
                max_active: Arc::new(AtomicUsize::new(0)),
            },
            vec![TrackId::new("late").unwrap()],
        )
        .await
        .unwrap();
    let pending = queue.begin_select_item(queue.item_ids()[1]).unwrap();
    queue.commit_select(pending).unwrap();
    assert_eq!(queue.apply_load(guard, late), Err(QueueError::Cancelled));
    assert_eq!(queue.current().unwrap().id.as_str(), "b");
}
