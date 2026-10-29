//! [`DecodingSink`]: приёмник видео зеркалирования, который декодирует
//! кадры в своём потоке и отдаёт их в [`FrameSink`] в срок.
//!
//! Декодирование — в отдельном потоке ОС, а не в задаче tokio: кадр
//! 1080p декодируется миллисекунды и занял бы рабочий поток рантайма.
//! Очередь к нему короткая; если декодер не успевает, приём ждёт, и TCP
//! притормаживает отправителя — кадры не теряются посреди группы, после
//! чего картинка рассыпалась бы до следующего ключевого. Ждёт приём через
//! `block_in_place`, чтобы рантайм отдал остальные задачи (звук, RTSP)
//! другим потокам.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use clock::{NTP_TIMELINE, TimeSource, now_ns};
use codec::video::{Picture, VideoCodec, VideoDecoder, VideoOptions};
use mirror::{Codec, StreamInfo, VideoSink};
use tracing::{info, warn};

use crate::Pacer;

/// Кадров в очереди к декодеру.
const QUEUE: usize = 8;
/// Кадр со временем показа дальше этого — часы отправителя врут или ещё не
/// сошлись: показываем не позже.
const MAX_AHEAD: Duration = Duration::from_secs(2);
/// Опоздавший больше этого кадр не показываем (как `max-lateness` у
/// видеовыводов GStreamer).
const MAX_LATE: Duration = Duration::from_millis(40);
/// Столько опоздавших подряд — значит, часы не те, а не декодер отстал:
/// дальше показываем без синхронизации, иначе экран замрёт.
const LATE_STREAK: u32 = 30;
const STATS_EVERY: Duration = Duration::from_secs(5);

/// Куда идут декодированные кадры. Вызовы — из потока декодера.
pub trait FrameSink: Send {
    /// Показать кадр. Кадр можно держать (он считается ссылками), но
    /// декодер ждёт возврата из `show`.
    fn show(&mut self, picture: Picture);
    /// Поток закончился.
    fn end(&mut self) {}
}

/// Ничего не показывает: чтобы мерить скорость декодера на железе.
pub struct Discard;

impl FrameSink for Discard {
    fn show(&mut self, _picture: Picture) {}
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub decoder: VideoOptions,
    /// `None` — показывать кадр сразу после декодирования (меньше всего
    /// задержка). `Some(d)` — в момент кадра по часам отправителя плюс `d`:
    /// вровень со звуком, опоздавшие кадры отбрасываются.
    pub delay: Option<Duration>,
}

enum Msg {
    Config(Codec, Vec<u8>),
    Frame(u64, Vec<u8>),
    End,
}

pub struct DecodingSink {
    tx: SyncSender<Msg>,
}

impl DecodingSink {
    pub fn new(info: &StreamInfo, opts: Options, sink: Box<dyn FrameSink>) -> Self {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let time = info.time.clone();
        let spawned = thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || Worker::new(opts, time, sink).run(rx));
        if let Err(e) = spawned {
            warn!("поток декодера видео: {e}");
        }
        Self { tx }
    }
}

impl DecodingSink {
    fn send(&self, msg: Msg) {
        let msg = match self.tx.try_send(msg) {
            Err(TrySendError::Full(msg)) => msg,
            // Отправлено или поток декодера уже кончился.
            _ => return,
        };
        let multi_thread = tokio::runtime::Handle::try_current()
            .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread);
        if multi_thread {
            tokio::task::block_in_place(|| self.tx.send(msg)).ok();
        } else {
            self.tx.send(msg).ok();
        }
    }
}

impl VideoSink for DecodingSink {
    fn config(&mut self, codec: Codec, record: &[u8]) {
        self.send(Msg::Config(codec, record.to_vec()));
    }

    fn frame(&mut self, pts_ns: u64, nals: &[u8]) {
        self.send(Msg::Frame(pts_ns, nals.to_vec()));
    }

    fn end(&mut self) {
        self.send(Msg::End);
    }
}

struct Worker {
    opts: Options,
    time: Arc<dyn TimeSource>,
    sink: Box<dyn FrameSink>,
    decoder: Option<VideoDecoder>,
    /// Пакет настроек, с которым создан `decoder`.
    config: Option<Vec<u8>>,
    pacer: Pacer<Picture>,
    late_streak: u32,
    stats: Stats,
}

impl Worker {
    fn new(opts: Options, time: Arc<dyn TimeSource>, sink: Box<dyn FrameSink>) -> Self {
        let skip_late = opts.delay.is_some();
        Self {
            opts,
            time,
            sink,
            decoder: None,
            config: None,
            pacer: Pacer::new(skip_late),
            late_streak: 0,
            stats: Stats::new(),
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.pacer.next_due() {
                Some(due) => {
                    let wait = (due - now_ns() as i64).max(0) as u64;
                    rx.recv_timeout(Duration::from_nanos(wait))
                }
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(Msg::Config(codec, record)) => self.configure(codec, &record),
                Ok(Msg::Frame(pts_ns, nals)) => self.decode(pts_ns, nals),
                Ok(Msg::End) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.present();
        }
        self.finish();
    }

    fn configure(&mut self, codec: Codec, record: &[u8]) {
        if self.decoder.is_some() && self.config.as_deref() == Some(record) {
            return;
        }
        self.config = Some(record.to_vec());
        // Новые параметры (поворот, другое разрешение) — новый декодер;
        // кадры старого уже не нужны.
        self.pacer.clear();
        self.late_streak = 0;
        let Some(ps) = mirror::parameter_sets(codec, record) else {
            warn!(?codec, "не разобрать параметры видео");
            self.decoder = None;
            return;
        };
        let kind = match codec {
            Codec::H264 => VideoCodec::H264,
            Codec::H265 => VideoCodec::H265,
        };
        self.decoder = match VideoDecoder::new(kind, &ps, &self.opts.decoder) {
            Ok(d) => {
                info!(?codec, decoder = d.name(), "декодер видео");
                Some(d)
            }
            Err(e) => {
                warn!(?codec, "декодер видео: {e}");
                None
            }
        };
    }

    fn decode(&mut self, pts_ns: u64, mut nals: Vec<u8>) {
        let Some(decoder) = self.decoder.as_mut() else {
            return;
        };
        mirror::avcc_to_annex_b(&mut nals);
        let started = Instant::now();
        let mut out = Vec::new();
        if let Err(e) = decoder.decode(pts_ns as i64, &nals, &mut |p| out.push(p)) {
            self.stats.errors += 1;
            tracing::debug!("кадр видео: {e}");
        }
        self.stats.decoded(started.elapsed());
        for p in out {
            self.enqueue(p);
        }
    }

    fn enqueue(&mut self, picture: Picture) {
        let now = now_ns() as i64;
        let synced = self.opts.delay.filter(|_| self.late_streak < LATE_STREAK);
        let due = match (synced, self.time.offset(NTP_TIMELINE)) {
            (Some(delay), Some(offset)) => {
                let due = picture.pts() - offset + delay.as_nanos() as i64;
                if due < now - MAX_LATE.as_nanos() as i64 {
                    self.late_streak += 1;
                    if self.late_streak == LATE_STREAK {
                        warn!(
                            late_ms = (now - due) / 1_000_000,
                            "видео: все кадры опаздывают, показ без синхронизации"
                        );
                    }
                    self.pacer.skip();
                    return;
                }
                self.late_streak = 0;
                due.min(now + MAX_AHEAD.as_nanos() as i64)
            }
            _ => now,
        };
        self.pacer.push(due, picture);
    }

    fn present(&mut self) {
        while let Some(p) = self.pacer.pop_due(now_ns() as i64) {
            self.sink.show(p);
            self.stats.shown += 1;
        }
        self.stats.report(self.pacer.dropped());
    }

    fn finish(&mut self) {
        if let Some(mut decoder) = self.decoder.take() {
            let mut out = Vec::new();
            decoder.flush(&mut |p| out.push(p));
            for p in out {
                self.enqueue(p);
            }
            // Досматриваем, что осталось, в свой срок (не дальше MAX_AHEAD).
            while let Some(due) = self.pacer.next_due() {
                let wait = (due - now_ns() as i64).max(0) as u64;
                thread::sleep(Duration::from_nanos(wait));
                self.present();
            }
        }
        self.stats.report_now(self.pacer.dropped());
        self.sink.end();
    }
}

/// Раз в несколько секунд — в лог: к/с, отброшенные кадры, время декодера.
struct Stats {
    since: Instant,
    shown: u64,
    frames: u64,
    errors: u64,
    decode_time: Duration,
    worst: Duration,
    reported_dropped: u64,
}

impl Stats {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            shown: 0,
            frames: 0,
            errors: 0,
            decode_time: Duration::ZERO,
            worst: Duration::ZERO,
            reported_dropped: 0,
        }
    }

    fn decoded(&mut self, took: Duration) {
        self.frames += 1;
        self.decode_time += took;
        self.worst = self.worst.max(took);
    }

    fn report(&mut self, dropped: u64) {
        if self.since.elapsed() >= STATS_EVERY {
            self.report_now(dropped);
        }
    }

    fn report_now(&mut self, dropped: u64) {
        let secs = self.since.elapsed().as_secs_f64().max(1e-3);
        if self.frames > 0 {
            let avg = self.decode_time / self.frames as u32;
            info!(
                fps = format!("{:.1}", self.shown as f64 / secs),
                dropped = dropped - self.reported_dropped,
                errors = self.errors,
                decode_ms = format!("{:.1}", avg.as_secs_f64() * 1e3),
                worst_ms = format!("{:.1}", self.worst.as_secs_f64() * 1e3),
                "видео"
            );
        }
        *self = Self {
            reported_dropped: dropped,
            ..Self::new()
        };
    }
}
