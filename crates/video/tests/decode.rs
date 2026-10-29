//! Настоящие потоки H.264 и H.265 (testsrc 128x72, 12 кадров, x264/x265) в
//! том виде, в каком их отдаёт приём зеркалирования: пакет настроек
//! (`avcC` или описание `hvc1`, как шлёт iOS) и кадры в AVCC.
#![cfg(feature = "ffmpeg")]

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ap_video::{DecodingSink, FrameSink, Options, Picture, PixelFormat};
use mirror::{Codec, StreamInfo, VideoSink};

/// Файл: u32 длина + настройки, дальше u32 длина + кадр.
fn load(data: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut chunks = Vec::new();
    let mut rest = data;
    while rest.len() >= 4 {
        let len = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        chunks.push(rest[4..4 + len].to_vec());
        rest = &rest[4 + len..];
    }
    let config = chunks.remove(0);
    (config, chunks)
}

#[derive(Clone)]
struct Collect {
    shown: Arc<Mutex<Shown>>,
    done: mpsc::Sender<()>,
}

impl FrameSink for Collect {
    fn show(&mut self, p: Picture) {
        assert!(p.planes() >= 2);
        assert!(p.stride(0) >= p.width() as usize);
        let y = *p.plane(0).iter().max().unwrap();
        self.shown
            .lock()
            .unwrap()
            .push((p.width(), p.height(), p.pts(), p.format(), y));
    }
    fn end(&mut self) {
        self.done.send(()).unwrap();
    }
}

struct FixedClock(i64);

impl clock::TimeSource for FixedClock {
    fn offset(&self, _timeline: u64) -> Option<i64> {
        Some(self.0)
    }
}

type Shown = Vec<(u32, u32, i64, PixelFormat, u8)>;

/// Поток `repeat` раз подряд, кадры через 1/60 с начиная с 1 с.
fn run(
    codec: Codec,
    file: &[u8],
    repeat: usize,
    opts: Options,
    time: Arc<dyn clock::TimeSource>,
) -> Shown {
    let (config, frames) = load(file);
    let (done, wait) = mpsc::channel();
    let sink = Collect {
        shown: Default::default(),
        done,
    };
    let shown = sink.shown.clone();
    let mut s = DecodingSink::new(&StreamInfo { time }, opts, Box::new(sink));
    s.config(codec, &config);
    for (i, f) in frames
        .iter()
        .cycle()
        .take(frames.len() * repeat)
        .enumerate()
    {
        s.frame(1_000_000_000 + i as u64 * 16_666_667, f);
    }
    s.end();
    wait.recv_timeout(Duration::from_secs(20))
        .expect("декодер не закончил");
    shown.lock().unwrap().clone()
}

fn check(codec: Codec, file: &[u8]) {
    let shown = run(codec, file, 1, Options::default(), Arc::new(clock::NoTime));
    assert_eq!(shown.len(), 12, "{codec:?}");
    for (i, (w, h, pts, format, _)) in shown.iter().enumerate() {
        assert_eq!((*w, *h), (128, 72));
        assert_eq!(*format, PixelFormat::Yuv420p);
        assert_eq!(
            *pts,
            1_000_000_000 + i as i64 * 16_666_667,
            "порядок показа"
        );
    }
    // testsrc не чёрный.
    assert!(shown.iter().all(|f| f.4 > 100));
}

#[test]
fn decodes_h264() {
    check(Codec::H264, include_bytes!("data/h264.mirror"));
}

#[test]
fn decodes_h265_from_ios_sample_description() {
    check(Codec::H265, include_bytes!("data/h265.mirror"));
}

/// Из задачи tokio, как в приёмнике: полная очередь не держит рабочий поток.
#[test]
fn inside_tokio_runtime() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .build()
        .unwrap();
    let shown = rt.block_on(async {
        tokio::spawn(async {
            run(
                Codec::H264,
                include_bytes!("data/h264.mirror"),
                4,
                Options::default(),
                Arc::new(clock::NoTime),
            )
        })
        .await
        .unwrap()
    });
    assert_eq!(shown.len(), 48);
}

#[test]
fn low_delay_decoder() {
    let opts = Options {
        decoder: ap_video::VideoOptions {
            low_delay: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let shown = run(
        Codec::H264,
        include_bytes!("data/h264.mirror"),
        1,
        opts,
        Arc::new(clock::NoTime),
    );
    assert_eq!(shown.len(), 12);
}

fn late_clock() -> Arc<dyn clock::TimeSource> {
    // pts ≈ 1 с по часам отправителя, а локально это 10 с назад.
    Arc::new(FixedClock(
        1_000_000_000 - clock::now_ns() as i64 + 10_000_000_000,
    ))
}

fn synced() -> Options {
    Options {
        delay: Some(Duration::from_millis(100)),
        ..Default::default()
    }
}

/// По часам отправителя: опоздавшие кадры не показываются.
#[test]
fn synced_drops_late_frames() {
    let shown = run(
        Codec::H264,
        include_bytes!("data/h264.mirror"),
        1,
        synced(),
        late_clock(),
    );
    assert!(shown.is_empty(), "показано {}", shown.len());
}

/// Опаздывают все кадры подряд — часы врут: показ продолжается без
/// синхронизации, экран не замирает.
#[test]
fn persistent_lateness_falls_back_to_asap() {
    let shown = run(
        Codec::H264,
        include_bytes!("data/h264.mirror"),
        3,
        synced(),
        late_clock(),
    );
    assert!(
        !shown.is_empty() && shown.len() <= 36 - 30,
        "показано {}",
        shown.len()
    );
}

/// Кадры в будущем ждут своего времени и показываются по порядку.
#[test]
fn synced_waits_for_due_time() {
    // pts 1 с по часам отправителя — это «сейчас» локально, плюс задержка.
    let offset = 1_000_000_000 - clock::now_ns() as i64;
    let started = std::time::Instant::now();
    let shown = run(
        Codec::H264,
        include_bytes!("data/h264.mirror"),
        1,
        synced(),
        Arc::new(FixedClock(offset)),
    );
    assert_eq!(shown.len(), 12);
    assert!(shown.windows(2).all(|w| w[0].2 < w[1].2));
    // Последний кадр — через 100 мс задержки и 11 кадров по 16,7 мс.
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn unknown_decoder_is_not_fatal() {
    let opts = Options {
        decoder: ap_video::VideoOptions {
            decoder: Some("нет_такого".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let shown = run(
        Codec::H264,
        include_bytes!("data/h264.mirror"),
        1,
        opts,
        Arc::new(clock::NoTime),
    );
    assert!(shown.is_empty());
}
