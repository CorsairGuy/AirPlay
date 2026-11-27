//! Планировщик показа: пауза, смена формата и поздние кадры (C47).
//!
//! Тесты идут на управляемых метках времени, без libavcodec и рендера.
//! Смена формата моделируется новым периодом меток после `clear()` — так
//! делает вызывающий код после нового конфигурационного кадра.

use video::Pacer;

#[test]
fn pause_drops_nothing_but_does_not_grow_after_clear() {
    let mut p = Pacer::new(true);
    for i in 0..10 {
        p.push(i * 10, i);
    }
    assert_eq!(p.len(), 10);
    // Пауза: очередь сброшена, новые кадры не копятся до возобновления.
    p.clear();
    assert!(p.is_empty());
    assert_eq!(p.next_due(), None);

    // Возобновление с новой шкалой.
    let mut p: Pacer<&str> = Pacer::new(true);
    p.push(1_000, "a");
    p.push(1_010, "b");
    assert_eq!(p.pop_due(999), None);
    assert_eq!(p.pop_due(1_000), Some("a"));
    assert_eq!(p.pop_due(1_010), Some("b"));
}

#[test]
fn format_change_reanchors_time_without_stale_frames() {
    let mut p = Pacer::new(true);
    p.push(500, "old-1");
    p.push(510, "old-2");
    p.clear(); // смена параметров видео
    p.push(0, "new-1");
    p.push(10, "new-2");
    assert_eq!(p.pop_due(0), Some("new-1"));
    assert_eq!(p.pop_due(10), Some("new-2"));
    assert!(p.is_empty());
}

#[test]
fn out_of_order_arrival_keeps_display_order() {
    let mut p = Pacer::new(false);
    p.push(30, 3);
    p.push(10, 1);
    p.push(20, 2);
    let order: Vec<_> = std::iter::from_fn(|| p.pop_due(i64::MAX)).collect();
    assert_eq!(order, vec![1, 2, 3]);
}

#[test]
fn equal_timestamps_keep_arrival_order() {
    let mut p: Pacer<char> = Pacer::new(true);
    p.push(10, 'a');
    p.push(10, 'b');
    p.push(10, 'c');
    // При skip_late и одинаковом времени остаётся последний.
    assert_eq!(p.pop_due(10), Some('c'));
    assert_eq!(p.dropped(), 2);
}

#[test]
fn late_frames_are_counted_not_queued_unbounded() {
    let mut p = Pacer::new(true);
    p.push(100, 1);
    p.push(200, 2);
    p.push(300, 3);
    // Показ отстал на две метки: показывается самый свежий, остальные — drop.
    assert_eq!(p.pop_due(350), Some(3));
    assert_eq!(p.dropped(), 2);
    assert!(p.is_empty());
}

#[test]
fn queue_is_bounded_even_without_consumption() {
    let mut p = Pacer::new(false);
    for i in 0..1_000 {
        p.push(i, i);
    }
    assert!(p.len() <= 120, "очередь выросла до {}", p.len());
    assert_eq!(p.dropped(), 1_000 - p.len() as u64);
}

#[test]
fn externally_late_frame_is_only_counted() {
    let mut p: Pacer<u8> = Pacer::new(true);
    p.skip();
    p.skip();
    assert_eq!(p.dropped(), 2);
    assert!(p.is_empty());
    assert_eq!(p.pop_due(i64::MAX), None);
}

#[test]
fn negative_timestamps_are_ordered() {
    // Метки до начала шкалы допустимы; порядок остаётся монотонным.
    let mut p = Pacer::new(false);
    p.push(-20, "a");
    p.push(-10, "b");
    p.push(0, "c");
    let order: Vec<_> = std::iter::from_fn(|| p.pop_due(0)).collect();
    assert_eq!(order, vec!["a", "b", "c"]);
}
