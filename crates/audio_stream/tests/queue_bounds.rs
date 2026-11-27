//! Ограничение очереди и управление задержкой приёма аудио (C48).
//!
//! Тесты работают через публичный `Playout` с приёмником `null` и без якоря:
//! проверяются предел буфера, отбрасывание дублей и прекращение приёма после
//! `stop`. Реальное прослушивание и число сгенерированных пакетов сюда не
//! входят; `src/playout.rs` в рамках задачи не менялся.

use std::sync::Arc;

use audio_stream::{Config, Playout};
use clock::TimeSource;

struct NoTime;
impl TimeSource for NoTime {
    fn offset(&self, _timeline: u64) -> Option<i64> {
        None
    }
}

fn playout() -> Playout {
    let cfg = Config {
        device: "null".into(),
        ..Config::default()
    };
    Playout::start(cfg, Arc::new(NoTime), None)
}

#[test]
fn buffer_is_bounded_under_a_fast_source() {
    let p = playout();
    // 6000 уникальных пакетов AAC (1024 кадра) — больше предела 4096.
    for i in 0..6000u32 {
        p.push(i * 1024, 96, vec![0u8; 8]);
    }
    assert!(
        p.buffered_packets() <= 4096,
        "буфер вырос до {}",
        p.buffered_packets()
    );
    p.stop();
}

#[test]
fn duplicate_timestamps_are_stored_once() {
    let p = playout();
    p.push(0, 96, vec![1u8; 8]);
    let first = p.buffered_packets();
    p.push(0, 96, vec![2u8; 8]);
    assert!(
        p.buffered_packets() <= first.max(1),
        "дубль метки не должен добавлять пакет"
    );
    p.stop();
}

#[test]
fn stop_stops_accepting_packets() {
    let p = playout();
    p.push(0, 96, vec![1u8; 8]);
    p.stop();
    p.push(1024, 96, vec![2u8; 8]);
    assert_eq!(p.buffered_packets(), 0, "после stop пакеты не копятся");
}

#[test]
fn clear_releases_everything_and_keeps_running() {
    let p = playout();
    for i in 0..100u32 {
        p.push(i * 1024, 96, vec![0u8; 8]);
    }
    p.clear();
    assert_eq!(p.buffered_packets(), 0);
    // Приём продолжается после очистки.
    p.push(200 * 1024, 96, vec![0u8; 8]);
    assert!(p.buffered_packets() <= 1);
    p.stop();
}

#[test]
fn long_silence_gap_does_not_resurrect_old_packets() {
    let p = playout();
    p.push(0, 96, vec![1u8; 8]);
    // Длинная пауза источника: FLUSHBUFFERED без from сохраняет только кадры
    // начиная с until, всё прошлое отпускается.
    p.flush(None, 10_000_000);
    assert_eq!(p.buffered_packets(), 0);
    p.push(10_000_000, 96, vec![2u8; 8]);
    assert!(p.buffered_packets() <= 1);
    p.stop();
}
