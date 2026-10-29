//! Медиачасть RTSP-сессии AirPlay 2: FairPlay, SETUP потоков, якорь
//! времени, перемотка, громкость, канал событий.
//!
//! Последовательность — по shairplay-rust и shairport-sync: первый SETUP
//! без `streams` (часы и канал событий), затем SETUP потока, RECORD и
//! SETRATEANCHORTIME, когда пора играть.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

use crate::events::{Events, Metadata, NoEvents, Progress};
use crate::remote::{self, Command, Remote};
use crate::session::{BPLIST, Context, OCTETS};

/// Сколько байт отправитель может прислать вперёд в буферизованном потоке:
/// 1 МиБ — около 30 с AAC 256 кбит/с.
const AUDIO_BUFFER_SIZE: u64 = 1 << 20;

/// Звук играет один отправитель: новый поток вытесняет прежний.
pub struct Player {
    pub audio: audio_stream::Config,
    pub time: TimeSync,
    /// Шкала громкости: ползунок iPhone (-30..0 дБ) растягивается на
    /// столько дБ (как `volume_range_db` в shairport-sync).
    pub volume_range_db: f32,
    /// Громкость не применять к звуку, только сообщать в [`Events`]: её
    /// ведёт внешний микшер.
    pub apply_volume: bool,
    pub events: Arc<dyn Events>,
    active: Mutex<Option<(u64, Stream)>>,
    /// Громкость AirPlay, как прислал отправитель.
    volume_db: Mutex<f32>,
    next_id: AtomicU64,
}

impl Player {
    pub fn new(audio: audio_stream::Config, time: TimeSync) -> Self {
        Self {
            audio,
            time,
            volume_range_db: 30.0,
            apply_volume: true,
            events: Arc::new(NoEvents),
            active: Mutex::new(None),
            volume_db: Mutex::new(0.0),
            next_id: AtomicU64::new(1),
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

    fn replace(&self, owner: u64, stream: Stream) {
        stream
            .playout()
            .set_volume(self.output_db(*self.volume_db.lock().unwrap_or_else(|e| e.into_inner())));
        if let Some((old, _)) = self.lock().replace((owner, stream))
            && old != owner
        {
            info!("новый отправитель, прежний поток остановлен");
        }
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
            *active = None;
        }
    }

    fn set_volume(&self, owner: u64, db: f32) {
        *self.volume_db.lock().unwrap_or_else(|e| e.into_inner()) = db;
        let out = self.output_db(db);
        self.with(owner, |s| s.playout().set_volume(out));
    }

    #[cfg(test)]
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
    mirror: Option<mirror::VideoSinkFactory>,
    /// Ключ AES из FairPlay `ekey` и IV `eiv` (зеркалирование).
    aes_key: Option<[u8; 16]>,
    eiv: Option<[u8; 16]>,
    /// Часы отправителя по NTP (зеркалирование).
    ntp: Option<timesync::ntp::NtpClock>,
    /// Общий секрет legacy pair-verify: подмешивается в ключ видео.
    legacy_shared: Option<[u8; 32]>,
    /// Часы NTP, видеопоток и заглушка звука зеркалирования.
    tasks: Vec<JoinHandle<()>>,
    /// Команды отправителю идут через канал событий этого соединения.
    control: Remote,
    /// Соединение только для пульта (`isRemoteControlOnly`): звука в нём
    /// нет, команды через него не шлём.
    rc_only: bool,
    /// Куда пересылать канал MRP (поток 130).
    mrp_bridge: Option<SocketAddr>,
    mrp: Option<JoinHandle<()>>,
    /// Адреса отправителя из `timingPeerInfo`.
    sender_addrs: Vec<IpAddr>,
    /// В сессии есть другие приёмники (мультирум).
    grouped: bool,
}

impl Media {
    pub fn new(ctx: &Context, local: IpAddr, remote: IpAddr) -> Self {
        Self {
            player: ctx.player.clone(),
            id: ctx.player.next_id.fetch_add(1, Ordering::Relaxed),
            local: local.to_canonical(),
            shared: None,
            fairplay: FairPlay::new(),
            events: None,
            anchor: None,
            remote: remote.to_canonical(),
            mirror: ctx.mirror.clone(),
            aes_key: None,
            eiv: None,
            ntp: None,
            legacy_shared: None,
            tasks: Vec::new(),
            control: ctx.remote.clone(),
            rc_only: false,
            mrp_bridge: ctx.mrp_bridge,
            mrp: None,
            sender_addrs: vec![remote.to_canonical()],
            grouped: false,
        }
    }

    pub fn set_legacy_shared(&mut self, shared: [u8; 32]) {
        self.legacy_shared = Some(shared);
    }

    pub fn set_shared(&mut self, shared: Vec<u8>) {
        self.shared = Some(shared);
    }

    /// Запрос медиачасти; `None` — не наш.
    pub fn handle(&mut self, ctx: &Context, req: &Request) -> Option<Response> {
        let ok = || Response::to(req, 200);
        Some(match (req.method.as_str(), req.path()) {
            ("POST", "/fp-setup") => match self.fairplay.handle(&req.body) {
                Ok(body) => ok().with_body(OCTETS, body),
                Err(e) => {
                    warn!("fp-setup: {e}");
                    Response::to(req, 400)
                }
            },
            ("SETUP", _) => self.setup(ctx, req),
            ("RECORD", _) => {
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
            ("FLUSH", _) => ok(),
            ("SETPEERS", _) | ("SETPEERSX", _) => {
                let peers = parse_peers(&req.body);
                let grouped = other_receivers(&req.body, &self.sender_addrs) > 0;
                self.grouped = grouped;
                self.player
                    .with(self.id, |s| s.playout().set_grouped(grouped));
                self.player.time.set_peers(peers);
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
                let streams_only = plist_dict(&req.body).is_some_and(|d| d.contains_key("streams"));
                self.player.release(self.id);
                if self.anchor.take().is_some_and(|a| a.playing) {
                    self.player.events.playing(false);
                }
                if !streams_only {
                    if let Some(events) = self.events.take() {
                        events.abort();
                    }
                    if let Some(mrp) = self.mrp.take() {
                        mrp.abort();
                    }
                    self.stop_tasks();
                }
                info!("TEARDOWN");
                ok()
            }
            ("POST", "/feedback") | ("POST", "/command") | ("POST", "/audioMode") => ok(),
            _ => return None,
        })
    }

    fn setup(&mut self, ctx: &Context, req: &Request) -> Response {
        let Some(body) = plist_dict(&req.body) else {
            warn!("SETUP без plist: отправители AirPlay 1 пока не поддерживаются");
            return Response::to(req, 400);
        };
        let reply = match body.get("streams").and_then(Value::as_array) {
            Some(streams) => self.setup_streams(streams),
            None => Ok(self.setup_session(ctx, &body)),
        };
        match reply {
            Ok(d) => Response::to(req, 200).with_body(BPLIST, write_plist(d)),
            Err(e) => {
                warn!("SETUP: {e}");
                Response::to(req, 500)
            }
        }
    }

    /// Первый SETUP: часы и канал событий.
    fn setup_session(&mut self, ctx: &Context, body: &Dictionary) -> Dictionary {
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
        if let Some(ekey) = body.get("ekey").and_then(Value::as_data) {
            match <&[u8; 72]>::try_from(ekey).map(|e| self.fairplay.decrypt(e)) {
                Ok(Ok(key)) => self.aes_key = Some(key),
                Ok(Err(e)) => warn!("ekey: {e}"),
                Err(_) => warn!(len = ekey.len(), "ekey неожиданной длины"),
            }
        }
        if let Some(eiv) = body.get("eiv").and_then(Value::as_data) {
            self.eiv = eiv.try_into().ok();
        }
        if timing == "NTP" {
            let port = uint(body, "timingPort").unwrap_or(0) as u16;
            match timesync::ntp::start(SocketAddr::new(self.remote, port)) {
                Ok((local, clock, task)) => {
                    self.tasks.push(task);
                    self.ntp = Some(clock);
                    d.insert("timingPort".into(), Value::Integer(local.into()));
                }
                Err(e) => warn!("NTP: {e}"),
            }
        }
        if timing == "PTP" {
            if let Some(info) = body.get("timingPeerInfo").and_then(Value::as_dictionary) {
                // Адреса самого отправителя: в SETPEERS они не «другие приёмники».
                self.sender_addrs = addresses(info);
                self.sender_addrs.push(self.remote);
                self.player.time.set_peers(addresses(info));
            }
            let ip = self.local.to_string();
            let mut peer = Dictionary::new();
            peer.insert("Addresses".into(), Value::Array(vec![ip.clone().into()]));
            peer.insert("ID".into(), ip.into());
            d.insert("timingPeerInfo".into(), peer.into());
        }
        let port = self.start_events(ctx).unwrap_or(0);
        d.insert("eventPort".into(), Value::Integer(port.into()));
        if !d.contains_key("timingPort") {
            d.insert("timingPort".into(), Value::Integer(0.into()));
        }
        d
    }

    fn stop_tasks(&mut self) {
        for t in self.tasks.drain(..) {
            t.abort();
        }
    }

    /// Часы отправителя для зеркалирования: NTP, если обмен метками начат.
    fn time_source(&self) -> Arc<dyn TimeSource> {
        match &self.ntp {
            Some(clock) => Arc::new(clock.clone()),
            None => Arc::new(clock::NoTime),
        }
    }

    /// Тип 110: видеопоток зеркалирования.
    fn setup_video(&mut self, s: &Dictionary, r: &mut Dictionary) -> Result<(), String> {
        let factory = self
            .mirror
            .clone()
            .ok_or("зеркалирование выключено (--mirror)")?;
        let aes = self.aes_key.ok_or("нет ключа видео: SETUP без ekey")?;
        let id = s
            .get("streamConnectionID")
            .and_then(|v| {
                v.as_unsigned_integer()
                    .or(v.as_signed_integer().map(|i| i as u64))
            })
            .unwrap_or(0);
        let session = mirror::session_key(&aes, self.legacy_shared.as_ref());
        let (key, iv) = mirror::stream_key_iv(&session, id);
        let listener = audio_stream::bind_tcp().map_err(|e| format!("видеосокет: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        info!(id, port, "видеопоток");
        let info = mirror::StreamInfo {
            time: self.time_source(),
        };
        self.tasks.push(tokio::spawn(mirror::receive(
            listener,
            key,
            iv,
            factory.open(&info),
        )));
        r.insert("dataPort".into(), Value::Integer(port.into()));
        Ok(())
    }

    /// Звук зеркалирования: тип 96 без `shk`, AAC-ELD, AES-128-CBC ключом
    /// сессии и `eiv`, время по NTP. Без декодера AAC-ELD (сборка без фичи
    /// `ffmpeg`) порты всё равно отдаём, иначе отправитель не начнёт видео.
    fn setup_mirror_audio(&mut self, s: &Dictionary, r: &mut Dictionary) -> Result<(), String> {
        let format = Format {
            codec: Codec::AacEld,
            sample_rate: uint(s, "sr").unwrap_or(44_100) as u32,
            channels: 2,
        };
        if let Err(e) = codec::Decoder::new(format) {
            warn!("звук зеркалирования не играет: {e}");
            return self.drain_mirror_audio(r);
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
        self.player.replace(self.id, stream);
        Ok(())
    }

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

    /// Канал событий: отправитель подключается сам; первым сообщением
    /// сообщаем о себе (`updateInfo`) — без него iOS дольше подключается.
    fn start_events(&mut self, ctx: &Context) -> Option<u16> {
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
        let mut hello = Vec::new();
        channel.seal(&update_info(ctx), &mut hello);
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
            if conn.write_all(&hello).await.is_err() {
                return;
            }
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
                        let req = remote::command_request(remote::command_body(command), cseq);
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

    fn setup_streams(&mut self, streams: &[Value]) -> Result<Dictionary, String> {
        let mut replies = Vec::new();
        for s in streams {
            let Some(s) = s.as_dictionary() else { continue };
            let kind = uint(s, "type").unwrap_or(0);
            let mut r = Dictionary::new();
            r.insert("type".into(), Value::Integer(kind.into()));
            match kind {
                96 if self.mirror.is_some() && !s.contains_key("shk") => {
                    self.setup_mirror_audio(s, &mut r)?;
                }
                110 => self.setup_video(s, &mut r)?,
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
                        let format = realtime_format(s)?;
                        info!(?format, "realtime-поток");
                        Stream::realtime(
                            self.player.audio.clone(),
                            StreamKey::ChaCha(shk),
                            format,
                            self.player.time_source(),
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
                    self.player.replace(self.id, stream);
                }
                130 => self.setup_remote_control(s, &mut r)?,
                other => {
                    warn!(kind = other, "поток не поддерживается");
                    continue;
                }
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
        let (Some(seed), Some(backend)) = (seed, self.mrp_bridge) else {
            debug!(seed = seed.is_some(), "MRP: логическая регистрация");
            return Ok(());
        };
        let shared = self.shared.as_ref().ok_or("канал MRP без сопряжения")?;
        let listener = audio_stream::bind_tcp().map_err(|e| format!("сокет MRP: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        if let Some(old) = self.mrp.take() {
            old.abort();
        }
        self.mrp = Some(tokio::spawn(remote::bridge(
            listener,
            self.remote,
            ControlKeys::accessory_data(shared, seed),
            backend,
        )));
        info!(port, %backend, "канал MRP");
        r.insert("dataPort".into(), Value::Integer(port.into()));
        Ok(())
    }

    fn set_anchor(&mut self, req: &Request) {
        let Some(d) = plist_dict(&req.body) else {
            return;
        };
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
        let Some(until) = uint(&d, "flushUntilTS") else {
            return;
        };
        let from = uint(&d, "flushFromTS").map(|t| t as u32);
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
        self.player.release(self.id);
        self.stop_tasks();
        if let Some(events) = self.events.take() {
            events.abort();
        }
        if let Some(mrp) = self.mrp.take() {
            mrp.abort();
        }
        self.control.detach(self.id);
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
    let sample_rate = uint(s, "sr").unwrap_or(44_100) as u32;
    let frames = uint(s, "spf").unwrap_or(352) as u32;
    let bits = uint(s, "ss").unwrap_or(16) as u8;
    let codec = match uint(s, "ct").unwrap_or(2) {
        2 => Codec::Alac {
            bit_depth: bits,
            frames_per_packet: frames,
        },
        4 => Codec::Aac,
        8 => return Err("AAC-ELD пока не поддерживается".into()),
        other => return Err(format!("неизвестный кодек ct={other}")),
    };
    Ok(Format {
        codec,
        sample_rate,
        channels: 2,
    })
}

fn update_info(ctx: &Context) -> Vec<u8> {
    let dev = &ctx.device();
    let mut value = Dictionary::new();
    value.insert(
        "statusFlags".into(),
        Value::Integer(dev.status_flags.into()),
    );
    value.insert(
        "features".into(),
        Value::Integer((dev.features.0 as i64).into()),
    );
    value.insert("model".into(), dev.model.clone().into());
    value.insert("sourceVersion".into(), dev.source_version.clone().into());
    value.insert(
        "protocolVersion".into(),
        device::DeviceInfo::PROTOCOL_VERSION.into(),
    );
    let mut msg = Dictionary::new();
    msg.insert("type".into(), "updateInfo".into());
    msg.insert("value".into(), value.into());
    let body = write_plist(msg);
    let mut req = Request::new("POST", "/command", "RTSP/1.0");
    req.headers.insert("CSeq", "0");
    req.headers.insert("Content-Type", BPLIST);
    req.body = body;
    let mut out = Vec::new();
    req.encode(&mut out);
    out
}

fn plist_dict(body: &[u8]) -> Option<Dictionary> {
    if body.is_empty() {
        return None;
    }
    plist::from_bytes::<Value>(body).ok()?.into_dictionary()
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

fn addresses(d: &Dictionary) -> Vec<IpAddr> {
    d.get("Addresses")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_string()?.parse().ok())
        .collect()
}

/// Сколько в SETPEERS/SETPEERSX приёмников, кроме нас и отправителя.
/// SETPEERS — плоский список адресов, SETPEERSX — участник на словарь.
fn other_receivers(body: &[u8], sender: &[IpAddr]) -> usize {
    let Ok(Value::Array(items)) = plist::from_bytes::<Value>(body) else {
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
    let Ok(Value::Array(items)) = plist::from_bytes::<Value>(body) else {
        return Vec::new();
    };
    let mut peers = Vec::new();
    for item in items {
        match item {
            Value::String(s) => peers.extend(s.parse::<IpAddr>().ok()),
            Value::Dictionary(d) => peers.extend(addresses(&d)),
            _ => {}
        }
    }
    peers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plist(v: Value) -> Vec<u8> {
        let mut out = Vec::new();
        v.to_writer_binary(&mut out).unwrap();
        out
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
}
