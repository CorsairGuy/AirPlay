//! Синтетический адаптер источника в духе Station поверх публичной политики
//! `media::tx::continuous::Timeline`: pause, seek (новая генерация) и EOF.
//!
//! Это проверка контракта ядра, а не адаптер Station: ни Quasar, ни ALSA, ни
//! файлы/демоны Станции здесь не участвуют. Ревизия внешнего описания
//! `a1aefc79/docs/CORE_SENDER_ADAPTER.md` на момент проверки недоступна (404),
//! поэтому таблица контрактов в docs/STATION-CORE-CONTRACTS.md опирается на
//! локальную инвентаризацию и код ядра.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use media::tx::continuous::{Input, Step, Timeline};
use media::tx::{FRAMES_PER_PACKET, PACKET_BYTES};

const FRAME: Duration = Duration::from_nanos(7_982_000); // 352 frames at 44.1 kHz с запасом

struct Adapter {
    timeline: Timeline,
    queue: VecDeque<(u64, Box<[u8; PACKET_BYTES]>)>,
    paused: bool,
}

impl Adapter {
    fn new(epoch: Instant) -> Self {
        Self {
            timeline: Timeline::new(epoch, Duration::from_secs(2)).unwrap(),
            queue: VecDeque::new(),
            paused: false,
        }
    }

    fn push(&mut self, generation: u64, byte: u8) {
        self.queue.push_back((generation, Box::new([byte; PACKET_BYTES])));
    }

    fn pause(&mut self, now: Instant) {
        self.timeline.pause(now).unwrap();
        self.paused = true;
    }

    fn resume(&mut self) {
        self.timeline.resume();
        self.paused = false;
    }

    fn seek(&mut self) -> u64 {
        self.queue.clear();
        self.timeline.invalidate_source().unwrap()
    }

    fn poll(&mut self, now: Instant) -> Step {
        let input = self
            .queue
            .front()
            .map(|(generation, pcm)| Input {
                generation: *generation,
                pcm: &pcm[..],
            });
        let step = self.timeline.poll(now, input).unwrap();
        // На паузе Frame — тишина по расписанию, источник не потребляется.
        if !self.paused
            && matches!(
                step,
                Step::Frame { .. } | Step::DiscardStale | Step::Skip { .. }
            )
        {
            self.queue.pop_front();
        }
        step
    }
}

fn due(epoch: Instant, index: u64) -> Instant {
    epoch + FRAME * ((index / FRAMES_PER_PACKET as u64) as u32) + Duration::from_millis(1)
}

#[test]
fn pause_emits_silence_without_consuming_the_source() {
    let epoch = Instant::now();
    let mut adapter = Adapter::new(epoch);
    adapter.push(0, 7);
    adapter.push(0, 8);

    let Step::Frame {
        frame_index,
        pcm,
        generation,
    } = adapter.poll(epoch)
    else {
        panic!("первый кадр")
    };
    assert_eq!((frame_index, generation), (0, 0));
    assert!(pcm.iter().all(|b| *b == 7));

    adapter.pause(epoch + FRAME);
    let Step::Frame {
        frame_index,
        pcm,
        generation,
    } = adapter.poll(due(epoch, 352))
    else {
        panic!("пауза обязана выдать тишину по расписанию")
    };
    assert_eq!((frame_index, generation), (352, 0));
    assert!(pcm.iter().all(|b| *b == 0), "на паузе данных нет");
    assert_eq!(adapter.queue.len(), 1, "кадр источника не съеден");

    adapter.resume();
    let Step::Frame { frame_index, pcm, .. } = adapter.poll(due(epoch, 704)) else {
        panic!("после resume")
    };
    assert_eq!(frame_index, 704);
    assert!(pcm.iter().all(|b| *b == 8), "resume отдаёт отложенный кадр");
    assert!(adapter.queue.is_empty());
}

#[test]
fn seek_drops_old_generation_and_keeps_media_time() {
    let epoch = Instant::now();
    let mut adapter = Adapter::new(epoch);
    adapter.push(0, 1);
    adapter.push(0, 2);
    adapter.push(0, 3);

    assert!(matches!(adapter.poll(epoch), Step::Frame { .. }));
    let generation = adapter.seek();
    assert_eq!(generation, 1);
    assert_eq!(adapter.queue.len(), 0, "seek очищает очередь адаптера");

    // Кадр, захваченный до seek, ядро помечает устаревшим.
    adapter.push(0, 4);
    assert_eq!(adapter.poll(due(epoch, 352)), Step::DiscardStale);
    assert!(adapter.queue.is_empty());

    adapter.push(1, 5);
    let Step::Frame {
        frame_index,
        generation,
        pcm,
    } = adapter.poll(due(epoch, 352))
    else {
        panic!("кадр нового поколения")
    };
    assert_eq!((frame_index, generation), (352, 1));
    assert!(pcm.iter().all(|b| *b == 5));
    assert_eq!(
        adapter.timeline.generation(),
        1,
        "seek не увеличивает поколение повторно"
    );
}

#[test]
fn eof_fills_silence_after_grace_and_does_not_restart_the_timeline() {
    let epoch = Instant::now();
    let mut adapter = Adapter::new(epoch);
    assert_eq!(adapter.poll(epoch), Step::Wait(epoch + Duration::from_millis(40)));

    let Step::Frame {
        frame_index, pcm, ..
    } = adapter.poll(epoch + Duration::from_millis(40))
    else {
        panic!("EOF: тишина после grace")
    };
    assert_eq!(frame_index, 0);
    assert!(pcm.iter().all(|b| *b == 0));

    // Новый кадр после EOF продолжает ту же медиашкалу, а не начинает заново.
    adapter.push(0, 9);
    let Step::Frame {
        frame_index, pcm, ..
    } = adapter.poll(due(epoch, 352))
    else {
        panic!("кадр после EOF")
    };
    assert_eq!(frame_index, 352);
    assert!(pcm.iter().all(|b| *b == 9));
    assert_eq!(adapter.timeline.epoch(), epoch, "epoch не пересоздаётся");
}

#[test]
fn stale_generation_after_seek_is_rejected_not_autoadvanced() {
    let epoch = Instant::now();
    let mut adapter = Adapter::new(epoch);
    adapter.seek();
    adapter.push(2, 1); // поколение из будущего никто не объявлял
    let input = adapter.queue.front().map(|(generation, pcm)| Input {
        generation: *generation,
        pcm: &pcm[..],
    });
    assert!(
        adapter.timeline.poll(epoch, input).is_err(),
        "необъявленное поколение — ошибка, а не тихий пропуск"
    );
}
