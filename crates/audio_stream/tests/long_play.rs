//! Ускоренный стенд длительной аудиосессии: виртуальные метки времени вместо
//! ожидания часов реального времени.
//!
//! Звук идёт в собственный `Sink`, поэтому звуковая карта не нужна, а метки RTP
//! охватывают часы медиа за доли секунды. Это проверка логики вывода, а не
//! многочасовой аппаратный прогон: реальные часы, PTP и карта не участвуют.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use audio::Sink;
use audio_stream::{Config, Playout};
use clock::{Anchor, NoTime};
use codec::{Codec, Format};

const RATE: u32 = 44_100;
const FRAMES: u32 = 1024;
const CHANNELS: usize = 2;

fn pcm_format() -> Format {
    Format {
        codec: Codec::Pcm {
            frames_per_packet: FRAMES,
        },
        sample_rate: RATE,
        channels: 2,
    }
}

/// Пакет-маркер: все сэмплы равны `marker`, чтобы порядок был виден в том, что
/// реально получил вывод.
fn pcm_packet(marker: i16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(FRAMES as usize * CHANNELS * 2);
    for _ in 0..FRAMES * CHANNELS as u32 {
        payload.extend_from_slice(&marker.to_be_bytes());
    }
    payload
}

#[derive(Default)]
struct Captured {
    samples: Vec<i16>,
    stops: usize,
}

struct Recording(Arc<Mutex<Captured>>);

impl Sink for Recording {
    fn sample_rate(&self) -> u32 {
        RATE
    }

    fn write(&mut self, samples: &[i16]) -> std::io::Result<()> {
        self.0
            .lock()
            .expect("captured")
            .samples
            .extend_from_slice(samples);
        Ok(())
    }

    fn delay_frames(&mut self) -> u32 {
        0
    }

    fn stop(&mut self) {
        self.0.lock().expect("captured").stops += 1;
    }
}

fn playout(captured: &Arc<Mutex<Captured>>) -> Playout {
    let sink = captured.clone();
    let factory =
        audio::SinkFactory::new(move |_, _| Ok(Box::new(Recording(sink.clone())) as Box<dyn Sink>));
    Playout::start(
        Config {
            device: "null".into(),
            output_latency: Duration::from_millis(10),
            fallback_latency: Duration::ZERO,
            sink: Some(factory),
            ..Default::default()
        },
        Arc::new(NoTime),
        Some(pcm_format()),
    )
}

fn anchor(rtp: u32, playing: bool) -> Anchor {
    Anchor {
        rtp,
        network_ns: 0,
        timeline: 1,
        playing,
    }
}

fn sample_count(captured: &Arc<Mutex<Captured>>) -> usize {
    captured.lock().expect("captured").samples.len()
}

/// Ждёт, пока в выводе появится маркер `marker` (не меньше `count` раз).
fn wait_for_marker(captured: &Arc<Mutex<Captured>>, marker: i16, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let seen = captured
            .lock()
            .expect("captured")
            .samples
            .iter()
            .filter(|sample| **sample == marker)
            .count();
        if seen >= count {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "маркер {marker} не дошёл до вывода ({seen} < {count})"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn first_position(captured: &Arc<Mutex<Captured>>, marker: i16) -> Option<usize> {
    captured
        .lock()
        .expect("captured")
        .samples
        .iter()
        .position(|sample| *sample == marker)
}

/// Переполнение 32-битной метки RTP: пакеты до и после перехода через ноль
/// играют по порядку, без потери и без повтора старого звука.
#[test]
fn packets_play_in_order_across_the_timestamp_wrap() {
    let captured = Arc::new(Mutex::new(Captured::default()));
    let playout = playout(&captured);
    let start = u32::MAX - 3 * FRAMES;
    playout.set_anchor(anchor(start, true));
    for index in 0..8u32 {
        playout.push(
            start.wrapping_add(index * FRAMES),
            0x1600_0000,
            pcm_packet(index as i16),
        );
    }
    wait_for_marker(&captured, 7, 1);

    let mut previous = 0usize;
    for marker in 0..8i16 {
        let position = first_position(&captured, marker)
            .unwrap_or_else(|| panic!("пакет {marker} не прозвучал"));
        assert!(
            position >= previous,
            "маркер {marker} прозвучал раньше предыдущего: порядок нарушен"
        );
        previous = position;
    }
    playout.stop();
}

/// Пауза глушит вывод, повторный якорь продолжает, а очистка буфера при
/// перемотке не возвращает уже игравшие пакеты.
#[test]
fn pause_seek_and_stop_keep_the_stream_consistent() {
    let captured = Arc::new(Mutex::new(Captured::default()));
    let playout = playout(&captured);
    playout.set_anchor(anchor(0, true));
    for index in 0..6u32 {
        playout.push(index * FRAMES, 0x1600_0000, pcm_packet(index as i16));
    }
    wait_for_marker(&captured, 1, 1);

    playout.set_anchor(anchor(6 * FRAMES, false));
    let paused_at = sample_count(&captured);
    std::thread::sleep(Duration::from_millis(120));
    let after_pause = sample_count(&captured);
    assert!(
        after_pause <= paused_at + RATE as usize * CHANNELS / 5,
        "звук шёл на паузе: {paused_at} → {after_pause}"
    );

    playout.flush(None, 20 * FRAMES);
    playout.set_anchor(anchor(20 * FRAMES, true));
    for index in 20..24u32 {
        playout.push(index * FRAMES, 0x1600_0000, pcm_packet(index as i16));
    }
    wait_for_marker(&captured, 21, 1);
    let seek_position = first_position(&captured, 20).expect("первый пакет после перемотки");
    assert!(
        seek_position >= after_pause,
        "после перемотки звук взят из старого буфера"
    );

    playout.stop();
    assert_eq!(playout.buffered_packets(), 0);
    playout.push(99 * FRAMES, 0x1600_0000, pcm_packet(99));
    assert_eq!(playout.buffered_packets(), 0, "пакеты после stop копятся");
}

/// Три часа медиа в быстром темпе: очередь остаётся ограниченной, пакетов не
/// копится больше предела.
#[test]
fn three_hours_of_media_keep_the_queue_bounded() {
    let captured = Arc::new(Mutex::new(Captured::default()));
    let playout = playout(&captured);
    playout.set_anchor(anchor(0, true));
    let packets = 3 * 3600 * RATE / FRAMES;
    for index in 0..packets {
        playout.push(index.wrapping_mul(FRAMES), 0x1600_0000, vec![0; 8]);
    }
    let buffered = playout.buffered_packets();
    assert!(buffered > 0, "вывод не успел забрать ни одного пакета");
    assert!(buffered <= 4096, "очередь выросла до {buffered}");
    playout.stop();
    assert_eq!(playout.buffered_packets(), 0);
}

/// Потеря соединения: закрытая сессия освобождает буфер и вывод, а новая
/// сессия начинает заново с той же карты.
#[test]
fn a_lost_connection_releases_the_buffer_and_a_new_session_starts() {
    let captured = Arc::new(Mutex::new(Captured::default()));
    let first = playout(&captured);
    first.set_anchor(anchor(0, true));
    for index in 0..64u32 {
        first.push(index * FRAMES, 0x1600_0000, pcm_packet(index as i16));
    }
    assert!(first.buffered_packets() > 0);
    drop(first);

    let second = playout(&captured);
    let before = sample_count(&captured);
    second.set_anchor(anchor(0, true));
    second.push(0, 0x1600_0000, pcm_packet(7));
    wait_for_marker(&captured, 7, 1);
    assert!(sample_count(&captured) > before);
    second.stop();
}
