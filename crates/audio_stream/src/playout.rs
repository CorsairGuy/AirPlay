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
//!
//! Экспериментально (только по явному включению) шкала меток времени
//! начинается заново только после серии пакетов с новой меткой. Одиночный
//! запоздавший пакет из прошлой эпохи не меняет шкалу и не сдвигает
//! `played_through`: он отбрасывается. При подтверждённой смене очередь
//! очищается, отметка о проигранном сбрасывается, якорь переставляется на
//! новую метку, и до следующего якоря отправителя вывод идёт по своим часам.
//! Без этого сброс метки заворачивался через 32-битную арифметику в «далёкое
//! будущее» и поток замирал навсегда.

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
/// Предел очереди пакетов (около полутора минут AAC 44,1 кГц): сессия на
/// паузе или ушедший вперёд отправитель не должны съедать память без предела.
const MAX_BUFFERED_PACKETS: usize = 4096;
/// Скачок метки больше этого числа секунд — не джиттер, а новая шкала
/// (перезапуск отправителя или потеря сессии).
pub const MAX_TS_JUMP_SECS: i64 = 10;
/// Сколько пакетов подряд должно подтвердить новую шкалу. Одиночный
/// запоздавший пакет из прошлой эпохи её не меняет.
const EPOCH_CONFIRM_PACKETS: usize = 3;

#[derive(Debug, Clone)]
pub struct Config {
    /// Устройство вывода: `default`, `null` или имя ALSA.
    pub device: String,
    /// Размер буфера звуковой карты.
    pub output_latency: Duration,
    /// Задержка, с которой играем, если часов отправителя нет (без PTP).
    pub fallback_latency: Duration,
    /// Свой вывод вместо `device`.
    pub sink: Option<audio::SinkFactory>,
    /// Сдвиг момента воспроизведения, нс: меньше нуля — играть раньше
    /// (компенсирует задержку после звуковой карты), больше — позже. Как
    /// `audio_backend_latency_offset_in_seconds` в shairport-sync.
    pub latency_offset_ns: i64,
    /// Тот же сдвиг, когда в сессии есть другие приёмники (мультирум);
    /// `None` — как `latency_offset_ns`.
    pub group_latency_offset_ns: Option<i64>,
    /// Experimental: infer an RTP restart from a sequence of timestamp jumps.
    /// Disabled by default: delayed packets can resemble a new timeline.
    pub allow_heuristic_rtp_recovery: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: "default".into(),
            output_latency: Duration::from_millis(100),
            fallback_latency: Duration::from_millis(250),
            sink: None,
            latency_offset_ns: 0,
            group_latency_offset_ns: None,
            allow_heuristic_rtp_recovery: false,
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

    /// Новая шкала: метка `ts` становится началом отсчёта.
    fn reset(&mut self, ts: u32) {
        self.last = Some((ts, ts as i64));
    }
}

/// Прыжок больше допустимого означает новую шкалу времени, а не джиттер.
fn is_discontinuity(delta_frames: i64, rate: i64) -> bool {
    delta_frames.abs() > rate * MAX_TS_JUMP_SECS
}

/// Пакеты, которые могут быть началом новой шкалы меток. До подтверждения
/// они не попадают в очередь: иначе одиночный запоздавший пакет звучал бы в
/// старой эпохе или занимал место под метки, которых не будет.
#[derive(Default)]
struct Pending {
    /// Метка первого пакета кандидата — от неё начнётся новая шкала.
    base: u32,
    /// Метка последнего принятого пакета кандидата.
    last: u32,
    packets: Vec<(u32, u32, Vec<u8>)>,
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
    volume_db: f32,
    /// Действующий сдвиг воспроизведения, нс.
    latency_offset_ns: i64,
    /// Растёт при перемотке и смене якоря: поток вывода начинает заново.
    epoch: u64,
    /// Самая поздняя метка, которую вывод уже прошёл (или выбросил).
    /// Пакет с более ранней меткой опоздал и не должен звучать заново.
    /// При подтверждённой смене шкалы сбрасывается: она относится к прежним
    /// меткам и иначе задержала бы весь новый поток.
    played_through: Option<i64>,
    /// Самая поздняя расширенная метка текущей шкалы. От неё считается
    /// расхождение с новым пакетом, поэтому кандидат на новую шкалу её не
    /// двигает: одиночный пакет не должен выглядеть как перезапуск.
    high_water: Option<i64>,
    /// Пакеты, которые могут быть началом новой шкалы.
    pending: Option<Pending>,
    /// Сколько раз шкала меток начиналась заново.
    resyncs: u64,
    allow_heuristic_rtp_recovery: bool,
    stopped: bool,
}

impl State {
    /// Кладёт пакет в очередь на текущей шкале.
    fn store(&mut self, timestamp: u32, ssrc: u32, payload: Vec<u8>) {
        let ext = self.unwrap.peek(timestamp);
        // Дубль или пакет, который вывод уже прошёл: первый вариант уже
        // прозвучал (или был выброшен), повторять его нельзя.
        if self.packets.contains_key(&ext)
            || self.played_through.is_some_and(|played| ext <= played)
        {
            debug!(ext, "пакет уже играл или повторён, отброшен");
            return;
        }
        let ext = self.unwrap.extend(timestamp);
        self.high_water = Some(self.high_water.map_or(ext, |hw| hw.max(ext)));
        self.packets.insert(ext, Packet { ssrc, payload });
        if self.packets.len() > MAX_BUFFERED_PACKETS {
            let excess = self.packets.len() - MAX_BUFFERED_PACKETS;
            let stale: Vec<i64> = self.packets.keys().take(excess).copied().collect();
            for ts in stale {
                self.packets.remove(&ts);
            }
            debug!(
                dropped = excess,
                "буфер переполнен, старые пакеты отброшены"
            );
        }
    }

    /// Пакет далеко от текущей шкалы. Одиночный такой пакет — опоздавший,
    /// шкалу меняет только серия из [`EPOCH_CONFIRM_PACKETS`] подряд.
    /// Возвращает `true`, если новая шкала подтверждена и очередь перестроена.
    fn consider_new_epoch(
        &mut self,
        timestamp: u32,
        ssrc: u32,
        payload: Vec<u8>,
        rate: i64,
    ) -> bool {
        if self.pending.as_ref().is_some_and(|p| p.last == timestamp && p.packets[0].1 == ssrc) {
            return false;
        }
        let continues = self.pending.as_ref().is_some_and(|p| {
            let step = rtp::ts_diff(timestamp, p.last) as i64;
            p.packets[0].1 == ssrc && (1..=rate).contains(&step)
        });
        if continues {
            if let Some(p) = self.pending.as_mut() {
                p.last = timestamp;
                p.packets.push((timestamp, ssrc, payload));
            }
        } else {
            self.pending = Some(Pending {
                base: timestamp,
                last: timestamp,
                packets: vec![(timestamp, ssrc, payload)],
            });
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.packets.len() < EPOCH_CONFIRM_PACKETS)
        {
            return false;
        }
        let Some(pending) = self.pending.take() else {
            return false;
        };
        info!(
            packets = pending.packets.len(),
            "разрыв меток времени: поток начинается заново"
        );
        self.packets.clear();
        self.played_through = None;
        self.unwrap.reset(pending.base);
        self.high_water = None;
        // Прежний якорь относится к старой шкале: до следующего якоря
        // отправителя играем по своим часам — путь realtime-потока без PTP.
        if self.anchor.is_some() {
            let playing = self.anchor.is_some_and(|a| a.playing);
            self.anchor = Some(Anchor {
                rtp: pending.last,
                network_ns: 0,
                timeline: 0,
                playing,
            });
            self.local_anchor = true;
            self.fallback_local_ns = now_ns() as i64;
        }
        self.epoch += 1;
        self.resyncs += 1;
        for (ts, ssrc, payload) in pending.packets {
            self.store(ts, ssrc, payload);
        }
        true
    }
}

pub struct Playout {
    shared: Arc<(Mutex<State>, Condvar)>,
    offsets: (i64, Option<i64>),
}

impl Playout {
    pub fn start(cfg: Config, time: Arc<dyn TimeSource>, format: Option<Format>) -> Playout {
        let shared = Arc::new((
            Mutex::new(State {
                format,
                latency_offset_ns: cfg.latency_offset_ns,
                played_through: None,
                allow_heuristic_rtp_recovery: cfg.allow_heuristic_rtp_recovery,
                ..State::default()
            }),
            Condvar::new(),
        ));
        let thread_shared = shared.clone();
        let offsets = (cfg.latency_offset_ns, cfg.group_latency_offset_ns);
        let spawned = std::thread::Builder::new()
            .name("playout".into())
            .spawn(move || Output::new(cfg, time, thread_shared).run());
        if let Err(e) = spawned {
            warn!("поток вывода не запущен: {e}");
        }
        Playout { shared, offsets }
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
        if s.stopped {
            // Сессия закрыта, а подключение ещё живёт: не копим пакеты в никуда.
            return;
        }
        let rate = s
            .format
            .or_else(|| Format::from_ssrc(ssrc))
            .map_or(44_100, |f| f.sample_rate.max(1)) as i64;
        let ext = s.unwrap.peek(timestamp);
        if s.allow_heuristic_rtp_recovery && s.high_water
            .is_some_and(|hw| is_discontinuity(ext - hw, rate))
        {
            if !s.consider_new_epoch(timestamp, ssrc, payload, rate) {
                // Кандидат ещё не подтверждён: пакет ждёт в нём, а не в очереди.
                return;
            }
        } else {
            // Поток идёт на прежней шкале: кандидат был одиночным пакетом.
            if let Some(held) = s.pending.take() {
                debug!(
                    held = held.packets.len(),
                    "кандидат на новую шкалу отброшен"
                );
            }
            s.store(timestamp, ssrc, payload);
        }
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
            // 0 пришёл бы из испорченного SETUP; дальше делим на это число.
            .map_or(44_100, |f| f.sample_rate.max(1)) as i64;
        let jump = match s.anchor {
            None => true,
            // Переход с собственного якоря на часы отправителя меняет систему
            // отсчёта: продолжать прежний курсор нельзя.
            Some(_) if s.local_anchor => true,
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
        // Отправитель переопределил поток: неподтверждённый кандидат к нему
        // уже не относится.
        s.pending = None;
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

    /// Classic RAOP FLUSH discards packets before the specified RTP boundary.
    /// Keep a watermark so delayed packets from before it cannot be replayed.
    pub fn flush_classic(&self, boundary: u32) {
        if boundary == 0 {
            self.clear();
            return;
        }
        let mut s = self.lock();
        // Отправитель объявил поток заново: неподтверждённый кандидат набран
        // из пакетов до границы и к новому потоку не относится.
        s.pending = None;
        let until = s.unwrap.peek(boundary);
        s.packets.retain(|&ts, _| ts >= until);
        let cutoff = until - 1;
        s.played_through = Some(s.played_through.map_or(cutoff, |old| old.max(cutoff)));
        s.epoch += 1;
        drop(s);
        self.changed();
    }

    /// Classic FLUSH: отправитель объявил весь буфер недействительным
    /// (перемотка, смена трека).
    pub fn clear(&self) {
        let mut s = self.lock();
        s.pending = None;
        let dropped = s.packets.len();
        s.packets.clear();
        s.epoch += 1;
        debug!(dropped, "FLUSH: буфер очищен");
        drop(s);
        self.changed();
    }

    pub fn set_volume(&self, db: f32) {
        self.lock().volume_db = db;
    }

    /// В сессии есть другие приёмники: сдвиг для мультирума.
    pub fn set_grouped(&self, grouped: bool) {
        let (solo, group) = self.offsets;
        let offset = if grouped { group.unwrap_or(solo) } else { solo };
        let mut s = self.lock();
        if s.latency_offset_ns != offset {
            info!(
                offset_ms = offset / 1_000_000,
                grouped, "сдвиг воспроизведения"
            );
            s.latency_offset_ns = offset;
        }
    }

    pub fn stop(&self) {
        let mut s = self.lock();
        s.stopped = true;
        s.packets.clear();
        s.pending = None;
        drop(s);
        self.changed();
    }

    pub fn buffered_packets(&self) -> usize {
        self.lock().packets.len()
    }

    /// Сколько раз шкала меток времени начиналась заново (перезапуск
    /// отправителя). Штатный FLUSH меняет эпоху, но не метку, и счётчик не
    /// увеличивает.
    pub fn resync_count(&self) -> u64 {
        self.lock().resyncs
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
    /// Громкость, уже переданная выводу, и применил ли он её сам.
    applied_volume: Option<(f32, bool)>,
    /// SSRC, для которого формат не определился: чтобы не писать в лог на
    /// каждый пакет и не замирать на нём.
    unknown_ssrc: Option<u32>,
    /// SSRC, для которого декодер уже не открылся.
    decoder_failed: Option<u32>,
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
            applied_volume: None,
            unknown_ssrc: None,
            decoder_failed: None,
        }
    }

    fn run(mut self) {
        loop {
            let shared = self.shared.clone();
            let (lock, cvar) = &*shared;
            let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if s.stopped {
                    if let Some(sink) = &mut self.sink {
                        sink.stop();
                    }
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
            let gain = self.gain(s.volume_db);
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
        let first_ssrc = first.ssrc;
        let Some(format) = s.format.or_else(|| Format::from_ssrc(first_ssrc)) else {
            // SSRC не несёт формат: такой пакет не сыграть. Раньше поток
            // замирал на нём навсегда, без единой строки в логе.
            if self.unknown_ssrc != Some(first_ssrc) {
                warn!(
                    ssrc = first_ssrc,
                    "неизвестный формат потока, пакет отброшен"
                );
                self.unknown_ssrc = Some(first_ssrc);
            }
            s.packets.remove(&first_ts);
            s.played_through = Some(first_ts);
            return Some(Vec::new());
        };
        self.unknown_ssrc = None;
        let rate = format.sample_rate.max(1);
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
        } + s.latency_offset_ns;
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

        // Старое уже не сыграть. Ноль не должен зациклить курсор.
        let frames_per_packet = format.frames_per_packet().max(1) as i64;
        let cursor = self.cursor.unwrap_or(due);
        while let Some((&ts, _)) = s.packets.iter().next() {
            if ts + frames_per_packet > cursor {
                break;
            }
            s.packets.remove(&ts);
            s.played_through = Some(ts);
        }
        if s.played_through.is_none_or(|played| played < cursor) {
            s.played_through = Some(cursor - 1);
        }

        let mut out = match s.packets.range(..=cursor).next_back() {
            Some((&ts, p)) if ts + frames_per_packet > cursor => {
                let ssrc = p.ssrc;
                let payload = std::mem::take(&mut s.packets.get_mut(&ts)?.payload);
                s.packets.remove(&ts);
                s.played_through = Some(ts);
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

    /// Множитель для сэмплов: единица, если громкость взял на себя вывод.
    fn gain(&mut self, db: f32) -> i32 {
        let hardware = match self.applied_volume {
            Some((applied, hw)) if applied == db => hw,
            _ => {
                let hw = self.sink.as_mut().is_some_and(|s| s.set_volume(db));
                self.applied_volume = Some((db, hw));
                hw
            }
        };
        if hardware { 32768 } else { audio::gain_q15(db) }
    }

    fn ensure_sink(&mut self, rate: u32) -> Option<()> {
        if self.sink.as_ref().is_some_and(|s| s.sample_rate() == rate) {
            return Some(());
        }
        self.sink = None;
        let opened = match &self.cfg.sink {
            Some(factory) => factory.open(rate, self.cfg.output_latency),
            None => audio::open(&self.cfg.device, rate, self.cfg.output_latency),
        };
        self.applied_volume = None;
        match opened {
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
        let frames = format.frames_per_packet().max(1) as usize;
        if self.decoder.as_ref().is_none_or(|(s, _)| *s != ssrc) {
            if self.decoder_failed == Some(ssrc) {
                // Декодер для этого потока уже не открылся: не повторяем
                // попытку и не пишем в лог на каждом пакете.
                self.pcm.resize(frames * CHANNELS, 0);
                return std::mem::take(&mut self.pcm);
            }
            self.decoder = match Decoder::new(format) {
                Ok(d) => {
                    info!(?format, "декодер");
                    self.decoder_failed = None;
                    Some((ssrc, d))
                }
                Err(e) => {
                    warn!(?format, "декодер не открыт: {e}");
                    self.decoder_failed = Some(ssrc);
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

#[cfg(test)]
mod s13_tests {
    use super::*;

    fn playout() -> Playout {
        Playout::start(
            Config {
                device: "null".into(),
                ..Default::default()
            },
            Arc::new(clock::NoTime),
            None,
        )
    }

    #[test]
    fn duplicate_packet_keeps_the_first_payload() {
        let p = playout();
        p.push(1000, 0x1600_0000, vec![1, 2, 3]);
        p.push(1000, 0x1600_0000, vec![9, 9, 9]);
        assert_eq!(p.buffered_packets(), 1);
        let s = p.lock();
        let packet = s.packets.values().next().unwrap();
        assert_eq!(packet.payload, [1, 2, 3]);
    }

    #[test]
    fn reordered_packets_are_stored_in_timestamp_order() {
        let p = playout();
        for ts in [3000u32, 1000, 2000] {
            p.push(ts, 0x1600_0000, vec![ts as u8]);
        }
        let s = p.lock();
        let order: Vec<i64> = s.packets.keys().copied().collect();
        assert_eq!(order, [1000, 2000, 3000]);
        drop(s);
        assert_eq!(p.buffered_packets(), 3);
    }

    #[test]
    fn late_packet_after_playhead_is_not_requeued() {
        let p = playout();
        p.lock().played_through = Some(2000);
        p.push(1000, 0x1600_0000, vec![1]);
        p.push(2000, 0x1600_0000, vec![2]);
        assert_eq!(p.buffered_packets(), 0);
        p.push(2001, 0x1600_0000, vec![3]);
        assert_eq!(p.buffered_packets(), 1);
    }

    #[test]
    fn queue_stays_bounded_when_sender_runs_ahead() {
        let p = playout();
        for ts in 0..(MAX_BUFFERED_PACKETS as u32 + 16) {
            p.push(ts * 1024, 0x1600_0000, vec![1]);
        }
        assert_eq!(p.buffered_packets(), MAX_BUFFERED_PACKETS);
        let s = p.lock();
        let first = *s.packets.keys().next().unwrap();
        assert!(first > 0, "старые пакеты должны быть выброшены");
    }

    #[test]
    fn classic_flush_keeps_boundary_and_rejects_delayed_earlier_packets() {
        let p = playout();
        for ts in [1000, 2000, 3000] {
            p.push(ts, 0x1600_0000, vec![1]);
        }
        p.flush_classic(2000);
        p.push(1000, 0x1600_0000, vec![2]);
        let state = p.lock();
        assert_eq!(
            state.packets.keys().copied().collect::<Vec<_>>(),
            vec![2000, 3000]
        );
        assert_eq!(state.played_through, Some(1999));
    }

    #[test]
    fn classic_flush_boundary_unwraps_across_rtp_rollover() {
        let p = playout();
        for ts in [u32::MAX - 10, 5, 20] {
            p.push(ts, 0x1600_0000, vec![1]);
        }
        p.flush_classic(5);
        p.push(u32::MAX - 10, 0x1600_0000, vec![2]);
        let state = p.lock();
        assert_eq!(
            state.packets.keys().copied().collect::<Vec<_>>(),
            vec![u32::MAX as i64 + 6, u32::MAX as i64 + 21]
        );
    }
}

#[cfg(test)]
mod epoch_tests {
    use super::*;

    impl Playout {
        /// Плейаут без потока вывода: проверяем только состояние очереди.
        fn inert() -> Self {
            let shared = Arc::new((Mutex::new(State { allow_heuristic_rtp_recovery: true, ..State::default() }), Condvar::new()));
            Self {
                shared,
                offsets: (0, None),
            }
        }
    }

    const AAC_44100: u32 = 0x1600_0000;

    fn keys(p: &Playout) -> Vec<i64> {
        p.lock().packets.keys().copied().collect()
    }

    #[test]
    fn discontinuity_threshold_is_ten_seconds() {
        let rate = 44_100;
        assert!(!is_discontinuity(rate * 9, rate));
        assert!(!is_discontinuity(0, rate));
        assert!(!is_discontinuity(-rate * 9, rate));
        assert!(is_discontinuity(rate * 10 + 1, rate));
        assert!(is_discontinuity(-rate * 10 - 1, rate));
    }

    #[test]
    fn single_late_packet_does_not_start_a_new_epoch() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.push(1_000_000_352, AAC_44100, vec![2]);
        let before = keys(&p);
        // Один запоздавший пакет из прошлой эпохи.
        p.push(0, AAC_44100, vec![9]);
        assert_eq!(p.resync_count(), 0);
        assert_eq!(keys(&p), before, "пакет не попал в очередь");
        // Поток продолжается на прежней шкале.
        p.push(1_000_000_704, AAC_44100, vec![3]);
        assert_eq!(p.resync_count(), 0);
        assert_eq!(keys(&p), vec![1_000_000_000, 1_000_000_352, 1_000_000_704]);
        assert!(p.lock().pending.is_none());
    }

    #[test]
    fn single_forward_jump_does_not_start_a_new_epoch() {
        let p = Playout::inert();
        p.push(352_000, AAC_44100, vec![1]);
        // Один пакет на 30 секунд вперёд.
        p.push(352_000 + 44_100 * 30, AAC_44100, vec![2]);
        assert_eq!(p.resync_count(), 0);
        p.push(352_000 + 352, AAC_44100, vec![3]);
        assert_eq!(p.resync_count(), 0);
        assert_eq!(keys(&p), vec![352_000, 352_352], "пакет вперёд отброшен");
    }

    #[test]
    fn duplicate_candidates_do_not_confirm_a_new_epoch() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        for _ in 0..8 { p.push(0, AAC_44100, vec![2]); }
        assert_eq!(p.resync_count(), 0);
        assert_eq!(keys(&p), vec![1_000_000_000]);
        p.push(1024, AAC_44100, vec![3]);
        assert_eq!(p.resync_count(), 0);
        p.push(2048, AAC_44100, vec![4]);
        assert_eq!(p.resync_count(), 1);
    }

    #[test]
    fn heuristic_recovery_is_disabled_by_default() {
        assert!(!Config::default().allow_heuristic_rtp_recovery);
        let p = Playout::inert();
        p.lock().allow_heuristic_rtp_recovery = false;
        p.push(1_000_000_000, AAC_44100, vec![1]);
        for ts in [0, 1024, 2048] { p.push(ts, AAC_44100, vec![2]); }
        assert_eq!(p.resync_count(), 0);
    }

    #[test]
    fn packets_from_different_sources_do_not_form_one_candidate() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.push(0, AAC_44100, vec![2]);
        p.push(1024, AAC_44100 + 1, vec![3]);
        p.push(2048, AAC_44100, vec![4]);
        assert_eq!(p.resync_count(), 0);
        assert_eq!(keys(&p), vec![1_000_000_000]);
    }

    #[test]
    fn two_packets_are_not_enough_to_rebase() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.push(0, AAC_44100, vec![2]);
        p.push(1024, AAC_44100, vec![3]);
        assert_eq!(p.resync_count(), 0);
        // Поток вернулся на прежнюю шкалу: кандидат отброшен.
        p.push(1_000_000_352, AAC_44100, vec![4]);
        assert_eq!(p.resync_count(), 0);
        assert_eq!(keys(&p), vec![1_000_000_000, 1_000_000_352]);
    }

    #[test]
    fn confirmed_restart_rebases_the_queue() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.push(1_000_000_352, AAC_44100, vec![2]);
        for (i, ts) in [0u32, 1024, 2048].into_iter().enumerate() {
            p.push(ts, AAC_44100, vec![10 + i as u8]);
        }
        assert_eq!(p.resync_count(), 1);
        assert_eq!(keys(&p), vec![0, 1024, 2048], "новая шкала от нуля");
        let s = p.lock();
        let payloads: Vec<u8> = s.packets.values().map(|p| p.payload[0]).collect();
        assert_eq!(payloads, vec![10, 11, 12], "пакеты кандидата сохранены");
        assert_eq!(s.high_water, Some(2048));
        assert_eq!(s.played_through, None, "старая отметка не действует");
    }

    #[test]
    fn rebase_keeps_playing_and_reanchors() {
        let p = Playout::inert();
        p.play_from(1_000_000_000);
        p.push(1_000_000_000, AAC_44100, vec![1]);
        for ts in [0u32, 1024, 2048] {
            p.push(ts, AAC_44100, vec![2]);
        }
        let s = p.lock();
        assert_eq!(s.resyncs, 1);
        assert!(s.local_anchor, "до нового якоря — свои часы");
        let anchor = s.anchor.as_ref().expect("якорь есть");
        assert!(anchor.playing, "играть продолжаем");
        assert_eq!(anchor.rtp, 2048);
    }

    #[test]
    fn old_played_through_does_not_block_the_new_scale() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.lock().played_through = Some(1_000_000_000);
        for ts in [0u32, 1024, 2048] {
            p.push(ts, AAC_44100, vec![2]);
        }
        assert_eq!(p.resync_count(), 1);
        assert_eq!(p.buffered_packets(), 3);
        assert_eq!(keys(&p), vec![0, 1024, 2048]);
    }

    #[test]
    fn classic_flush_drops_a_pending_candidate() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.push(0, AAC_44100, vec![2]);
        assert!(p.lock().pending.is_some(), "кандидат набран");
        p.flush_classic(1_000_000_000);
        assert!(
            p.lock().pending.is_none(),
            "граница FLUSH снимает кандидата"
        );
        assert_eq!(p.resync_count(), 0);
    }

    // Перенесено из `feat/epoch-recovery@bfeb487`. `Playout::rebase` в этой
    // ветке удалён: объявленная отправителем граница (`FLUSH` + `RTP-Info`)
    // приходит в `flush_classic`, и она сбрасывает очередь сразу, без серии
    // пакетов. Граница — это позиция в потоке, а не новая шкала меток,
    // поэтому счётчик перезапусков не растёт, а якорь не переставляется.
    // Сборка этих тестов не проверялась: `cargo`/`rustc` в окружении автора нет.

    /// Объявленная граница выбрасывает очередь немедленно: серия из трёх
    /// пакетов для этого не нужна, и первый же пакет за границей звучит.
    #[test]
    fn declared_boundary_flushes_without_waiting_for_a_series() {
        let p = Playout::inert();
        p.play_from(1_000_000_000);
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.push(1_000_000_352, AAC_44100, vec![2]);
        p.flush_classic(1_000_000_352);
        assert_eq!(keys(&p), vec![1_000_000_352], "всё до границы брошено");
        assert_eq!(p.resync_count(), 0, "граница не меняет шкалу");
        assert_eq!(
            p.lock().played_through,
            Some(1_000_000_351),
            "граница становится водяным знаком"
        );
        {
            let s = p.lock();
            let anchor = s.anchor.as_ref().expect("якорь есть");
            assert!(anchor.playing, "вывод продолжается");
            assert_eq!(anchor.rtp, 1_000_000_000, "якорь не переставляется");
        }
        // Первый пакет новой части потока попадает в очередь без кандидата.
        p.push(1_000_000_704, AAC_44100, vec![3]);
        assert_eq!(keys(&p), vec![1_000_000_352, 1_000_000_704]);
        assert!(p.lock().pending.is_none(), "серия не набиралась");
    }

    /// Запоздавший пакет из выброшенной части потока не возвращается: граница
    /// работает и после сброса, а не только в момент объявления.
    #[test]
    fn packets_from_before_the_declared_boundary_do_not_return() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.push(1_000_000_352, AAC_44100, vec![2]);
        p.flush_classic(1_000_000_352);
        p.push(1_000_000_000, AAC_44100, vec![3]);
        assert_eq!(keys(&p), vec![1_000_000_352], "пакет до границы отброшен");
        assert_eq!(p.resync_count(), 0);
        assert!(
            p.lock().pending.is_none(),
            "кандидат на новую шкалу не набран"
        );
        p.push(1_000_000_704, AAC_44100, vec![4]);
        assert_eq!(keys(&p), vec![1_000_000_352, 1_000_000_704]);
    }

    /// Две шкалы вперемешку не дают серии: каждая пара «пакет чужой шкалы +
    /// пакет действующей» начинается заново, поэтому перезапуск не
    /// подтверждается, а действующая шкала не сдвигается.
    #[test]
    fn alternating_scales_never_confirm_a_new_epoch() {
        let p = Playout::inert();
        p.push(500_000_000, AAC_44100, vec![0]);
        for round in 0..8u32 {
            p.push(round, AAC_44100, vec![1]);
            p.push(500_000_000 + round * 352, AAC_44100, vec![2]);
        }
        assert_eq!(p.resync_count(), 0);
        let scale: Vec<i64> = (0..=7).map(|r| 500_000_000 + r * 352).collect();
        assert_eq!(keys(&p), scale, "прежняя шкала не тронута");
    }

    /// Долгая сессия с потерями и повторами остаётся на одной шкале: счётчик
    /// перезапусков не растёт, `played_through` сам не выставляется, очередь
    /// держится в пределе.
    #[test]
    fn long_session_keeps_one_scale_through_losses_and_reordering() {
        let p = Playout::inert();
        // Около трёх минут 44,1 кГц: потери и перестановки внутри окна.
        let mut last = 0u32;
        for i in 0..20_000u32 {
            let ts = i.wrapping_mul(352);
            if i % 97 == 0 {
                continue; // потеря
            }
            if i % 53 == 0 && i > 0 {
                p.push(last, AAC_44100, vec![1]); // повтор
            }
            p.push(ts, AAC_44100, vec![2]);
            last = ts;
        }
        assert_eq!(p.resync_count(), 0, "без объявления шкала не менялась");
        let s = p.lock();
        assert_eq!(s.packets.len(), MAX_BUFFERED_PACKETS);
        assert_eq!(s.high_water, Some(19_999 * 352));
        assert!(
            s.played_through.is_none(),
            "без вывода отметка не выставляется сама"
        );
    }

    #[test]
    fn flush_drops_a_pending_candidate() {
        let p = Playout::inert();
        p.push(1_000_000_000, AAC_44100, vec![1]);
        p.push(0, AAC_44100, vec![2]);
        assert!(p.lock().pending.is_some());
        p.flush(None, 1_000_000_000);
        assert!(p.lock().pending.is_none());
        assert_eq!(p.resync_count(), 0);
    }

    #[test]
    fn wrap_around_is_not_a_discontinuity() {
        let p = Playout::inert();
        p.push(u32::MAX - 100, AAC_44100, vec![1]);
        p.push(252, AAC_44100, vec![2]);
        assert_eq!(p.resync_count(), 0);
        assert_eq!(p.buffered_packets(), 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clock::NoTime;
    use codec::Codec;

    fn playout(format: Format) -> Playout {
        Playout::start(
            Config {
                device: "null".into(),
                ..Default::default()
            },
            Arc::new(NoTime),
            Some(format),
        )
    }

    /// Частота 0 из испорченного SETUP раньше делила на ноль при втором
    /// якоре и убивала поток вывода; теперь пересчёт не падает.
    #[test]
    fn zero_sample_rate_does_not_panic_on_repeated_anchors() {
        let playout = playout(Format {
            codec: Codec::Pcm {
                frames_per_packet: 352,
            },
            sample_rate: 0,
            channels: 2,
        });
        for rtp in [0u32, 352, 704] {
            playout.set_anchor(Anchor {
                rtp,
                network_ns: rtp as u64 * 1_000_000,
                timeline: 7,
                playing: true,
            });
        }
        playout.stop();
    }

    /// Ложной смены эпохи нет: джиттер часов и согласованный якорь вывод не
    /// перезапускают, а пауза, скачок шкалы и перемотка — перезапускают.
    #[test]
    fn only_real_discontinuities_start_a_new_epoch() {
        let p = playout(Format {
            codec: Codec::Pcm {
                frames_per_packet: 1024,
            },
            sample_rate: 44_100,
            channels: 2,
        });
        let anchor = |rtp: u32, network_ns: u64, playing: bool| Anchor {
            rtp,
            network_ns,
            timeline: 4,
            playing,
        };
        p.set_anchor(anchor(0, 1_000_000_000, true));
        let start = p.lock().epoch;
        p.set_anchor(anchor(0, 1_020_000_000, true));
        assert_eq!(p.lock().epoch, start, "джиттер сети сменил эпоху");
        p.set_anchor(anchor(44_100, 2_000_000_000, true));
        assert_eq!(p.lock().epoch, start, "согласованный якорь сменил эпоху");
        p.set_anchor(anchor(44_100, 2_000_000_000, false));
        assert_eq!(p.lock().epoch, start + 1, "пауза не сменила эпоху");
        p.set_anchor(anchor(88_200, 5_000_000_000, false));
        assert_eq!(p.lock().epoch, start + 2, "скачок шкалы не сменил эпоху");
        p.flush(None, 88_200);
        assert_eq!(p.lock().epoch, start + 3, "перемотка не сменила эпоху");
        p.stop();
    }

    /// Нулевой `frames_per_packet` не должен зацикливать очередь.
    #[test]
    fn zero_frames_per_packet_still_stops() {
        let playout = playout(Format {
            codec: Codec::Pcm {
                frames_per_packet: 0,
            },
            sample_rate: 44_100,
            channels: 2,
        });
        playout.push(0, 0x1600_0000, vec![0; 4096]);
        playout.play_from(0);
        playout.stop();
        assert!(playout.buffered_packets() <= 1);
    }
}
