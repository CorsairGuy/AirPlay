//! Буфер пакетов и поток вывода.
//!
//! Пакеты лежат в сжатом виде и декодируются прямо перед выводом: iPhone
//! присылает буферизованный поток на десятки секунд вперёд, и держать его
//! в PCM на слабом устройстве незачем.
//!
//! Подстройка к часам отправителя — как в shairport-sync: если вывод
//! отстаёт или спешит больше чем на полмиллисекунды, в очередной пакет
//! добавляем или из него выбрасываем один кадр. На слух это незаметно, в
//! отличие от ресемплинга не стоит процессору ничего.

use std::collections::BTreeMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use audio::{CHANNELS, Sink};
use clock::{Anchor, TimeSource, now_ns};
use codec::{Decoder, Format};
use tracing::{debug, info, warn};

/// Сколько кадров пишем за раз, если пакета нет (тишина).
const CHUNK: usize = 352;
/// Порог подстройки: ±0,5 мс при 44,1 кГц.
const NUDGE_FRAMES: i64 = 22;
/// Расхождение, после которого не подстраиваемся, а перепрыгиваем.
const RESYNC_NS: i64 = 100_000_000;

#[derive(Debug, Clone)]
pub struct Config {
    /// Устройство вывода: `default`, `null` или имя ALSA.
    pub device: String,
    /// Размер буфера звуковой карты.
    pub output_latency: Duration,
    /// Задержка, с которой играем, если часов отправителя нет (без PTP).
    pub fallback_latency: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: "default".into(),
            output_latency: Duration::from_millis(100),
            fallback_latency: Duration::from_millis(250),
        }
    }
}

struct Packet {
    ssrc: u32,
    payload: Vec<u8>,
}

/// Время с учётом переполнения 32-битной RTP-метки.
#[derive(Default)]
struct Unwrap {
    last: Option<(u32, i64)>,
}

impl Unwrap {
    fn extend(&mut self, ts: u32) -> i64 {
        let ext = match self.last {
            None => ts as i64,
            Some((last, ext)) => ext + rtp::ts_diff(ts, last) as i64,
        };
        self.last = Some((ts, ext));
        ext
    }

    fn peek(&self, ts: u32) -> i64 {
        match self.last {
            None => ts as i64,
            Some((last, ext)) => ext + rtp::ts_diff(ts, last) as i64,
        }
    }
}

#[derive(Default)]
struct State {
    packets: BTreeMap<i64, Packet>,
    unwrap: Unwrap,
    anchor: Option<Anchor>,
    /// Локальное время якоря, если часов отправителя нет.
    fallback_local_ns: i64,
    /// Якорь поставлен по своим часам ([`Playout::play_from`]).
    local_anchor: bool,
    /// Формат из SETUP (realtime); буферизованный поток сообщает его в SSRC.
    format: Option<Format>,
    gain_q15: i32,
    /// Растёт при перемотке и смене якоря: поток вывода начинает заново.
    epoch: u64,
    stopped: bool,
}

pub struct Playout {
    shared: Arc<(Mutex<State>, Condvar)>,
}

impl Playout {
    pub fn start(cfg: Config, time: Arc<dyn TimeSource>, format: Option<Format>) -> Playout {
        let shared = Arc::new((
            Mutex::new(State {
                format,
                gain_q15: 32768,
                ..State::default()
            }),
            Condvar::new(),
        ));
        let thread_shared = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("playout".into())
            .spawn(move || Output::new(cfg, time, thread_shared).run());
        if let Err(e) = spawned {
            warn!("поток вывода не запущен: {e}");
        }
        Playout { shared }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self) {
        self.shared.1.notify_all();
    }

    /// Пакет после расшифровки: метка, SSRC (формат) и сжатые данные.
    pub fn push(&self, timestamp: u32, ssrc: u32, payload: Vec<u8>) {
        let mut s = self.lock();
        let ext = s.unwrap.extend(timestamp);
        s.packets.insert(ext, Packet { ssrc, payload });
        drop(s);
        self.changed();
    }

    /// SETRATEANCHORTIME или синхропакет realtime-потока. Якорь приходит
    /// и во время игры (раз в секунду в realtime): если он согласуется с
    /// прежним, вывод продолжается без щелчка.
    pub fn set_anchor(&self, anchor: Anchor) {
        let mut s = self.lock();
        let rate = s
            .format
            .or_else(|| {
                s.packets
                    .values()
                    .next()
                    .and_then(|p| Format::from_ssrc(p.ssrc))
            })
            .map_or(44_100, |f| f.sample_rate) as i64;
        let jump = match s.anchor {
            None => true,
            Some(old) if s.local_anchor => old.playing != anchor.playing,
            Some(old) => {
                let frames = rtp::ts_diff(anchor.rtp, old.rtp) as i64;
                let expected = old.network_ns as i64 + frames * 1_000_000_000 / rate;
                old.playing != anchor.playing
                    || old.timeline != anchor.timeline
                    || (anchor.network_ns as i64 - expected).abs() > RESYNC_NS
            }
        };
        if jump {
            if anchor.playing {
                info!(rtp = anchor.rtp, "воспроизведение");
            } else {
                info!("пауза");
            }
            s.epoch += 1;
        }
        s.local_anchor = false;
        s.anchor = Some(anchor);
        s.fallback_local_ns = now_ns() as i64;
        drop(s);
        self.changed();
    }

    /// Realtime-поток без часов отправителя: играем с первого пакета через
    /// `fallback_latency` по своим часам.
    pub fn play_from(&self, rtp: u32) {
        let mut s = self.lock();
        if s.anchor.is_some() {
            return;
        }
        info!(rtp, "воспроизведение по своим часам");
        s.anchor = Some(Anchor {
            rtp,
            network_ns: 0,
            timeline: 0,
            playing: true,
        });
        s.local_anchor = true;
        s.fallback_local_ns = now_ns() as i64;
        s.epoch += 1;
        drop(s);
        self.changed();
    }

    /// FLUSHBUFFERED: выбросить пакеты с `from` (если задано) до `until`.
    pub fn flush(&self, from: Option<u32>, until: u32) {
        let mut s = self.lock();
        let until = s.unwrap.peek(until);
        let from = from.map(|f| s.unwrap.peek(f));
        let before = s.packets.len();
        s.packets
            .retain(|&ts, _| ts >= until || from.is_some_and(|f| ts < f));
        debug!(dropped = before - s.packets.len(), "сброс буфера");
        s.epoch += 1;
        drop(s);
        self.changed();
    }

    pub fn set_volume(&self, db: f32) {
        self.lock().gain_q15 = audio::gain_q15(db);
    }

    pub fn stop(&self) {
        self.lock().stopped = true;
        self.changed();
    }

    pub fn buffered_packets(&self) -> usize {
        self.lock().packets.len()
    }
}

impl Drop for Playout {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Output {
    cfg: Config,
    time: Arc<dyn TimeSource>,
    shared: Arc<(Mutex<State>, Condvar)>,
    sink: Option<Box<dyn Sink>>,
    decoder: Option<(u32, Decoder)>,
    /// Следующий кадр к выводу.
    cursor: Option<i64>,
    epoch: u64,
    pcm: Vec<i16>,
}

impl Output {
    fn new(cfg: Config, time: Arc<dyn TimeSource>, shared: Arc<(Mutex<State>, Condvar)>) -> Self {
        Self {
            cfg,
            time,
            shared,
            sink: None,
            decoder: None,
            cursor: None,
            epoch: 0,
            pcm: Vec::new(),
        }
    }

    fn run(mut self) {
        loop {
            let shared = self.shared.clone();
            let (lock, cvar) = &*shared;
            let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if s.stopped {
                    return;
                }
                let playing = s.anchor.is_some_and(|a| a.playing);
                if playing && !s.packets.is_empty() {
                    break;
                }
                if s.epoch != self.epoch {
                    // Пауза или перемотка: замолкаем сразу, а не после буфера карты.
                    self.epoch = s.epoch;
                    self.cursor = None;
                    if let Some(sink) = &mut self.sink {
                        sink.stop();
                    }
                }
                s = cvar.wait(s).unwrap_or_else(|e| e.into_inner());
            }
            if s.epoch != self.epoch {
                self.epoch = s.epoch;
                self.cursor = None;
                if let Some(sink) = &mut self.sink {
                    sink.stop();
                }
            }
            let Some(chunk) = self.next_chunk(&mut s) else {
                drop(s);
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            let gain = s.gain_q15;
            drop(s);
            let mut samples = chunk;
            audio::apply_gain(&mut samples, gain);
            if let Some(sink) = &mut self.sink
                && let Err(e) = sink.write(&samples)
            {
                warn!("вывод звука: {e}");
                self.sink = None;
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }

    /// Готовит очередную порцию сэмплов. Вызывается под замком состояния.
    fn next_chunk(&mut self, s: &mut State) -> Option<Vec<i16>> {
        let anchor = s.anchor?;
        let (&first_ts, first) = s.packets.iter().next()?;
        let format = s.format.or_else(|| Format::from_ssrc(first.ssrc))?;
        let rate = format.sample_rate;
        self.ensure_sink(rate)?;
        let sink = self.sink.as_mut()?;

        // Когда прозвучит кадр, который мы запишем сейчас.
        let heard_at = now_ns() as i64 + sink.delay_frames() as i64 * 1_000_000_000 / rate as i64;
        let anchor_ext = s.unwrap.peek(anchor.rtp);
        let offset = match s.local_anchor {
            true => None,
            false => self.time.offset(anchor.timeline),
        };
        let anchor_local = match offset {
            Some(offset) => anchor.network_ns as i64 - offset,
            None => s.fallback_local_ns + self.cfg.fallback_latency.as_nanos() as i64,
        };
        let due = anchor_ext + (heard_at - anchor_local) * rate as i64 / 1_000_000_000;

        let cursor = *self.cursor.get_or_insert_with(|| {
            debug!(due, first_ts, "старт вывода");
            due
        });
        let error = cursor - due; // > 0: спешим, < 0: опаздываем
        let error_ns = error * 1_000_000_000 / rate as i64;
        if error_ns.abs() > RESYNC_NS {
            if error > 0 {
                // Рано: допишем тишину до нужного момента.
                let frames = error.min(rate as i64) as usize;
                return Some(vec![0; frames * CHANNELS]);
            }
            debug!(late_ms = -error_ns / 1_000_000, "опоздание, перепрыгиваем");
            self.cursor = Some(due);
            return Some(Vec::new());
        }

        // Старое уже не сыграть.
        let frames_per_packet = format.frames_per_packet() as i64;
        let cursor = self.cursor.unwrap_or(due);
        while let Some((&ts, _)) = s.packets.iter().next() {
            if ts + frames_per_packet > cursor {
                break;
            }
            s.packets.remove(&ts);
        }

        let mut out = match s.packets.range(..=cursor).next_back() {
            Some((&ts, p)) if ts + frames_per_packet > cursor => {
                let ssrc = p.ssrc;
                let payload = std::mem::take(&mut s.packets.get_mut(&ts)?.payload);
                s.packets.remove(&ts);
                let pcm = self.decode(format, ssrc, &payload);
                let skip = ((cursor - ts) as usize * CHANNELS).min(pcm.len());
                pcm[skip..].to_vec()
            }
            _ => {
                // Дыра в потоке: тишина до следующего пакета.
                let gap = s
                    .packets
                    .range(cursor..)
                    .next()
                    .map_or(CHUNK as i64, |(&ts, _)| ts - cursor);
                vec![0; gap.clamp(1, CHUNK as i64) as usize * CHANNELS]
            }
        };
        let frames = (out.len() / CHANNELS) as i64;
        self.cursor = Some(cursor + frames);

        if error > NUDGE_FRAMES && out.len() >= CHANNELS {
            // Спешим: повторяем последний кадр, чтобы потянуть время.
            let last = out[out.len() - CHANNELS..].to_vec();
            out.extend_from_slice(&last);
        } else if error < -NUDGE_FRAMES && out.len() > CHANNELS {
            // Опаздываем: выбрасываем кадр.
            out.truncate(out.len() - CHANNELS);
        }
        Some(out)
    }

    fn ensure_sink(&mut self, rate: u32) -> Option<()> {
        if self.sink.as_ref().is_some_and(|s| s.sample_rate() == rate) {
            return Some(());
        }
        self.sink = None;
        match audio::open(&self.cfg.device, rate, self.cfg.output_latency) {
            Ok(sink) => {
                info!(device = %self.cfg.device, rate, "вывод звука открыт");
                self.sink = Some(sink);
                Some(())
            }
            Err(e) => {
                warn!(device = %self.cfg.device, "вывод звука: {e}, играю в никуда");
                self.sink = Some(Box::new(audio::Null::new(rate)));
                Some(())
            }
        }
    }

    fn decode(&mut self, format: Format, ssrc: u32, payload: &[u8]) -> Vec<i16> {
        let frames = format.frames_per_packet() as usize;
        if self.decoder.as_ref().is_none_or(|(s, _)| *s != ssrc) {
            self.decoder = match Decoder::new(format) {
                Ok(d) => {
                    info!(?format, "декодер");
                    Some((ssrc, d))
                }
                Err(e) => {
                    warn!("{e}");
                    None
                }
            };
        }
        self.pcm.clear();
        if let Some((_, d)) = &mut self.decoder
            && let Err(e) = d.decode(payload, &mut self.pcm)
        {
            debug!("{e}");
            self.pcm.clear();
        }
        if self.pcm.is_empty() {
            self.pcm.resize(frames * CHANNELS, 0);
        }
        std::mem::take(&mut self.pcm)
    }
}
