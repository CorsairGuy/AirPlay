//! Медиачасть RTSP-сессии AirPlay 2: FairPlay, SETUP потоков, якорь
//! времени, перемотка, громкость, канал событий.
//!
//! Последовательность — по shairplay-rust и shairport-sync: первый SETUP
//! без `streams` (часы и канал событий), затем SETUP потока, RECORD и
//! SETRATEANCHORTIME, когда пора играть.

pub use audio_stream;
#[cfg(feature = "mirroring")]
pub use mirror;

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use audio_stream::{Stream, StreamKey};
use clock::{Anchor, TimeSource};
use codec::{Codec, Format};
use crypto::Channel;
use crypto::kdf::ControlKeys;
use fairplay::FairPlay;
use plist::{Dictionary, Value};
use rtsp::{Request, Response};
use timesync::TimeSync;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::control::{self, BPLIST, Command, MrpService, OCTETS, Remote};
use crate::events::{Events, Metadata, NoEvents, Progress};
#[cfg(feature = "hls")]
use crate::hls::HlsOptions;
use crate::raop::Raop;

/// Сколько байт отправитель может прислать вперёд в буферизованном потоке:
/// 1 МиБ — около 30 с AAC 256 кбит/с.
const AUDIO_BUFFER_SIZE: u64 = 1 << 20;

#[cfg(feature = "mirroring")]
fn video_stream_id(s: &Dictionary, authenticated: bool) -> Result<u64, String> {
    let id = s.get("streamConnectionID").and_then(|v| {
        v.as_unsigned_integer()
            .or(v.as_signed_integer().map(|i| i as u64))
    });
    if authenticated {
        id.ok_or_else(|| "authenticated video requires an integer streamConnectionID".into())
    } else {
        Ok(id.unwrap_or(0))
    }
}

/// Группа сессии отправителя: `gid` в Bonjour, `/info` и карточке.
///
/// Медиачасть только сообщает о начале и конце сессии; кто обновляет
/// объявление и карточку, решает хозяин сессии.
pub trait SessionGroup: Send + Sync {
    fn set_group(&self, owner: u64, group: Option<String>);
}

/// Что медиачасти нужно от сессии: звук, вывод и пульт.
pub struct MediaContext {
    pub player: Arc<Player>,
    /// Куда отдавать видео зеркалирования.
    #[cfg(feature = "mirroring")]
    pub mirror: Option<mirror::VideoSinkFactory>,
    /// URL-cast output; only configured by hosts with a video renderer.
    #[cfg(feature = "hls")]
    pub hls: Option<HlsOptions>,
    /// Команды отправителю через канал событий.
    pub remote: Remote,
    /// Кто разбирает канал потока 130; `None` — только регистрация.
    pub mrp: Option<MrpService>,
    /// `ID` нашего PTP-участника в ответе SETUP.
    pub pairing_id: String,
    /// Сессия отправителя началась или закончилась.
    pub session_group: Arc<dyn SessionGroup>,
}

/// Что делать, когда звук уже занят другим отправителем.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Ownership {
    /// Новый отправитель останавливает прежний поток (по умолчанию).
    #[default]
    Takeover,
    /// Пока играет один отправитель, второй получает отказ.
    Reject,
}

/// Звук играет один отправитель: по политике [`Player::ownership`] новый
/// поток вытесняет прежний или отклоняется.
pub struct Player {
    pub audio: audio_stream::Config,
    pub time: TimeSync,
    /// Шкала громкости: ползунок iPhone (-30..0 дБ) растягивается на
    /// столько дБ (как `volume_range_db` в shairport-sync).
    pub volume_range_db: f32,
    /// Громкость не применять к звуку, только сообщать в [`Events`]: её
    /// ведёт внешний микшер.
    pub apply_volume: bool,
    /// Политика для второго отправителя; см. [`Ownership`].
    pub ownership: Ownership,
    pub events: Arc<dyn Events>,
    active: Mutex<Option<(u64, Stream)>>,
    /// Громкость AirPlay, как прислал отправитель.
    volume_db: Mutex<f32>,
    next_id: AtomicU64,
    reported_owner: Arc<Mutex<Option<u64>>>,
}

impl Player {
    pub fn new(audio: audio_stream::Config, time: TimeSync) -> Self {
        Self {
            audio,
            time,
            volume_range_db: 30.0,
            apply_volume: true,
            ownership: Ownership::Takeover,
            events: Arc::new(NoEvents),
            active: Mutex::new(None),
            volume_db: Mutex::new(0.0),
            next_id: AtomicU64::new(1),
            reported_owner: Arc::new(Mutex::new(None)),
        }
    }

    /// Громкость AirPlay в усиление вывода, дБ.
    fn output_db(&self, airplay_db: f32) -> f32 {
        if !self.apply_volume {
            0.0
        } else if airplay_db <= -100.0 {
            -144.0
        } else {
            airplay_db.clamp(-30.0, 0.0) * self.volume_range_db / 30.0
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<(u64, Stream)>> {
        self.active.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn time_source(&self) -> Arc<dyn TimeSource> {
        Arc::new(self.time.clone())
    }

    /// Может ли этот владелец получить звук при текущей политике.
    fn available(&self, owner: u64) -> bool {
        self.ownership == Ownership::Takeover
            || self.lock().as_ref().is_none_or(|(id, _)| *id == owner)
    }

    /// Ставит поток владельцу. `false` — политика [`Ownership::Reject`]
    /// отказала: поток не занял плеер и освобождён.
    fn replace(&self, owner: u64, mut stream: Stream) -> bool {
        let mut active = self.lock();
        if self.ownership == Ownership::Reject
            && active.as_ref().is_some_and(|(id, _)| *id != owner)
        {
            warn!(owner, "второй отправитель отклонён политикой владения");
            return false;
        }
        stream
            .playout()
            .set_volume(self.output_db(*self.volume_db.lock().unwrap_or_else(|e| e.into_inner())));
        let events = self.events.clone();
        let reported = self.reported_owner.clone();
        {
            let mut current = reported.lock().unwrap_or_else(|e| e.into_inner());
            if current.is_some_and(|id| id != owner) {
                *current = None;
            }
        }
        stream.on_realtime_started(move || {
            if *reported.lock().unwrap_or_else(|e| e.into_inner()) != Some(owner) {
                events.playing(true);
            }
        });
        if let Some((old, _)) = active.replace((owner, stream))
            && old != owner
        {
            info!("новый отправитель, прежний поток остановлен");
        }
        true
    }

    fn with<R>(&self, owner: u64, f: impl FnOnce(&Stream) -> R) -> Option<R> {
        match &*self.lock() {
            Some((id, stream)) if *id == owner => Some(f(stream)),
            _ => None,
        }
    }

    fn release(&self, owner: u64) {
        let mut active = self.lock();
        if active.as_ref().is_some_and(|(id, _)| *id == owner) {
            let realtime = active
                .as_ref()
                .is_some_and(|(_, stream)| stream.kind == audio_stream::StreamKind::Realtime);
            *active = None;
            drop(active);
            if realtime
                && *self
                    .reported_owner
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    != Some(owner)
            {
                self.events.playing(false);
            }
        }
    }

    fn report_state(&self, owner: u64, playing: bool) {
        *self
            .reported_owner
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(owner);
        self.events.playing(playing);
    }

    fn forget_reported_state(&self, owner: u64) {
        let mut reported = self
            .reported_owner
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *reported == Some(owner) {
            *reported = None;
            drop(reported);
            self.events.playing(false);
        }
    }

    fn set_volume(&self, owner: u64, db: f32) {
        *self.volume_db.lock().unwrap_or_else(|e| e.into_inner()) = db;
        let out = self.output_db(db);
        self.with(owner, |s| s.playout().set_volume(out));
    }

    #[doc(hidden)]
    pub fn active_packets(&self) -> Option<usize> {
        self.lock()
            .as_ref()
            .map(|(_, s)| s.playout().buffered_packets())
    }
}

pub struct Media {
    player: Arc<Player>,
    id: u64,
    /// Наш адрес в этом соединении: его отдаём как адрес PTP.
    local: IpAddr,
    /// Общий секрет сопряжения, из него — ключи канала событий.
    shared: Option<Vec<u8>>,
    fairplay: FairPlay,
    events: Option<JoinHandle<()>>,
    anchor: Option<Anchor>,
    /// Адрес отправителя: туда шлём запросы времени NTP.
    remote: IpAddr,
    /// Куда отдавать видео; `None` — зеркалирование выключено.
    #[cfg(feature = "mirroring")]
    mirror: Option<mirror::VideoSinkFactory>,
    /// Ключ AES из FairPlay `ekey` и IV `eiv` (зеркалирование).
    #[cfg(feature = "mirroring")]
    aes_key: Option<[u8; 16]>,
    #[cfg(feature = "mirroring")]
    eiv: Option<[u8; 16]>,
    /// Часы отправителя по NTP (зеркалирование или realtime audio).
    ntp: Option<timesync::ntp::NtpClock>,
    /// Общий секрет legacy pair-verify: подмешивается в ключ видео.
    #[cfg(feature = "mirroring")]
    legacy_shared: Option<[u8; 32]>,
    /// Часы NTP, видеопоток и заглушка звука зеркалирования.
    tasks: Vec<JoinHandle<()>>,
    /// Команды отправителю идут через канал событий этого соединения.
    control: Remote,
    /// Соединение только для пульта (`isRemoteControlOnly`): звука в нём
    /// нет, команды через него не шлём.
    rc_only: bool,
    /// Куда пересылать канал MRP (поток 130).
    mrp_service: Option<MrpService>,
    mrp: Option<JoinHandle<()>>,
    /// Сессия отправителя: `gid` в объявлении и карточке.
    group: Arc<dyn SessionGroup>,
    pending_group: Option<String>,
    /// `ID` нашего PTP-участника в ответе SETUP.
    pairing_id: String,
    /// Адреса отправителя из `timingPeerInfo`.
    sender_addrs: Vec<IpAddr>,
    /// В сессии есть другие приёмники (мультирум).
    grouped: bool,
    #[cfg(feature = "hls")]
    hls: Option<HlsOptions>,
    #[cfg(feature = "hls")]
    hls_playback: Option<Arc<hls::Playback>>,
    /// Playlist and start position waiting for `/rate` in the AirPlay 2 flow.
    #[cfg(feature = "hls")]
    pending_hls: Option<(hls::Url, Duration)>,
    #[cfg(feature = "hls")]
    cast_v2: bool,
    #[cfg(feature = "hls")]
    hls_v2_setup: bool,
    #[cfg(feature = "hls")]
    hls_v2_recorded: bool,
    /// Classic AirPlay 1: формат из ANNOUNCE и UDP-порты SETUP.
    raop: Raop,
    now_playing_duration: Mutex<Option<Duration>>,
}

impl Media {
    pub fn new(ctx: &MediaContext, local: IpAddr, remote: IpAddr) -> Self {
        Self {
            player: ctx.player.clone(),
            id: ctx.player.next_id.fetch_add(1, Ordering::Relaxed),
            local: local.to_canonical(),
            shared: None,
            fairplay: FairPlay::new(),
            events: None,
            anchor: None,
            remote: remote.to_canonical(),
            #[cfg(feature = "mirroring")]
            mirror: ctx.mirror.clone(),
            #[cfg(feature = "mirroring")]
            aes_key: None,
            #[cfg(feature = "mirroring")]
            eiv: None,
            ntp: None,
            #[cfg(feature = "mirroring")]
            legacy_shared: None,
            tasks: Vec::new(),
            control: ctx.remote.clone(),
            rc_only: false,
            mrp_service: ctx.mrp.clone(),
            mrp: None,
            group: ctx.session_group.clone(),
            pending_group: None,
            pairing_id: ctx.pairing_id.clone(),
            sender_addrs: vec![remote.to_canonical()],
            grouped: false,
            #[cfg(feature = "hls")]
            hls: ctx.hls.clone(),
            #[cfg(feature = "hls")]
            hls_playback: None,
            #[cfg(feature = "hls")]
            pending_hls: None,
            #[cfg(feature = "hls")]
            cast_v2: false,
            #[cfg(feature = "hls")]
            hls_v2_setup: false,
            #[cfg(feature = "hls")]
            hls_v2_recorded: false,
            raop: Raop::default(),
            now_playing_duration: Mutex::new(None),
        }
    }

    #[cfg(feature = "mirroring")]
    pub fn set_legacy_shared(&mut self, shared: [u8; 32]) {
        self.legacy_shared = Some(shared);
    }

    pub fn set_shared(&mut self, shared: Vec<u8>) {
        self.shared = Some(shared);
    }

    /// Запрос медиачасти; `None` — не наш.
    pub fn handle(&mut self, req: &Request) -> Option<Response> {
        let ok = || Response::to(req, 200);
        Some(match (req.method.as_str(), req.path()) {
            ("POST", "/fp-setup") => match self.fairplay.handle(&req.body) {
                Ok(body) => ok().with_body(OCTETS, body),
                Err(e) => {
                    warn!("fp-setup: {e}");
                    Response::to(req, 400)
                }
            },
            ("ANNOUNCE", _) => self.raop.announce(req),
            ("SETUP", _) => {
                #[cfg(feature = "hls")]
                {
                    self.cast_v2 = true;
                }
                self.setup(req)
            }
            ("RECORD", _) => {
                #[cfg(feature = "hls")]
                {
                    self.hls_v2_recorded = true;
                }
                let mut resp = ok();
                resp.headers.insert("Audio-Latency", "0");
                resp
            }
            ("SETRATEANCHORTIME", _) => {
                self.set_anchor(req);
                ok()
            }
            ("FLUSHBUFFERED", _) => {
                self.flush(req);
                ok()
            }
            ("FLUSH", _) => {
                // Classic FLUSH: отправитель перемотал или сменил трек.
                if req.headers.get("RTP-Info").is_some() {
                    self.player.with(self.id, |s| s.playout().clear());
                    debug!("FLUSH AirPlay 1: буфер очищен");
                }
                ok()
            }
            ("SETPEERS", _) | ("SETPEERSX", _) => {
                let peers = parse_peers(&req.body);
                let grouped = other_receivers(&req.body, &self.sender_addrs) > 0;
                self.grouped = grouped;
                self.player
                    .with(self.id, |s| s.playout().set_grouped(grouped));
                self.player.time.set_session_peers(self.id.into(), peers);
                ok()
            }
            ("SET_PARAMETER", _) => {
                self.set_parameter(req);
                ok()
            }
            ("GET_PARAMETER", _) => {
                let db = *self
                    .player
                    .volume_db
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                let body = if String::from_utf8_lossy(&req.body).contains("volume") {
                    format!("volume: {db:.6}\r\n").into_bytes()
                } else {
                    Vec::new()
                };
                ok().with_body("text/parameters", body)
            }
            ("TEARDOWN", _) => {
                #[cfg(feature = "hls")]
                self.stop_hls();
                let streams_only = plist_dict(&req.body).is_some_and(|d| d.contains_key("streams"));
                self.raop.reset();
                self.player.release(self.id);
                if self.anchor.take().is_some_and(|a| a.playing) {
                    self.player.events.playing(false);
                }
                if !streams_only {
                    self.player.time.release_session_peers(self.id.into());
                    self.player.forget_reported_state(self.id);
                    if let Some(events) = self.events.take() {
                        events.abort();
                    }
                    // Как shairport на станции: канал MRP живёт до конца
                    // RTSP-соединения или до нового SETUP с wantsDedicatedSocket,
                    // TEARDOWN его не закрывает.
                    self.stop_tasks();
                    if !self.rc_only {
                        self.group.set_group(self.id, None);
                    }
                }
                info!("TEARDOWN");
                ok()
            }
            ("POST", "/command") => self.sender_command(req),
            #[cfg(feature = "hls")]
            ("POST", "/play") => self.hls_play(req),
            #[cfg(feature = "hls")]
            ("POST", "/rate") => self.hls_rate(req),
            #[cfg(feature = "hls")]
            ("POST", "/stop") => {
                self.stop_hls();
                ok()
            }
            #[cfg(feature = "hls")]
            ("POST", "/scrub") => self.hls_scrub(req),
            ("POST", "/feedback") | ("POST", "/audioMode") => ok(),
            _ => return None,
        })
    }

    #[cfg(feature = "hls")]
    fn hls_play(&mut self, req: &Request) -> Response {
        let Some(options) = self.hls.clone() else {
            return Response::to(req, 501);
        };
        if req.body.len() > 64 * 1024
            || !req
                .headers
                .get("Content-Type")
                .is_some_and(|value| value.to_ascii_lowercase().contains("apple-binary-plist"))
            || req
                .headers
                .get("X-Apple-Session-ID")
                .is_none_or(str::is_empty)
        {
            return Response::to(req, 400);
        }
        let Some(body) = plist_dict(&req.body) else {
            return Response::to(req, 400);
        };
        let Some(uuid) = body.get("uuid").and_then(Value::as_string) else {
            return Response::to(req, 400);
        };
        if uuid.is_empty() {
            return Response::to(req, 400);
        }
        if body.get("clientProcName").and_then(Value::as_string) != Some("YouTube") {
            return Response::to(req, 415);
        }
        let start = match body.get("Start-Position-Seconds") {
            None => 0.0,
            Some(value) => {
                let Some(start) = value
                    .as_real()
                    .or_else(|| value.as_signed_integer().map(|number| number as f64))
                    .or_else(|| value.as_unsigned_integer().map(|number| number as f64))
                else {
                    return Response::to(req, 400);
                };
                start
            }
        };
        if !start.is_finite() || start < 0.0 {
            return Response::to(req, 400);
        }
        let Some(location) = body.get("Content-Location").and_then(Value::as_string) else {
            return Response::to(req, 400);
        };
        let url = match hls::Url::parse(location) {
            Ok(url) => url,
            Err(error) => {
                warn!("HLS Content-Location: {error}");
                return Response::to(req, 415);
            }
        };

        if self.cast_v2 {
            if !self.hls_v2_setup || !self.hls_v2_recorded {
                return Response::to(req, 409);
            }
            self.stop_hls();
            self.pending_hls = Some((url, Duration::from_secs_f64(start)));
            return Response::to(req, 200);
        }
        self.start_hls(req, options, url, Duration::from_secs_f64(start))
    }

    /// `POST /rate?value=RATE`: `1.0` starts a pending AirPlay 2 playback or
    /// resumes a paused one, `0.0` pauses. Other rates are not implemented and
    /// are refused instead of being reported as supported.
    #[cfg(feature = "hls")]
    fn hls_rate(&mut self, req: &Request) -> Response {
        let rate = req
            .uri
            .split_once('?')
            .map(|(_, query)| query)
            .into_iter()
            .flat_map(|query| query.split('&'))
            .find_map(|item| item.strip_prefix("value="))
            .and_then(|value| value.parse::<f64>().ok());
        let Some(rate) = rate.filter(|rate| rate.is_finite()) else {
            return Response::to(req, 400);
        };
        if rate != 0.0 && rate != 1.0 {
            return Response::to(req, 501);
        }
        if rate == 1.0 && self.pending_hls.is_some() {
            if !self.cast_v2 || !self.hls_v2_setup || !self.hls_v2_recorded {
                return Response::to(req, 409);
            }
            let Some((url, position)) = self.pending_hls.take() else {
                return Response::to(req, 409);
            };
            let Some(options) = self.hls.clone() else {
                return Response::to(req, 501);
            };
            return self.start_hls(req, options, url, position);
        }
        let Some(playback) = self.hls_playback.as_ref() else {
            return Response::to(req, 409);
        };
        if playback.finished() {
            return Response::to(req, 409);
        }
        if rate == 0.0 {
            debug!("HLS pause");
            playback.pause();
        } else {
            debug!("HLS resume");
            playback.resume();
        }
        Response::to(req, 200)
    }

    /// `POST /scrub?position=SECONDS`: restart at the playlist segment that
    /// contains the position. Segment granularity is the confirmed data; a
    /// frame-accurate seek is not offered.
    #[cfg(feature = "hls")]
    fn hls_scrub(&mut self, req: &Request) -> Response {
        let Some(playback) = self.hls_playback.as_ref() else {
            return Response::to(req, 409);
        };
        if playback.finished() {
            return Response::to(req, 409);
        }
        let position = req
            .uri
            .split_once('?')
            .map(|(_, query)| query)
            .into_iter()
            .flat_map(|query| query.split('&'))
            .find_map(|item| item.strip_prefix("position="))
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0);
        let Some(position) = position else {
            return Response::to(req, 400);
        };
        if !playback.seek(Duration::from_secs_f64(position)) {
            return Response::to(req, 416);
        }
        info!(position, "HLS scrub");
        Response::to(req, 200)
    }

    #[cfg(feature = "hls")]
    fn start_hls(
        &mut self,
        req: &Request,
        options: HlsOptions,
        url: hls::Url,
        position: Duration,
    ) -> Response {
        let fetcher = match hls::Fetcher::new() {
            Ok(fetcher) => fetcher,
            Err(error) => {
                warn!("HLS client: {error}");
                return Response::to(req, 500);
            }
        };
        self.stop_hls();
        let playback = hls::Playback::spawn(
            fetcher,
            url,
            hls::PlaybackOptions {
                max_bandwidth: options.max_bandwidth,
                video: options.decoder,
                video_sink: options.sink,
                audio: options.audio,
            },
            position,
        );
        info!(position = %position.as_secs_f64(), "HLS playback");
        self.hls_playback = Some(Arc::new(playback));
        Response::to(req, 200)
    }

    #[cfg(feature = "hls")]
    fn stop_hls(&mut self) {
        self.pending_hls = None;
        if let Some(playback) = self.hls_playback.take() {
            playback.stop();
        }
    }

    fn sender_command(&self, req: &Request) -> Response {
        let ok = || Response::to(req, 200);
        let Some(command) = plist_dict(&req.body) else {
            // `/command` — всегда plist. Пустое тело не наказываем, битое — 400.
            return if req.body.is_empty() {
                ok()
            } else {
                Response::to(req, 400)
            };
        };
        let kind = command.get("type").and_then(Value::as_string).unwrap_or("");
        debug!(kind, "sender command");
        if self.rc_only {
            return ok();
        }
        if kind == "updateMRNowPlayingInfo" {
            let mut previous = self.now_playing_duration.lock().unwrap_or_else(|e| e.into_inner());
            let Some(now) = parse_now_playing(&command, *previous) else {
                // Конверт `npi-text` обязателен: отправитель, который его не
                // прислал, должен узнать об этом кодом 400, а не считать,
                // что метаданные приняты (music-assistant/airplay-cli, §8).
                debug!("now-playing: конверт npi-text не разобран");
                return Response::to(req, 400);
            };
            if now.replace || now.metadata.duration.is_some() { *previous = now.metadata.duration; }
            drop(previous);
            debug!(?now.metadata, "now-playing через POST /command");
            let events = &self.player.events;
            if now.metadata != Metadata::default() { events.metadata(&now.metadata); }
            if let Some(progress) = now.progress {
                events.progress(progress);
            }
            if let Some((mime, data)) = now.artwork {
                events.artwork(&mime, &data);
            }
            return ok();
        }
        if kind == "updateMRPlaybackState" {
            let state = command
                .get("params")
                .and_then(Value::as_dictionary)
                .and_then(|p| uint(p, "mrPlaybackState"));
            match state {
                Some(1) => self.player.report_state(self.id, true),
                Some(2..=4) => self.player.report_state(self.id, false),
                _ => return ok(),
            }
            debug!(?state, "sender playback state");
        }
        ok()
    }

    fn setup(&mut self, req: &Request) -> Response {
        let Some(body) = plist_dict(&req.body) else {
            return self.setup_raop(req);
        };
        let reply = match body.get("streams").and_then(Value::as_array) {
            Some(streams) => self.setup_streams(streams),
            None => self.setup_session(&body).map_err(SetupError::Failed),
        };
        match reply {
            Ok(d) => Response::to(req, 200).with_body(BPLIST, write_plist(d)),
            Err(SetupError::Conflict) => {
                warn!("SETUP: звук уже у другого отправителя");
                Response::to(req, 409)
            }
            Err(SetupError::Failed(e)) => {
                warn!("SETUP: {e}");
                Response::to(req, 500)
            }
        }
    }

    /// SETUP classic AirPlay 1: `Transport` без plist, порты занимает
    /// [`Raop`], медиачасть ставит поток в плеер.
    fn setup_raop(&mut self, req: &Request) -> Response {
        if !self.player.available(self.id) {
            return Response::to(req, 409);
        }
        match self.raop.setup(req, self.player.audio.clone(), self.remote) {
            Ok((resp, stream)) => {
                self.anchor = None;
                if self.player.replace(self.id, stream) {
                    resp
                } else {
                    Response::to(req, 409)
                }
            }
            Err(resp) => resp,
        }
    }

    /// Первый SETUP: часы и канал событий.
    fn setup_session(&mut self, body: &Dictionary) -> Result<Dictionary, String> {
        let timing = body
            .get("timingProtocol")
            .and_then(Value::as_string)
            .unwrap_or("None");
        self.rc_only = body
            .get("isRemoteControlOnly")
            .and_then(Value::as_boolean)
            .unwrap_or(false);
        info!(timing, rc_only = self.rc_only, "SETUP сессии");
        let mut d = Dictionary::new();
        #[cfg(feature = "mirroring")]
        if let Some(ekey) = body.get("ekey").and_then(Value::as_data) {
            match <&[u8; 72]>::try_from(ekey).map(|e| self.fairplay.decrypt(e)) {
                Ok(Ok(key)) => self.aes_key = Some(key),
                Ok(Err(e)) => warn!("ekey: {e}"),
                Err(_) => warn!(len = ekey.len(), "ekey неожиданной длины"),
            }
        }
        #[cfg(feature = "mirroring")]
        if let Some(eiv) = body.get("eiv").and_then(Value::as_data) {
            self.eiv = eiv.try_into().ok();
        }
        if timing == "NTP" {
            let requested = uint(body, "timingPort").unwrap_or(0);
            // Порт из сети: `as u16` завернул бы 65536+n в чужой порт.
            match u16::try_from(requested) {
                Ok(port) => match timesync::ntp::start(SocketAddr::new(self.remote, port)) {
                    Ok((local, clock, task)) => {
                        self.tasks.push(task);
                        self.ntp = Some(clock);
                        #[cfg(feature = "hls")]
                        {
                            self.hls_v2_setup = port != 0;
                        }
                        d.insert("timingPort".into(), Value::Integer(local.into()));
                    }
                    Err(e) => warn!("NTP: {e}"),
                },
                Err(_) => warn!(requested, "NTP: timingPort вне диапазона u16"),
            }
        }
        if timing == "PTP" {
            if let Some(group) = body.get("groupUUID").and_then(Value::as_string) {
                self.pending_group = Some(group.to_owned());
                if self.player.available(self.id) {
                    self.group.set_group(self.id, self.pending_group.clone());
                }
            }
            // The RTSP peer remains a timing peer even when timingPeerInfo is absent.
            self.sender_addrs = body
                .get("timingPeerInfo")
                .and_then(Value::as_dictionary)
                .map(addresses)
                .unwrap_or_default();
            if !self.sender_addrs.contains(&self.remote) {
                self.sender_addrs.push(self.remote);
            }
            self.player
                .time
                .set_session_peers(self.id.into(), self.sender_addrs.clone());
            let ip = self.local.to_string();
            let mut peer = Dictionary::new();
            peer.insert("Addresses".into(), Value::Array(vec![ip.clone().into()]));
            peer.insert("ID".into(), ip.into());
            if let Some(reading) = self.player.time.selected_reading() {
                peer.insert(
                    "ClockID".into(),
                    Value::Integer(reading.domain().id().into()),
                );
                peer.insert("ID".into(), self.pairing_id.clone().into());
                peer.insert("DeviceType".into(), Value::Integer(0u64.into()));
                peer.insert("SupportsClockPortMatchingOverride".into(), true.into());
                info!(
                    clock_id = reading.domain().id(),
                    "PTP: selected timeline in SETUP"
                );
            }
            d.insert("timingPeerInfo".into(), peer.into());
        }
        let port = self.start_events().unwrap_or(0);
        d.insert("eventPort".into(), Value::Integer(port.into()));
        // shairport-sync: фиктивный timingPort 0 только в ветке PTP; у сессии
        // только для пульта (timingProtocol None) в ответе один eventPort.
        if timing == "PTP" && !d.contains_key("timingPort") {
            d.insert("timingPort".into(), Value::Integer(0.into()));
        }
        Ok(d)
    }

    fn stop_tasks(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
        self.ntp = None;
    }

    /// Session NTP mapping when selected, otherwise the shared PTP service.
    fn time_source(&self) -> Arc<dyn TimeSource> {
        match &self.ntp {
            Some(clock) => Arc::new(clock.clone()),
            None => Arc::new(self.player.time.clone()),
        }
    }

    /// Тип 110: видеопоток зеркалирования.
    #[cfg(feature = "mirroring")]
    fn setup_video(&mut self, s: &Dictionary, r: &mut Dictionary) -> Result<(), String> {
        let factory = self
            .mirror
            .clone()
            .ok_or("зеркалирование выключено (--mirror)")?;
        let id = video_stream_id(s, self.aes_key.is_none())?;
        let authenticated = if self.aes_key.is_none() {
            let shared = self
                .shared
                .as_deref()
                .ok_or("video requires ekey or HAP session")?;
            Some(crypto::kdf::hkdf_sha512(
                shared,
                format!("DataStream-Salt{id}").as_bytes(),
                b"DataStream-Output-Encryption-Key",
            ))
        } else {
            None
        };
        let listener = audio_stream::bind_tcp().map_err(|e| format!("видеосокет: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        info!(id, port, "видеопоток");
        let info = mirror::StreamInfo {
            time: self.time_source(),
        };
        let sink = factory.open(&info);
        if let Some(key) = authenticated {
            info!("video cipher: HAP-derived ChaCha20-Poly1305");
            self.tasks.push(tokio::spawn(mirror::receive_authenticated(
                listener, key, sink,
            )));
        } else {
            let aes = self.aes_key.ok_or("missing legacy video key")?;
            let session = mirror::session_key(&aes, self.legacy_shared.as_ref());
            let (key, iv) = mirror::stream_key_iv(&session, id);
            self.tasks
                .push(tokio::spawn(mirror::receive(listener, key, iv, sink)));
        }
        r.insert("dataPort".into(), Value::Integer(port.into()));
        Ok(())
    }

    /// Звук зеркалирования: тип 96 без `shk`, AAC-ELD, AES-128-CBC ключом
    /// сессии и `eiv`, время по NTP. Без декодера AAC-ELD (сборка без фичи
    /// `ffmpeg`) порты всё равно отдаём, иначе отправитель не начнёт видео.
    #[cfg(feature = "mirroring")]
    fn setup_mirror_audio(&mut self, s: &Dictionary, r: &mut Dictionary) -> Result<(), SetupError> {
        let format = Format {
            codec: Codec::AacEld,
            sample_rate: sample_rate(s)?,
            channels: 2,
        };
        if let Err(e) = codec::Decoder::new(format) {
            warn!("звук зеркалирования не играет: {e}");
            return self.drain_mirror_audio(r).map_err(Into::into);
        }
        let aes = self.aes_key.ok_or("нет ключа звука: SETUP без ekey")?;
        let iv = self.eiv.ok_or("нет eiv в SETUP")?;
        let key = mirror::session_key(&aes, self.legacy_shared.as_ref());
        let stream = Stream::realtime(
            self.player.audio.clone(),
            StreamKey::AesCbc { key, iv },
            format,
            self.time_source(),
        )
        .map_err(|e| format!("сокеты звука: {e}"))?;
        info!("звук зеркалирования (AAC-ELD)");
        r.insert("dataPort".into(), Value::Integer(stream.data_port.into()));
        r.insert(
            "controlPort".into(),
            Value::Integer(stream.control_port.into()),
        );
        if !self.player.replace(self.id, stream) {
            return Err(SetupError::Conflict);
        }
        Ok(())
    }

    #[cfg(feature = "mirroring")]
    fn drain_mirror_audio(&mut self, r: &mut Dictionary) -> Result<(), String> {
        for key in ["dataPort", "controlPort"] {
            let socket = std::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0))
                .or_else(|_| std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)))
                .and_then(|s| {
                    s.set_nonblocking(true)?;
                    tokio::net::UdpSocket::from_std(s)
                })
                .map_err(|e| format!("сокет звука: {e}"))?;
            let port = socket.local_addr().map_err(|e| e.to_string())?.port();
            r.insert(key.into(), Value::Integer(port.into()));
            self.tasks.push(tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                while socket.recv(&mut buf).await.is_ok() {}
            }));
        }
        Ok(())
    }

    /// Канал событий: отправитель подключается сам; отправляем только команды.
    fn start_events(&mut self) -> Option<u16> {
        let shared = self.shared.as_ref()?;
        let listener = match audio_stream::bind_tcp() {
            Ok(l) => l,
            Err(e) => {
                warn!("канал событий: {e}");
                return None;
            }
        };
        let port = listener.local_addr().ok()?.port();
        let mut channel = Channel::new(ControlKeys::accessory_events(shared));
        if let Some(old) = self.events.take() {
            old.abort();
        }
        let control = (!self.rc_only).then(|| self.control.clone());
        let player = self.player.clone();
        let owner = self.id;
        self.events = Some(tokio::spawn(async move {
            let Ok((mut conn, peer)) = listener.accept().await else {
                return;
            };
            debug!(%peer, "канал событий подключён");
            // shairport-sync на станции сам ничего не шлёт в канал событий
            // при подключении: только команды пульта. updateInfo не шлём.
            let mut commands = control.as_ref().map(|c| c.attach(owner));
            let mut cseq = 0u32;
            let mut buf = [0u8; 1024];
            loop {
                tokio::select! {
                    // Ответы отправителя на наши команды; читать надо, чтобы он не встал.
                    n = conn.read(&mut buf) => {
                        if !matches!(n, Ok(n) if n > 0) {
                            break;
                        }
                    }
                    Some(command) = next_command(&mut commands) => {
                        if let Command::Volume(v) = command {
                            player.set_volume(owner, Command::volume_db(v));
                        }
                        cseq += 1;
                        let req = control::command_request(control::command_body(command), cseq);
                        let mut wire = Vec::new();
                        channel.seal(&req, &mut wire);
                        if conn.write_all(&wire).await.is_err() {
                            break;
                        }
                        debug!(?command, "команда отправителю");
                    }
                }
            }
            if let Some(c) = control {
                c.detach(owner);
            }
            debug!("канал событий закрыт");
        }));
        Some(port)
    }

    fn setup_streams(&mut self, streams: &[Value]) -> Result<Dictionary, SetupError> {
        let mut replies = Vec::new();
        for s in streams {
            let Some(s) = s.as_dictionary() else { continue };
            let kind = uint(s, "type").unwrap_or(0);
            // Звуковые потоки занимают единственный плеер.
            if matches!(kind, 96 | 103) && !self.player.available(self.id) {
                return Err(SetupError::Conflict);
            }
            let mut r = Dictionary::new();
            r.insert("type".into(), Value::Integer(kind.into()));
            match kind {
                #[cfg(feature = "mirroring")]
                96 if self.mirror.is_some() && !s.contains_key("shk") => {
                    self.setup_mirror_audio(s, &mut r)?;
                }
                #[cfg(feature = "mirroring")]
                110 => self.setup_video(s, &mut r)?,
                #[cfg(not(feature = "mirroring"))]
                110 => return Err("mirroring is disabled in this build".into()),
                96 | 103 => {
                    let shk: [u8; 32] = s
                        .get("shk")
                        .and_then(Value::as_data)
                        .and_then(|k| k.try_into().ok())
                        .ok_or("нет ключа потока shk")?;
                    let stream = if kind == 103 {
                        info!("буферизованный поток (AAC)");
                        Stream::buffered(self.player.audio.clone(), &shk, self.player.time_source())
                    } else {
                        // This renderer schedules the anchor as the DAC deadline;
                        // sink.delay_frames accounts for its output queue. There is
                        // no additional fixed AP2 render interval to subtract at TX.
                        r.insert("latencyMin".into(), Value::Integer(0u64.into()));
                        let format = realtime_format(s)?;
                        info!(?format, "realtime-поток");
                        Stream::realtime(
                            self.player.audio.clone(),
                            StreamKey::ChaCha(shk),
                            format,
                            if self.ntp.is_some() {
                                self.time_source()
                            } else {
                                self.player.time_source()
                            },
                        )
                    }
                    .map_err(|e| format!("сокеты потока: {e}"))?;
                    r.insert("dataPort".into(), Value::Integer(stream.data_port.into()));
                    r.insert(
                        "controlPort".into(),
                        Value::Integer(stream.control_port.into()),
                    );
                    if kind == 103 {
                        r.insert(
                            "audioBufferSize".into(),
                            Value::Integer(AUDIO_BUFFER_SIZE.into()),
                        );
                    }
                    self.anchor = None;
                    stream.playout().set_grouped(self.grouped);
                    if !self.player.replace(self.id, stream) {
                        return Err(SetupError::Conflict);
                    }
                }
                130 => self.setup_remote_control(s, &mut r)?,
                other => {
                    warn!(kind = other, "поток не поддерживается");
                    continue;
                }
            }
            if matches!(kind, 96 | 103)
                && let Some(group) = &self.pending_group
            {
                self.group.set_group(self.id, Some(group.clone()));
            }
            replies.push(Value::Dictionary(r));
        }
        let mut d = Dictionary::new();
        d.insert("streams".into(), Value::Array(replies));
        Ok(d)
    }

    /// Поток 130 (MediaRemote). Без `seed` это логическая регистрация:
    /// отвечаем `streamID`, канала нет. С `seed` и заданной службой MRP
    /// открываем шифрованный канал данных и пересылаем его туда; новый
    /// SETUP заменяет прежний канал.
    fn setup_remote_control(&mut self, s: &Dictionary, r: &mut Dictionary) -> Result<(), String> {
        r.insert("streamID".into(), Value::Integer(1.into()));
        let seed = uint(s, "seed");
        let (Some(seed), Some(service)) = (seed, self.mrp_service.clone()) else {
            debug!(seed = seed.is_some(), "MRP: логическая регистрация");
            return Ok(());
        };
        let shared = self.shared.as_ref().ok_or("канал MRP без сопряжения")?;
        let listener = audio_stream::bind_tcp().map_err(|e| format!("сокет MRP: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        if let Some(old) = self.mrp.take() {
            old.abort();
        }
        self.mrp = Some(tokio::spawn(control::bridge(
            listener,
            self.remote,
            ControlKeys::accessory_data(shared, seed),
            service,
        )));
        info!(port, "канал MRP");
        r.insert("dataPort".into(), Value::Integer(port.into()));
        Ok(())
    }

    fn set_anchor(&mut self, req: &Request) {
        let Some(d) = plist_dict(&req.body) else {
            return;
        };
        // RTP-метка — 32 бита; большее значение молча завернулось бы.
        if let Some(rtp) = uint(&d, "rtpTime")
            && rtp > u32::MAX as u64
        {
            warn!(rtp, "SETRATEANCHORTIME: rtpTime не влезает в u32");
            return;
        }
        let rate = d
            .get("rate")
            .and_then(|v| {
                v.as_real()
                    .or_else(|| v.as_signed_integer().map(|i| i as f64))
            })
            .unwrap_or(0.0);
        let anchor = match (uint(&d, "rtpTime"), uint(&d, "networkTimeSecs")) {
            (Some(rtp), Some(secs)) => Anchor {
                rtp: rtp as u32,
                network_ns: Anchor::network_time(secs, uint(&d, "networkTimeFrac").unwrap_or(0)),
                timeline: uint(&d, "networkTimeTimelineID").unwrap_or(0),
                playing: rate != 0.0,
            },
            // Пауза приходит без времени: берём прежний якорь.
            _ => match self.anchor {
                Some(a) => Anchor {
                    playing: rate != 0.0,
                    ..a
                },
                None => return,
            },
        };
        debug!(?anchor, rate, "SETRATEANCHORTIME");
        if self.anchor.is_none_or(|a| a.playing != anchor.playing) {
            self.player.events.playing(anchor.playing);
        }
        self.anchor = Some(anchor);
        self.player
            .with(self.id, |s| s.playout().set_anchor(anchor));
    }

    fn flush(&mut self, req: &Request) {
        let Some(d) = plist_dict(&req.body) else {
            return;
        };
        // Метки — 32 бита, как и rtpTime.
        let Some(until) = uint(&d, "flushUntilTS").filter(|t| *t <= u32::MAX as u64) else {
            return;
        };
        let from = uint(&d, "flushFromTS")
            .filter(|t| *t <= u32::MAX as u64)
            .map(|t| t as u32);
        self.player
            .with(self.id, |s| s.playout().flush(from, until as u32));
    }

    /// Громкость и позиция (`text/parameters`), метаданные (DMAP) и
    /// обложка (картинка).
    fn set_parameter(&mut self, req: &Request) {
        let events = &self.player.events;
        let ct = req.headers.get("Content-Type").unwrap_or("text/parameters");
        if ct.starts_with("application/x-dmap-tagged") {
            let m = Metadata::from_dmap(&req.body);
            debug!(?m, "метаданные");
            events.metadata(&m);
            return;
        }
        if ct.starts_with("image") {
            debug!(ct, len = req.body.len(), "обложка");
            if !req.body.is_empty() {
                events.artwork(ct, &req.body);
            }
            return;
        }
        let text = String::from_utf8_lossy(&req.body);
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("volume:")
                && let Ok(db) = v.trim().parse::<f32>()
            {
                info!(db, "громкость");
                self.player.set_volume(self.id, db);
                events.volume(db);
            } else if let Some(p) = line.strip_prefix("progress:").and_then(Progress::parse) {
                events.progress(p);
            }
        }
    }
}

/// Соединение RTSP закрылось — его звук тоже заканчивается.
impl Drop for Media {
    fn drop(&mut self) {
        #[cfg(feature = "hls")]
        self.stop_hls();
        self.player.time.release_session_peers(self.id.into());
        self.player.release(self.id);
        self.player.forget_reported_state(self.id);
        self.stop_tasks();
        if let Some(events) = self.events.take() {
            events.abort();
        }
        if let Some(mrp) = self.mrp.take() {
            mrp.abort();
        }
        self.control.detach(self.id);
        // Соединение оборвалось без TEARDOWN: сессия отправителя закончилась,
        // иначе gid/card остаются висеть после краткого или долгого разрыва.
        if !self.rc_only {
            self.group.set_group(self.id, None);
        }
    }
}

/// Следующая команда, или вечное ожидание, если команд для этого
/// соединения нет.
async fn next_command(
    rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<Command>>,
) -> Option<Command> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

fn realtime_format(s: &Dictionary) -> Result<Format, String> {
    let advertisement = codec::matrix::Advertisement::from_fields(
        uint(s, "ct"),
        uint(s, "ss"),
        uint(s, "sr"),
        uint(s, "spf"),
    );
    let mut format = codec::matrix::format(advertisement).map_err(codec::matrix::Rejection::reason)?;
    let channels = uint(s, "ch").unwrap_or(2);
    if !matches!(channels, 1 | 2 | 6 | 8) || (matches!(format.codec, Codec::Aac) && channels > 2) {
        return Err(format!("неподдерживаемое число каналов ch={channels}"));
    }
    format.channels = channels as u8;
    Ok(format)
}

/// Частота дискретизации из SETUP: ноль или абсурдное значение дали бы
/// деление на ноль в планировщике вывода.
#[cfg(feature = "mirroring")]
fn sample_rate(s: &Dictionary) -> Result<u32, String> {
    let rate = uint(s, "sr").unwrap_or(44_100);
    if !(8_000..=384_000).contains(&rate) {
        return Err(format!("недопустимая частота sr={rate}"));
    }
    Ok(rate as u32)
}

/// Тело RTSP → словарь plist. Разбор идёт через [`rtsp::plist`]: вложенность
/// ограничена, иначе тело из сети снимает стек.
/// Распознанное `updateMRNowPlayingInfo` — метаданные, позиция и обложка.
#[derive(Debug)]
struct NowPlaying {
    replace: bool,
    metadata: Metadata,
    progress: Option<Progress>,
    artwork: Option<(String, Vec<u8>)>,
}

/// `POST /command` с `type=updateMRNowPlayingInfo` (D63). Конверт
/// `{type:"npi-text", mergePolicy, params:{…}}` обязателен, иначе
/// отправитель получает 400; внутри — ключи `kMRMediaRemoteNowPlayingInfo*`
/// без префикса. Источник: music-assistant/airplay-cli, DESIGN.md §8
/// (Apache-2.0); код не копировался — только имена полей и их смысл.
/// `update` can carry only elapsed time, so it may reuse the prior duration.
fn parse_now_playing(command: &Dictionary, previous_duration: Option<Duration>) -> Option<NowPlaying> {
    /// Сколько байт обложки принимаем: отправитель шлёт около 43 КБ, предел
    /// в 5 МиБ — из того же описания и защищает память.
    const MAX_ARTWORK: usize = 5 << 20;
    let envelope = command.get("params").and_then(Value::as_dictionary)?;
    if envelope.get("type").and_then(Value::as_string) != Some("npi-text") {
        return None;
    }
    let replace = match envelope.get("mergePolicy").and_then(Value::as_string)? {
        "replace" => true,
        "update" => false,
        _ => return None,
    };
    let params = envelope.get("params").and_then(Value::as_dictionary)?;
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_string)
            .filter(|s| !s.is_empty() && s.len() <= 4096)
            .map(str::to_owned)
    };
    let real = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_real)
            .filter(|v| v.is_finite())
    };
    let duration = real("Duration")
        .filter(|s| *s > 0.0)
        .and_then(|s| Duration::try_from_secs_f64(s).ok());
    let metadata = Metadata {
        title: text("Title"),
        artist: text("Artist"),
        album: text("Album"),
        genre: text("Genre"),
        duration,
    };
    let progress = duration.or(if replace { None } else { previous_duration }).and_then(|duration| {
        let elapsed = real("ElapsedTime").filter(|s| *s >= 0.0)?;
        // Позиция не может быть дальше конца: отправитель считает её сам,
        // а расхождение иначе уедет в карточку.
        let position = elapsed.min(duration.as_secs_f64());
        Some(Progress {
            position: Duration::try_from_secs_f64(position).ok()?,
            duration,
        })
    });
    let artwork = params
        .get("ArtworkData")
        .and_then(Value::as_data)
        .filter(|data| !data.is_empty() && data.len() <= MAX_ARTWORK)
        .map(|data| {
            let mime = text("ArtworkMIMEType").unwrap_or_else(|| "image/jpeg".into());
            (mime, data.to_vec())
        });
    Some(NowPlaying {
        replace,
        metadata,
        progress,
        artwork,
    })
}

fn plist_dict(body: &[u8]) -> Option<Dictionary> {
    if body.is_empty() {
        return None;
    }
    rtsp::plist::parse(body)?.into_dictionary()
}

fn write_plist(d: Dictionary) -> Vec<u8> {
    let mut out = Vec::new();
    Value::Dictionary(d)
        .to_writer_binary(&mut out)
        .expect("запись plist в память не может завершиться ошибкой");
    out
}

fn uint(d: &Dictionary, key: &str) -> Option<u64> {
    d.get(key).and_then(Value::as_unsigned_integer)
}

/// Адреса из `timingPeerInfo.Addresses` и SETPEERSX. Нестроковые и
/// неразбираемые элементы пропускаются (как и в shairport после 5.5.1),
/// список ограничен и очищен от повторов: он приходит из сети и уходит
/// в службу часов.
const MAX_PEERS: usize = 32;

fn addresses(d: &Dictionary) -> Vec<IpAddr> {
    let mut peers = Vec::new();
    extend_peers(
        &mut peers,
        d.get("Addresses")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_string()?.parse().ok()),
    );
    peers
}

fn extend_peers(peers: &mut Vec<IpAddr>, candidates: impl IntoIterator<Item = IpAddr>) {
    for ip in candidates {
        if peers.len() >= MAX_PEERS {
            return;
        }
        if !ip.is_unspecified() && !peers.contains(&ip) {
            peers.push(ip);
        }
    }
}

/// Сколько в SETPEERS/SETPEERSX приёмников, кроме нас и отправителя.
/// SETPEERS — плоский список адресов, SETPEERSX — участник на словарь.
fn other_receivers(body: &[u8], sender: &[IpAddr]) -> usize {
    let Some(Value::Array(items)) = rtsp::plist::parse(body) else {
        return 0;
    };
    let local: Vec<IpAddr> = if_addrs::get_if_addrs()
        .map(|a| a.into_iter().map(|i| i.ip().to_canonical()).collect())
        .unwrap_or_default();
    let known = |ip: &IpAddr| {
        let ip = ip.to_canonical();
        sender.contains(&ip) || local.contains(&ip) || ip.is_loopback()
    };
    items
        .iter()
        .filter(|item| match item {
            Value::String(s) => s.parse::<IpAddr>().is_ok_and(|ip| !known(&ip)),
            Value::Dictionary(d) => {
                let addrs = addresses(d);
                !addrs.is_empty() && !addrs.iter().any(known)
            }
            _ => false,
        })
        .count()
}

/// SETPEERS — массив адресов, SETPEERSX — массив словарей с `Addresses`.
fn parse_peers(body: &[u8]) -> Vec<IpAddr> {
    let Some(Value::Array(items)) = rtsp::plist::parse(body) else {
        return Vec::new();
    };
    let mut peers = Vec::new();
    for item in items {
        match item {
            Value::String(s) => extend_peers(&mut peers, s.parse::<IpAddr>().ok()),
            Value::Dictionary(d) => extend_peers(&mut peers, addresses(&d)),
            _ => {}
        }
        if peers.len() >= MAX_PEERS {
            break;
        }
    }
    peers
}

/// Ошибка SETUP: конфликт владения отличается от прочих.
enum SetupError {
    Conflict,
    Failed(String),
}

impl From<String> for SetupError {
    fn from(e: String) -> Self {
        SetupError::Failed(e)
    }
}

impl From<&str> for SetupError {
    fn from(e: &str) -> Self {
        SetupError::Failed(e.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_playing_command_maps_to_the_existing_events() {
        let mut inner = Dictionary::new();
        inner.insert("Title".into(), Value::String("Song".into()));
        inner.insert("Artist".into(), Value::String("Artist".into()));
        inner.insert("Album".into(), Value::String("Album".into()));
        inner.insert("Duration".into(), Value::Real(210.5));
        inner.insert("ElapsedTime".into(), Value::Real(12.0));
        inner.insert("ArtworkMIMEType".into(), Value::String("image/jpeg".into()));
        inner.insert("ArtworkData".into(), Value::Data(vec![0xff, 0xd8, 0xff]));
        let mut envelope = Dictionary::new();
        envelope.insert("type".into(), Value::String("npi-text".into()));
        envelope.insert("mergePolicy".into(), Value::String("replace".into()));
        envelope.insert("params".into(), Value::Dictionary(inner));
        let mut command = Dictionary::new();
        command.insert(
            "type".into(),
            Value::String("updateMRNowPlayingInfo".into()),
        );
        command.insert("params".into(), Value::Dictionary(envelope));
        let now = parse_now_playing(&command, None).unwrap();
        assert_eq!(now.metadata.title.as_deref(), Some("Song"));
        assert_eq!(now.metadata.artist.as_deref(), Some("Artist"));
        assert_eq!(
            now.metadata.duration,
            Some(std::time::Duration::from_secs_f64(210.5))
        );
        let progress = now.progress.unwrap();
        assert_eq!(progress.position, std::time::Duration::from_secs(12));
        assert_eq!(
            progress.duration,
            std::time::Duration::from_secs_f64(210.5)
        );
        let (mime, data) = now.artwork.unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(data, vec![0xff, 0xd8, 0xff]);
    }

    #[test]
    fn now_playing_needs_the_npi_envelope_and_keeps_metadata_without_duration() {
        let mut inner = Dictionary::new();
        inner.insert("Title".into(), Value::String("Song".into()));
        let mut bare = Dictionary::new();
        bare.insert("params".into(), Value::Dictionary(inner));
        assert!(parse_now_playing(&bare, None).is_none());
        let mut inner = Dictionary::new();
        inner.insert("Title".into(), Value::String("Song".into()));
        let mut envelope = Dictionary::new();
        envelope.insert("type".into(), Value::String("npi-text".into()));
        envelope.insert("mergePolicy".into(), Value::String("replace".into()));
        envelope.insert("params".into(), Value::Dictionary(inner));
        let mut command = Dictionary::new();
        command.insert("params".into(), Value::Dictionary(envelope));
        let now = parse_now_playing(&command, None).unwrap();
        assert_eq!(now.metadata.title.as_deref(), Some("Song"));
        assert!(now.progress.is_none());
        assert!(now.artwork.is_none());
    }

    #[test]
    fn now_playing_update_preserves_duration_and_rejects_invalid_envelopes() {
        let mut params = Dictionary::new();
        params.insert("ElapsedTime".into(), Value::Real(12.0));
        params.insert("Duration".into(), Value::Real(1.0e100));
        let mut envelope = Dictionary::new();
        envelope.insert("type".into(), Value::String("npi-text".into()));
        envelope.insert("params".into(), Value::Dictionary(params));
        let mut command = Dictionary::new();
        command.insert("params".into(), Value::Dictionary(envelope.clone()));
        assert!(parse_now_playing(&command, None).is_none());
        envelope.insert("mergePolicy".into(), Value::String("update".into()));
        command.insert("params".into(), Value::Dictionary(envelope));
        let now = parse_now_playing(&command, Some(Duration::from_secs(100))).unwrap();
        assert_eq!(now.progress.unwrap().position, Duration::from_secs(12));
        assert!(now.metadata.duration.is_none());
    }

    #[cfg(feature = "mirroring")]
    #[test]
    fn authenticated_video_requires_integer_id() {
        let mut descriptor = Dictionary::new();
        assert!(video_stream_id(&descriptor, true).is_err());
        assert_eq!(video_stream_id(&descriptor, false).unwrap(), 0);
        descriptor.insert("streamConnectionID".into(), "invalid".into());
        assert!(video_stream_id(&descriptor, true).is_err());
        descriptor.insert("streamConnectionID".into(), Value::Integer(7u64.into()));
        assert_eq!(video_stream_id(&descriptor, true).unwrap(), 7);
    }

    fn plist(v: Value) -> Vec<u8> {
        let mut out = Vec::new();
        v.to_writer_binary(&mut out).unwrap();
        out
    }

    #[test]
    fn addresses_are_bounded_and_cleaned() {
        let mut items: Vec<Value> = vec![
            Value::Integer(1.into()),
            Value::Dictionary(Dictionary::new()),
            "not-an-ip".into(),
            "0.0.0.0".into(),
            "::".into(),
            "192.168.1.10".into(),
            "192.168.1.10".into(),
        ];
        items.extend((0..1000).map(|i| format!("10.1.{}.{}", i / 256, i % 256).into()));
        let mut d = Dictionary::new();
        d.insert("Addresses".into(), Value::Array(items));
        let peers = addresses(&d);
        assert_eq!(peers.len(), MAX_PEERS);
        assert_eq!(peers[0], "192.168.1.10".parse::<IpAddr>().unwrap());
        assert!(!peers.iter().any(|ip| ip.is_unspecified()));

        // `Addresses` не массив — пустой список, без паники.
        for value in [
            Value::String("192.168.1.10".into()),
            Value::Integer(1.into()),
            Value::Data(vec![1, 2, 3]),
        ] {
            let mut d = Dictionary::new();
            d.insert("Addresses".into(), value);
            assert!(addresses(&d).is_empty());
        }
        assert!(addresses(&Dictionary::new()).is_empty());
    }

    #[test]
    fn peers_from_setpeers_are_bounded() {
        let mut flat: Vec<Value> = vec!["192.168.1.10".into()];
        flat.extend((0..100).map(|i| format!("10.2.0.{i}").into()));
        let peers = parse_peers(&plist(Value::Array(flat)));
        assert_eq!(peers.len(), MAX_PEERS);
        assert_eq!(peers[0], "192.168.1.10".parse::<IpAddr>().unwrap());
        assert!(parse_peers(b"garbage").is_empty());
    }

    #[test]
    fn realtime_format_rejects_impossible_parameters() {
        fn descriptor(fields: &[(&str, u64)]) -> Dictionary {
            let mut d = Dictionary::new();
            for (key, value) in fields {
                d.insert((*key).into(), Value::Integer((*value).into()));
            }
            d
        }
        for (sr, spf, ss, ct) in [
            (0u64, 352u64, 16u64, 1u64),
            (4_294_967_296, 352, 16, 1),
            (44_100, 0, 16, 1),
            (44_100, 10_000, 16, 1),
            (44_100, 352, 17, 1),
            (44_100, 352, 272, 2),
            (44_100, 352, 16, 8),
        ] {
            let d = descriptor(&[("sr", sr), ("spf", spf), ("ss", ss), ("ct", ct)]);
            assert!(
                realtime_format(&d).is_err(),
                "sr={sr} spf={spf} ss={ss} ct={ct}"
            );
        }
        let pcm = descriptor(&[("sr", 44_100), ("spf", 352), ("ss", 16), ("ct", 1)]);
        let format = realtime_format(&pcm).unwrap();
        assert_eq!(format.sample_rate, 44_100);
        assert_eq!(
            format.codec,
            Codec::Pcm {
                frames_per_packet: 352
            }
        );
        let alac = descriptor(&[("sr", 48_000), ("spf", 352), ("ss", 24), ("ct", 2)]);
        assert_eq!(
            realtime_format(&alac).unwrap().codec,
            Codec::Alac {
                bit_depth: 24,
                frames_per_packet: 352
            }
        );
    }

    #[test]
    fn other_receivers_exclude_sender_and_self() {
        let sender: Vec<IpAddr> =
            vec!["192.168.1.10".parse().unwrap(), "fd00::10".parse().unwrap()];
        let flat = |a: &[&str]| plist(Value::Array(a.iter().map(|s| (*s).into()).collect()));
        assert_eq!(
            other_receivers(&flat(&["192.168.1.10", "fd00::10", "127.0.0.1"]), &sender),
            0
        );
        assert_eq!(
            other_receivers(&flat(&["192.168.1.10", "192.168.1.20"]), &sender),
            1
        );

        let peer = |a: &[&str]| {
            let mut d = Dictionary::new();
            d.insert(
                "Addresses".into(),
                Value::Array(a.iter().map(|s| (*s).into()).collect()),
            );
            Value::Dictionary(d)
        };
        let x = plist(Value::Array(vec![
            peer(&["192.168.1.10", "fe80::1"]),
            peer(&["192.168.1.30"]),
        ]));
        assert_eq!(other_receivers(&x, &sender), 1);
        assert_eq!(other_receivers(b"garbage", &sender), 0);
    }

    #[test]
    fn realtime_channels_come_from_setup() {
        let mut d = Dictionary::new();
        d.insert("ct".into(), Value::Integer(1.into()));
        d.insert("sr".into(), Value::Integer(48_000.into()));
        d.insert("spf".into(), Value::Integer(352.into()));
        d.insert("ch".into(), Value::Integer(6.into()));
        assert_eq!(realtime_format(&d).unwrap().channels, 6);
        d.insert("ch".into(), Value::Integer(258.into()));
        assert!(realtime_format(&d).is_err());
        d.insert("ch".into(), Value::Integer(3.into()));
        assert!(realtime_format(&d).is_err());
        d.insert("ct".into(), Value::Integer(4.into()));
        d.insert("ch".into(), Value::Integer(6.into()));
        assert!(realtime_format(&d).is_err());
        d.insert("ct".into(), Value::Integer(1.into()));
        d.remove("ch");
        assert_eq!(realtime_format(&d).unwrap().channels, 2);
    }

    #[test]
    fn volume_scale() {
        let mut p = Player::new(audio_stream::Config::default(), TimeSync::idle(1));
        assert_eq!(p.output_db(-15.0), -15.0);
        p.volume_range_db = 60.0;
        assert_eq!(p.output_db(-15.0), -30.0);
        assert_eq!(p.output_db(-144.0), -144.0);
        assert_eq!(p.output_db(0.0), 0.0);
        p.apply_volume = false;
        assert_eq!(p.output_db(-15.0), 0.0);
    }

    /// Новый SETUP вытесняет прежний поток, а TEARDOWN прежнего отправителя
    /// не должен останавливать новый.
    #[tokio::test]
    async fn buffered_stream_replacement_and_release() {
        let player = Player::new(
            audio_stream::Config {
                device: "null".into(),
                ..Default::default()
            },
            TimeSync::idle(1),
        );
        let first =
            Stream::buffered(player.audio.clone(), &[1; 32], Arc::new(clock::NoTime)).unwrap();
        first.playout().push(0, 0x1600_0000, vec![0; 8]);
        player.replace(1, first);
        assert_eq!(player.active_packets(), Some(1));

        let second =
            Stream::buffered(player.audio.clone(), &[2; 32], Arc::new(clock::NoTime)).unwrap();
        player.replace(2, second);
        assert_eq!(player.active_packets(), Some(0));

        player.release(1);
        assert_eq!(
            player.active_packets(),
            Some(0),
            "TEARDOWN прежнего отправителя остановил новый поток"
        );
        player.release(2);
        assert_eq!(player.active_packets(), None);
    }

    #[tokio::test]
    async fn rejected_sender_retries_after_the_owner_leaves() {
        let mut player = Player::new(audio_stream::Config::default(), TimeSync::idle(1));
        player.ownership = Ownership::Reject;
        let one =
            Stream::buffered(player.audio.clone(), &[1; 32], Arc::new(clock::NoTime)).unwrap();
        assert!(player.replace(1, one));
        let two =
            Stream::buffered(player.audio.clone(), &[2; 32], Arc::new(clock::NoTime)).unwrap();
        assert!(!player.replace(2, two), "второй отправитель отклонён");
        assert_eq!(player.active_packets(), Some(0), "прежний поток не тронут");

        // TEARDOWN от отклонённого отправителя не освобождает чужой поток.
        player.release(2);
        assert_eq!(player.active_packets(), Some(0));
        player.release(1);
        assert_eq!(player.active_packets(), None);

        // Повторная попытка после ухода владельца принимается.
        let retry =
            Stream::buffered(player.audio.clone(), &[2; 32], Arc::new(clock::NoTime)).unwrap();
        assert!(
            player.replace(2, retry),
            "после освобождения портов второй отправитель принимается"
        );
    }

    #[tokio::test]
    async fn parallel_teardowns_never_free_the_new_owner() {
        let mut player = Player::new(audio_stream::Config::default(), TimeSync::idle(1));
        player.ownership = Ownership::Takeover;
        let player = Arc::new(player);
        let one =
            Stream::buffered(player.audio.clone(), &[1; 32], Arc::new(clock::NoTime)).unwrap();
        assert!(player.replace(1, one));
        let two =
            Stream::buffered(player.audio.clone(), &[2; 32], Arc::new(clock::NoTime)).unwrap();
        assert!(player.replace(2, two));

        let barrier = Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let a = player.clone();
            let b = player.clone();
            let first_gate = barrier.clone();
            let second_gate = barrier.clone();
            scope.spawn(move || {
                first_gate.wait();
                a.release(1);
            });
            scope.spawn(move || {
                second_gate.wait();
                b.release(2);
            });
        });
        assert_eq!(
            player.active_packets(),
            None,
            "параллельные TEARDOWN не оставляют поток и не трогают чужой"
        );
    }

    #[tokio::test]
    async fn reject_policy_is_atomic_for_simultaneous_senders() {
        let mut player = Player::new(audio_stream::Config::default(), TimeSync::idle(1));
        player.ownership = Ownership::Reject;
        let player = Arc::new(player);
        let one =
            Stream::buffered(player.audio.clone(), &[1; 32], Arc::new(clock::NoTime)).unwrap();
        let two =
            Stream::buffered(player.audio.clone(), &[2; 32], Arc::new(clock::NoTime)).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let results = std::thread::scope(|scope| {
            let a = player.clone();
            let b = player.clone();
            let gate = barrier.clone();
            let first = scope.spawn(move || {
                gate.wait();
                a.replace(1, one)
            });
            let second = scope.spawn(move || {
                barrier.wait();
                b.replace(2, two)
            });
            [first.join().unwrap(), second.join().unwrap()]
        });
        assert_eq!(results.into_iter().filter(|ok| *ok).count(), 1);
    }
}

#[cfg(test)]
mod realtime_state_tests {
    use super::*;
    use codec::Codec;
    use std::time::Duration;
    #[derive(Default)]
    struct States(Mutex<Vec<bool>>);
    impl Events for States {
        fn playing(&self, value: bool) {
            self.0.lock().unwrap().push(value);
        }
    }
    #[tokio::test]
    async fn realtime_state_requires_audio_and_respects_owner() {
        let states = Arc::new(States::default());
        let mut player = Player::new(
            audio_stream::Config {
                device: "null".into(),
                ..Default::default()
            },
            TimeSync::idle(1),
        );
        player.events = states.clone();
        let stream = Stream::realtime(
            player.audio.clone(),
            StreamKey::ChaCha([7; 32]),
            Format {
                codec: Codec::Pcm {
                    frames_per_packet: 352,
                },
                sample_rate: 44100,
                channels: 2,
            },
            Arc::new(clock::NoTime),
        )
        .unwrap();
        let port = stream.data_port;
        player.replace(1, stream);
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket
            .send_to(b"invalid", ("127.0.0.1", port))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(
            states.0.lock().unwrap().is_empty(),
            "SETUP or invalid data must not announce playing"
        );
        let packet = rtp::AudioCipher::new(&[7; 32]).seal(
            &rtp::Header {
                marker: false,
                payload_type: 96,
                seq: 1,
                timestamp: 0,
                ssrc: 0,
            },
            &[0; 1408],
            1,
        );
        socket.send_to(&packet, ("127.0.0.1", port)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while states.0.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        player.release(2);
        assert_eq!(*states.0.lock().unwrap(), vec![true]);
        player.release(1);
        player.release(1);
        assert_eq!(*states.0.lock().unwrap(), vec![true, false]);
    }
}
