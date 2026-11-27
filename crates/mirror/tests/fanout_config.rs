//! Fanout зеркалирования: смена конфигурационного кадра и уход подписчиков
//! (C41).
//!
//! `FanOutSink` синхронный: очередь на подписчика — это сам потребитель, и
//! память ограничена его политикой. Тесты фиксируют порядок событий, повтор
//! параметров кадра всем подписчикам и отсутствие доставки после `end`.
//! Число 60 FPS и реальный приёмник здесь не проверяются.

use std::sync::{Arc, Mutex};

use mirror::{Codec, FanOutSink, VideoSink};

#[derive(Debug, PartialEq, Clone)]
enum Event {
    Config(Codec, Vec<u8>),
    Frame(u64, Vec<u8>),
    End,
}

struct Recorder(Arc<Mutex<Vec<Event>>>);

impl VideoSink for Recorder {
    fn config(&mut self, codec: Codec, record: &[u8]) {
        self.0
            .lock()
            .unwrap()
            .push(Event::Config(codec, record.to_vec()));
    }

    fn frame(&mut self, pts_ns: u64, nals: &[u8]) {
        self.0
            .lock()
            .unwrap()
            .push(Event::Frame(pts_ns, nals.to_vec()));
    }

    fn end(&mut self) {
        self.0.lock().unwrap().push(Event::End);
    }
}

fn recorder() -> (Arc<Mutex<Vec<Event>>>, Box<dyn VideoSink>) {
    let log = Arc::new(Mutex::new(Vec::new()));
    (log.clone(), Box::new(Recorder(log)))
}

#[test]
fn every_subscriber_sees_a_mid_stream_config_change_before_the_next_frame() {
    let (a, sa) = recorder();
    let (b, sb) = recorder();
    let mut sink = FanOutSink::new(vec![sa, sb]);

    sink.config(Codec::H264, b"avcC-1");
    sink.frame(1, b"p-frame-1");
    // Смена параметров (например, поворот экрана) — новый конфигурационный кадр.
    sink.config(Codec::H264, b"avcC-2");
    sink.frame(2, b"p-frame-2");

    let expected = vec![
        Event::Config(Codec::H264, b"avcC-1".to_vec()),
        Event::Frame(1, b"p-frame-1".to_vec()),
        Event::Config(Codec::H264, b"avcC-2".to_vec()),
        Event::Frame(2, b"p-frame-2".to_vec()),
    ];
    assert_eq!(*a.lock().unwrap(), expected);
    assert_eq!(*b.lock().unwrap(), expected);
}

#[test]
fn codec_switch_is_delivered_to_all_subscribers() {
    let (a, sa) = recorder();
    let (b, sb) = recorder();
    let mut sink = FanOutSink::new(vec![sa, sb]);
    sink.config(Codec::H264, b"avcC");
    sink.config(Codec::H265, b"hvcC");
    for log in [a, b] {
        let log = log.lock().unwrap();
        assert_eq!(
            *log,
            vec![
                Event::Config(Codec::H264, b"avcC".to_vec()),
                Event::Config(Codec::H265, b"hvcC".to_vec()),
            ]
        );
    }
}

#[test]
fn end_reaches_every_subscriber_exactly_once() {
    let (a, sa) = recorder();
    let (b, sb) = recorder();
    let mut sink = FanOutSink::new(vec![sa, sb]);
    sink.frame(1, b"x");
    sink.end();
    for log in [a, b] {
        let log = log.lock().unwrap();
        assert_eq!(log.iter().filter(|e| **e == Event::End).count(), 1);
        assert_eq!(log.last(), Some(&Event::End));
    }
}

#[test]
fn each_subscriber_receives_the_full_frame_and_its_pts() {
    // Fanout synchronously forwards the complete frame; a subscriber may
    // still copy or retain it, so this test does not prove a memory bound.
    let (a, sa) = recorder();
    let mut sink = FanOutSink::new(vec![sa]);
    let big = vec![0xab; 256 * 1024];
    sink.frame(7, &big);
    sink.frame(8, b"small");
    let log = a.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0], Event::Frame(7, big));
    assert_eq!(log[1], Event::Frame(8, b"small".to_vec()));
}

#[test]
fn a_slow_subscriber_does_not_skip_its_own_events() {
    // Backpressure документирован: медленный потребитель тормозит остальных,
    // но не теряет кадры, пока сам не решит их отбросить.
    struct Slow(Arc<Mutex<Vec<u64>>>);
    impl VideoSink for Slow {
        fn config(&mut self, _: Codec, _: &[u8]) {}
        fn frame(&mut self, pts: u64, _: &[u8]) {
            std::thread::sleep(std::time::Duration::from_millis(1));
            self.0.lock().unwrap().push(pts);
        }
        fn end(&mut self) {}
    }
    let (fast, sf) = recorder();
    let slow = Arc::new(Mutex::new(Vec::new()));
    let mut sink = FanOutSink::new(vec![Box::new(Slow(slow.clone())), sf]);
    for pts in 0..20 {
        sink.frame(pts, b"f");
    }
    assert_eq!(*slow.lock().unwrap(), (0..20).collect::<Vec<_>>());
    assert_eq!(fast.lock().unwrap().len(), 20);
}
