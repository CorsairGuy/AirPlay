//! Отправка зеркалирования экрана: видео 110 и звук экрана 96.
//!
//! Захват, кодирование и упаковка NAL — снаружи: вызывающий передаёт готовые
//! конфигурацию кодека (`avcC`/`hvcC`) и кадры в формате AVCC, а также кадры
//! звука, уже сжатые согласованным кодеком. Ядро отвечает за согласование
//! сессии, SETUP, шифрование, отправку и остановку.
//!
//! Последовательность (наблюдена у doubletake, не нормативная спецификация):
//! pair-verify → SETUP с аудиопотоком 96 → SETUP видеопотока 110 → TCP к
//! `dataPort` → RECORD → кадры. Кадр видео: 128-байтовый заголовок и тело,
//! ChaCha20-Poly1305 с ключом HKDF-SHA512 из общего секрета pair-verify;
//! nonce — счётчик кадров, AAD — весь заголовок. Звук — RTP (PT 0x60, SSRC 0),
//! ChaCha20-Poly1305 с ключом `shk` из SETUP, nonce — счётчик, дописанный в
//! хвост пакета, AAD — timestamp и SSRC из RTP-заголовка.
//!
//! Что не входит: NTP/PTP-движки не изменяются; метки времени приходят от
//! вызывающего. FairPlay не используется: этот путь не отправляет `ekey`.
//! Совместимость с конкретной моделью Apple TV требует аппаратной проверки.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use crypto::kdf::{ControlKeys, hkdf_sha512};
use pair::hap::{Accessory, ControllerVerify};
use rand::RngCore;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tokio::task::JoinHandle;

use crate::tx::session::{Identity, PairingCredentials};
use crate::tx::transport::{Connection, ReceiverStatus, Rtsp};

struct AbortTask(JoinHandle<()>);

impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub const VIDEO_HEADER_LEN: usize = 128;
/// RTP payload type кадров звука зеркалирования.
pub const AUDIO_PAYLOAD_TYPE: u8 = 0x60;
/// RTP payload type TimeAnnounce по NTP.
pub const AUDIO_SYNC_PAYLOAD_TYPE: u8 = 0xd4;
/// Сэмплов на кадр по умолчанию для AAC-ELD.
pub const AAC_ELD_SPF: u32 = 480;
pub const AUDIO_SAMPLE_RATE: u32 = 44_100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    H265,
}

impl VideoCodec {
    /// Поле `header[6]` пакета конфигурации.
    pub fn format_option(self) -> u8 {
        match self {
            Self::H264 => 0x16,
            Self::H265 => 0x1e,
        }
    }
}

/// Как договариваемся с приёмником.
#[derive(Clone)]
pub enum MirrorAuth {
    /// Сохранённые данные PIN-сопряжения: pair-verify даёт общий секрет,
    /// управляющий канал шифруется, видео — ChaCha20-Poly1305.
    Paired(Box<PairingCredentials>),
    /// Без сопряжения и без шифрования управления и видео. Только для
    /// диагностики и тестового приёмника; Apple TV такой профиль не примет.
    Plain,
}

/// Параметры звука, которые уходят в SETUP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    /// `ct`: 2 — ALAC, 8 — AAC-ELD.
    pub ct: u8,
    /// Сэмплов в кадре: 352 для ALAC, 480 для AAC-ELD.
    pub spf: u32,
    /// `audioFormat` из SETUP (0x40000 или 0x1000000).
    pub format: u64,
    /// `latencyMax` в сэмплах.
    pub latency_samples: u32,
}

impl AudioFormat {
    pub const AAC_ELD: Self = Self {
        ct: 8,
        spf: AAC_ELD_SPF,
        format: 0x100_0000,
        latency_samples: 22_050,
    };
    pub const ALAC: Self = Self {
        ct: 2,
        spf: 352,
        format: 0x4_0000,
        latency_samples: 22_050,
    };
}

/// Что вызывающий должен подготовить заранее: адрес, identity и параметры
/// протокола. Все значения задаются снаружи.
#[derive(Clone)]
pub struct MirrorConfig {
    pub endpoint: SocketAddr,
    pub identity: Identity,
    pub auth: MirrorAuth,
    pub audio: AudioFormat,
    /// `latencyMs` видеопотока в SETUP.
    pub video_latency_ms: i64,
    pub request_timeout: Duration,
}

impl MirrorConfig {
    pub fn new(endpoint: SocketAddr, identity: Identity, auth: MirrorAuth) -> Self {
        Self {
            endpoint,
            identity,
            auth,
            audio: AudioFormat::AAC_ELD,
            video_latency_ms: 50,
            request_timeout: Duration::from_secs(8),
        }
    }
}

/// Запущенная TX-сессия зеркалирования.
///
/// Живёт на одном управляющем TCP-соединении и одном TCP-соединении видео.
/// Звук идёт по UDP. Разрыв обнаруживается по ошибке записи или закрытию
/// видеосоединения; [`MirrorSession::reconnect`] поднимает новую сессию с теми
/// же параметрами и новыми идентификаторами потоков.
pub struct MirrorSession {
    config: MirrorConfig,
    rtsp: Rtsp,
    video: TcpStream,
    audio: UdpSocket,
    audio_control: UdpSocket,
    video_key: Option<[u8; 32]>,
    /// Последняя конфигурация кодека: после переподключения её нужно
    /// отправить заново, иначе приёмник не разберёт кадры.
    video_config: Option<(VideoCodec, Vec<u8>, u64)>,
    audio_key: [u8; 32],
    video_nonce: u64,
    audio_nonce: u64,
    audio_seq: u16,
    session_uuid: String,
    ntp_task: Option<AbortTask>,
    probe_task: Option<AbortTask>,
    event_task: Option<AbortTask>,
    stopped: bool,
}

impl MirrorSession {
    /// Согласует профиль, выполняет SETUP обоих потоков, RECORD и подключает
    /// видеоканал. Готовность картинки на приёмнике это не подтверждает.
    pub async fn connect(config: MirrorConfig) -> Result<Self> {
        validate(&config)?;
        let stream =
            tokio::time::timeout(config.request_timeout, TcpStream::connect(config.endpoint))
                .await
                .context("mirror control connect deadline")?
                .context("mirror control connect")?;
        stream.set_nodelay(true)?;
        let local_ip = stream.local_addr()?.ip();
        let session_uuid = uuid_v4();
        let session_id = random_u64() & 0x7fff_ffff_ffff_ffff;
        let audio_stream_id = session_id;
        let video_stream_id = random_u64() & 0x7fff_ffff_ffff_ffff;
        let uri_host = match local_ip {
            std::net::IpAddr::V4(ip) => ip.to_string(),
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        };
        let session_uri = format!("rtsp://{uri_host}/{audio_stream_id}");
        let dacp = format!("{:016X}", random_u64());
        let mut rtsp = Rtsp::new(
            Connection::new(stream),
            session_uri.clone(),
            dacp,
            random_u64() as u32,
            config.request_timeout,
        );

        let mut video_key = None;
        let mut event_keys = None;
        let mut audio_key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut audio_key);
        match &config.auth {
            MirrorAuth::Paired(credentials) => {
                let mut verify = ControllerVerify::new(
                    credentials.controller_identity().clone(),
                    Accessory {
                        id: credentials.accessory_id().to_vec(),
                        public_key: credentials.accessory_public_key(),
                    },
                );
                let m2 = rtsp.verify_post(verify.m1()).await?;
                let m3 = verify.m3(&m2.body)?;
                let m4 = rtsp.verify_post(m3).await?;
                let keys: ControlKeys = verify.check_m4(&m4.body)?;
                let shared = *verify
                    .session_key()
                    .context("pair-verify did not produce a shared secret")?;
                event_keys = Some(ControlKeys::controller_events(&shared));
                rtsp.conn.encrypt(keys);
                video_key = Some(hkdf_sha512(
                    &shared,
                    format!("DataStream-Salt{video_stream_id}").as_bytes(),
                    b"DataStream-Output-Encryption-Key",
                ));
            }
            MirrorAuth::Plain => {}
        }

        let timing = UdpSocket::bind((local_ip, 0)).await?;
        let timing_port = timing.local_addr()?.port();
        let timing = Arc::new(timing);
        let ntp_task = Some(AbortTask(tokio::spawn(ntp_responder(timing.clone()))));

        // Сокеты звука нужны до SETUP: локальный порт RTCP уходит в дескриптор.
        let audio = UdpSocket::bind((local_ip, 0)).await?;
        let audio_control = UdpSocket::bind((local_ip, 0)).await?;
        let audio_control_port = audio_control.local_addr()?.port();

        // SETUP 1: сессия и звук экрана.
        let audio_setup = dictionary(vec![
            (
                "deviceID",
                plist::Value::String(config.identity.device_id.clone()),
            ),
            (
                "macAddress",
                plist::Value::String(config.identity.device_id.clone()),
            ),
            ("name", plist::Value::String(config.identity.name.clone())),
            ("model", plist::Value::String(config.identity.model.clone())),
            (
                "sourceVersion",
                plist::Value::String(config.identity.source_version.clone()),
            ),
            ("sessionUUID", plist::Value::String(session_uuid.clone())),
            ("timingProtocol", plist::Value::String("NTP".into())),
            (
                "timingPort",
                plist::Value::Integer((timing_port as u64).into()),
            ),
            ("isMultiSelectAirPlay", plist::Value::Boolean(true)),
            ("senderSupportsRelay", plist::Value::Boolean(false)),
            ("statsCollectionEnabled", plist::Value::Boolean(false)),
            (
                "streams",
                plist::Value::Array(vec![plist::Value::Dictionary(audio_descriptor(
                    &config.audio,
                    audio_stream_id,
                    &audio_key,
                    audio_control_port,
                )?)]),
            ),
        ]);
        let response = rtsp
            .request(
                "SETUP",
                None,
                binary_plist(audio_setup.clone())?,
                Some("application/x-apple-binary-plist"),
                &[],
            )
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error)
                if error
                    .downcast_ref::<ReceiverStatus>()
                    .is_some_and(|status| audio_setup_rejected(status.0)) =>
            {
                let mut alternate = audio_setup;
                let stream = alternate
                    .get_mut("streams")
                    .and_then(|v| v.as_array_mut())
                    .and_then(|v| v.first_mut())
                    .and_then(|v| v.as_dictionary_mut())
                    .context("audio descriptor missing")?;
                stream.remove("streamConnections");
                stream.insert(
                    "controlPort".into(),
                    plist::Value::Integer((audio_control_port as u64).into()),
                );
                rtsp.request(
                    "SETUP",
                    None,
                    binary_plist(alternate)?,
                    Some("application/x-apple-binary-plist"),
                    &[],
                )
                .await
                .context("alternate audio SETUP")?
            }
            Err(error) => return Err(error.context("audio SETUP")),
        };
        let prepared = async {
            let response = plist::Value::from_reader(std::io::Cursor::new(response.body))?;
            let mut probe_task = start_timing_probes(&response, config.endpoint, timing.clone())?;
            let mut skip_record = response
                .as_dictionary()
                .and_then(|d| d.get("skipRecord"))
                .and_then(|v| v.as_boolean())
                .unwrap_or(false);
            let mut event_task = connect_event(
                &response,
                config.endpoint,
                config.request_timeout,
                event_keys.clone(),
            )
            .await?;
            let audio_stream = response
                .as_dictionary()
                .and_then(|d| d.get("streams"))
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .ok_or_else(|| anyhow!("audio SETUP reply has no stream"))?;
            let audio_data_port = stream_port(audio_stream, "dataPort", "streamConnectionTypeRTP")?
                .ok_or_else(|| anyhow!("audio SETUP reply has no data port"))?;
            let audio_control_port =
                stream_port(audio_stream, "controlPort", "streamConnectionTypeRTCP")?
                    .ok_or_else(|| anyhow!("audio SETUP reply has no control port"))?;

            // SETUP 2: видео.
            let video_setup = dictionary(vec![(
                "streams",
                plist::Value::Array(vec![plist::Value::Dictionary(dictionary(vec![
                    ("type", plist::Value::Integer(110u64.into())),
                    (
                        "streamConnectionID",
                        plist::Value::Integer(video_stream_id.into()),
                    ),
                    (
                        "latencyMs",
                        plist::Value::Integer((config.video_latency_ms.max(0) as u64).into()),
                    ),
                    ("timestampInfo", plist::Value::Array(vec![])),
                ]))]),
            )]);
            let response = rtsp
                .request(
                    "SETUP",
                    None,
                    binary_plist(video_setup)?,
                    Some("application/x-apple-binary-plist"),
                    &[],
                )
                .await
                .context("video SETUP")?;
            let response = plist::Value::from_reader(std::io::Cursor::new(response.body))?;
            if probe_task.is_none() {
                probe_task = start_timing_probes(&response, config.endpoint, timing.clone())?;
            }
            skip_record |= response
                .as_dictionary()
                .and_then(|d| d.get("skipRecord"))
                .and_then(|v| v.as_boolean())
                .unwrap_or(false);
            if event_task.is_none() {
                event_task = connect_event(
                    &response,
                    config.endpoint,
                    config.request_timeout,
                    event_keys,
                )
                .await?;
            }
            let video_stream = response
                .as_dictionary()
                .and_then(|d| d.get("streams"))
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
                .ok_or_else(|| anyhow!("video SETUP reply has no stream"))?;
            let video_port = stream_port(video_stream, "dataPort", "streamConnectionTypeRTP")?
                .ok_or_else(|| anyhow!("video SETUP reply has no data port"))?;

            let video = tokio::time::timeout(
                config.request_timeout,
                TcpStream::connect((config.endpoint.ip(), video_port)),
            )
            .await
            .context("mirror video connect deadline")?
            .context("mirror video connect")?;
            video.set_nodelay(true)?;

            if !skip_record {
                rtsp.request(
                    "RECORD",
                    None,
                    Vec::new(),
                    None,
                    &[
                        ("Session", session_uuid.clone()),
                        ("Range", "npt=0-".into()),
                        ("RTP-Info", "seq=0;rtptime=0".into()),
                    ],
                )
                .await
                .context("RECORD")?;
            }

            audio
                .connect((config.endpoint.ip(), audio_data_port))
                .await?;
            audio_control
                .connect((config.endpoint.ip(), audio_control_port))
                .await?;

            Ok::<_, anyhow::Error>((video, probe_task, event_task))
        }
        .await;
        let (video, probe_task, event_task) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = rtsp
                    .request(
                        "TEARDOWN",
                        None,
                        Vec::new(),
                        None,
                        &[("Session", session_uuid.clone())],
                    )
                    .await;
                return Err(error);
            }
        };

        Ok(Self {
            config,
            rtsp,
            video,
            audio,
            audio_control,
            video_key,
            video_config: None,
            audio_key,
            video_nonce: 0,
            audio_nonce: 0,
            audio_seq: 0,
            session_uuid,
            ntp_task,
            probe_task,
            event_task,
            stopped: false,
        })
    }

    pub fn endpoint(&self) -> SocketAddr {
        self.config.endpoint
    }

    /// Отправляет описание кодека (`avcC` или `hvc1`/`hvcC`). Не шифруется:
    /// это пакет конфигурации, приёмник разбирает его до первого кадра.
    pub async fn send_video_config(
        &mut self,
        codec: VideoCodec,
        record: &[u8],
        ntp_timestamp: u64,
    ) -> Result<()> {
        if record.is_empty() || record.len() > 1024 * 1024 {
            bail!("invalid video configuration size");
        }
        let mut header = [0u8; VIDEO_HEADER_LEN];
        header[..4].copy_from_slice(&(record.len() as u32).to_le_bytes());
        header[4] = 0x01;
        header[5] = 0x00;
        header[6] = codec.format_option();
        header[7] = 0x01;
        header[8..16].copy_from_slice(&ntp_timestamp.to_le_bytes());
        self.write_video(&header, record).await?;
        self.video_config = Some((codec, record.to_vec(), ntp_timestamp));
        Ok(())
    }

    /// Отправляет кадр AVCC (NAL с 4-байтовой длиной). `timeline` — ноль для
    /// NTP; ненулевое значение означает PTP-идентификатор и здесь не
    /// поддерживается (движок PTP не меняется).
    pub async fn send_video_frame(
        &mut self,
        ntp_timestamp: u64,
        timeline: u64,
        keyframe: bool,
        avcc: &[u8],
    ) -> Result<()> {
        if avcc.is_empty() || avcc.len() > 16 * 1024 * 1024 {
            bail!("invalid video frame size");
        }
        if timeline != 0 {
            bail!("PTP timeline is not supported by the mirroring sender");
        }
        let nonce = self.video_nonce;
        self.video_nonce = nonce
            .checked_add(1)
            .ok_or_else(|| anyhow!("video nonce exhausted"))?;
        let sealed;
        let payload: &[u8] = match self.video_key {
            Some(key) => {
                sealed = crypto::aead::seal(
                    &key,
                    &nonce.to_le_bytes(),
                    &video_header(
                        avcc.len() + crypto::aead::TAG_LEN,
                        ntp_timestamp,
                        timeline,
                        keyframe,
                    ),
                    avcc,
                );
                &sealed
            }
            None => avcc,
        };
        let header = video_header(payload.len(), ntp_timestamp, timeline, keyframe);
        self.write_video(&header, payload).await?;
        Ok(())
    }

    /// Пульс видеоканала: раз в секунду, пока идёт поток. Приёмник считает
    /// молчание разрывом.
    pub async fn heartbeat(&mut self) -> Result<()> {
        let mut header = [0u8; VIDEO_HEADER_LEN];
        header[4] = 0x02;
        header[6] = 0x1e;
        self.write_video(&header, &[]).await
    }

    /// Один уже сжатый кадр звука (AAC-ELD или ALAC) в RTP.
    pub async fn send_audio_frame(&mut self, rtp_time: u32, encoded: &[u8]) -> Result<()> {
        if encoded.is_empty() || encoded.len() > 65_000 {
            bail!("invalid audio frame size");
        }
        let nonce = self.audio_nonce;
        self.audio_nonce = nonce
            .checked_add(1)
            .ok_or_else(|| anyhow!("audio nonce exhausted"))?;
        let mut header = [0u8; 12];
        header[0] = 0x80;
        header[1] = AUDIO_PAYLOAD_TYPE;
        header[2..4].copy_from_slice(&self.audio_seq.to_be_bytes());
        header[4..8].copy_from_slice(&rtp_time.to_be_bytes());
        // SSRC = 0, как у отправителей зеркалирования.
        let sealed = crypto::aead::seal(
            &self.audio_key,
            &nonce.to_le_bytes(),
            &header[4..12],
            encoded,
        );
        let mut packet = Vec::with_capacity(12 + sealed.len() + 8);
        packet.extend_from_slice(&header);
        packet.extend_from_slice(&sealed);
        packet.extend_from_slice(&nonce.to_le_bytes());
        self.audio.send(&packet).await?;
        self.audio_seq = self.audio_seq.wrapping_add(1);
        Ok(())
    }

    /// TimeAnnounce: соответствие позиции RTP и времени NTP. Без него
    /// приёмник не знает, когда играть звук.
    pub async fn send_audio_sync(&self, ntp_timestamp: u64, rtp_now: u32) -> Result<()> {
        let mut packet = [0u8; 20];
        packet[0] = 0x80;
        packet[1] = AUDIO_SYNC_PAYLOAD_TYPE;
        packet[2..4].copy_from_slice(&4u16.to_be_bytes());
        packet[4..8].copy_from_slice(&rtp_now.to_be_bytes());
        packet[8..16].copy_from_slice(&ntp_timestamp.to_be_bytes());
        packet[16..20].copy_from_slice(&rtp_now.to_be_bytes());
        self.audio_control.send(&packet).await?;
        Ok(())
    }

    /// Приёмник закрыл видеоканал или запись больше не проходит.
    pub fn is_disconnected(&mut self) -> bool {
        let mut probe = [0u8; 1];
        match self.video.try_read(&mut probe) {
            Ok(0) => true,
            Ok(_) => false,
            Err(error) => !matches!(error.kind(), std::io::ErrorKind::WouldBlock),
        }
    }

    /// Поднимает сессию заново с теми же параметрами: новые идентификаторы
    /// потоков, новый SETUP и RECORD. Старая сессия закрывается без ошибки,
    /// если приёмник уже её потерял.
    pub async fn reconnect(&mut self) -> Result<()> {
        let _ = self.teardown().await;
        let config = self.video_config.clone();
        let mut replacement = Self::connect(self.config.clone()).await?;
        if let Some((codec, record, pts)) = config {
            replacement.send_video_config(codec, &record, pts).await?;
        }
        *self = replacement;
        Ok(())
    }

    /// Корректная остановка: TEARDOWN, затем закрытие каналов.
    pub async fn stop(mut self) -> Result<()> {
        self.teardown().await
    }

    async fn write_video(&mut self, header: &[u8; VIDEO_HEADER_LEN], payload: &[u8]) -> Result<()> {
        if self.stopped {
            bail!("mirror session is stopped");
        }
        if let Err(error) = self.video.write_all(header).await {
            self.stopped = true;
            return Err(error.into());
        }
        if let Err(error) = self.video.write_all(payload).await {
            self.stopped = true;
            return Err(error.into());
        }
        Ok(())
    }

    async fn teardown(&mut self) -> Result<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        if let Some(task) = self.ntp_task.take() {
            drop(task);
        }
        self.event_task.take();
        self.probe_task.take();
        let result = self
            .rtsp
            .request(
                "TEARDOWN",
                None,
                Vec::new(),
                None,
                &[("Session", self.session_uuid.clone())],
            )
            .await;
        let _ = self.video.shutdown().await;
        result.map(|_| ()).context("TEARDOWN")
    }
}

impl Drop for MirrorSession {
    fn drop(&mut self) {
        if let Some(task) = self.ntp_task.take() {
            drop(task);
        }
    }
}

async fn connect_event(
    response: &plist::Value,
    endpoint: SocketAddr,
    deadline: Duration,
    keys: Option<ControlKeys>,
) -> Result<Option<AbortTask>> {
    let Some(value) = response.as_dictionary().and_then(|d| d.get("eventPort")) else {
        return Ok(None);
    };
    let port = value
        .as_unsigned_integer()
        .and_then(|v| u16::try_from(v).ok())
        .context("invalid eventPort")?;
    if port == 0 {
        return Ok(None);
    }
    let socket = tokio::time::timeout(deadline, TcpStream::connect((endpoint.ip(), port)))
        .await
        .context("event connect deadline")?
        .context("event connect")?;
    let mut conn = Connection::new(socket);
    if let Some(keys) = keys {
        conn.encrypt(keys);
    }
    Ok(Some(AbortTask(tokio::spawn(async move {
        while let Ok(request) = conn.event_request().await {
            if request.headers.get("CSeq").is_none() {
                break;
            }
            let response = rtsp::Response::to(&request, 200);
            let mut bytes = Vec::new();
            response.encode(&mut bytes);
            if conn.send(&bytes).await.is_err() {
                break;
            }
        }
    }))))
}

fn start_timing_probes(
    response: &plist::Value,
    endpoint: SocketAddr,
    socket: Arc<UdpSocket>,
) -> Result<Option<AbortTask>> {
    let Some(value) = response.as_dictionary().and_then(|d| d.get("timingPort")) else {
        return Ok(None);
    };
    let port = value
        .as_unsigned_integer()
        .and_then(|v| u16::try_from(v).ok())
        .context("invalid timingPort")?;
    if port == 0 {
        return Ok(None);
    }
    let peer = SocketAddr::new(endpoint.ip(), port);
    Ok(Some(AbortTask(tokio::spawn(async move {
        for sequence in 1..=3 {
            if socket
                .send_to(&timesync::ntp::request_with_sequence(sequence), peer)
                .await
                .is_err()
            {
                break;
            }
            if sequence != 3 {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }))))
}

fn validate(config: &MirrorConfig) -> Result<()> {
    if config.endpoint.port() == 0 {
        bail!("mirror receiver port must not be zero");
    }
    if config.identity.device_id.is_empty()
        || config.identity.name.is_empty()
        || config.identity.model.is_empty()
        || config.identity.source_version.is_empty()
    {
        bail!("mirror identity fields must not be empty");
    }
    if config.request_timeout.is_zero() || config.request_timeout > Duration::from_secs(30) {
        bail!("mirror request timeout must be between 0 and 30 seconds");
    }
    if config.audio.spf == 0 || config.audio.spf > 4096 {
        bail!("audio samples per frame must be 1..4096");
    }
    Ok(())
}

fn video_header(len: usize, ntp: u64, timeline: u64, keyframe: bool) -> [u8; VIDEO_HEADER_LEN] {
    let mut header = [0u8; VIDEO_HEADER_LEN];
    header[..4].copy_from_slice(&(len as u32).to_le_bytes());
    header[4] = 0x00;
    header[5] = if keyframe { 0x10 } else { 0x00 };
    header[8..16].copy_from_slice(&ntp.to_le_bytes());
    header[40..48].copy_from_slice(&timeline.to_le_bytes());
    header
}

fn audio_descriptor(
    format: &AudioFormat,
    stream_id: u64,
    key: &[u8; 32],
    control_port: u16,
) -> Result<plist::Dictionary> {
    Ok(dictionary(vec![
        ("type", plist::Value::Integer(96u64.into())),
        (
            "streamConnectionID",
            plist::Value::Integer(stream_id.into()),
        ),
        ("ct", plist::Value::Integer((format.ct as u64).into())),
        ("spf", plist::Value::Integer((format.spf as u64).into())),
        (
            "sr",
            plist::Value::Integer((AUDIO_SAMPLE_RATE as u64).into()),
        ),
        ("audioFormat", plist::Value::Integer(format.format.into())),
        ("audioMode", plist::Value::String("default".into())),
        ("usingScreen", plist::Value::Boolean(true)),
        ("isMedia", plist::Value::Boolean(false)),
        ("supportsDynamicStreamID", plist::Value::Boolean(true)),
        ("latencyMin", plist::Value::Integer(0u64.into())),
        (
            "latencyMax",
            plist::Value::Integer((format.latency_samples as u64).into()),
        ),
        ("shk", plist::Value::Data(key.to_vec())),
        (
            "streamConnections",
            plist::Value::Dictionary(dictionary(vec![
                (
                    "streamConnectionTypeRTP",
                    plist::Value::Dictionary(dictionary(vec![(
                        "streamConnectionKeyUseStreamEncryptionKey",
                        plist::Value::Boolean(true),
                    )])),
                ),
                (
                    "streamConnectionTypeRTCP",
                    plist::Value::Dictionary(dictionary(vec![(
                        "streamConnectionKeyPort",
                        plist::Value::Integer((control_port as u64).into()),
                    )])),
                ),
            ])),
        ),
    ]))
}

fn stream_port(stream: &plist::Value, legacy: &str, connection: &str) -> Result<Option<u16>> {
    let Some(d) = stream.as_dictionary() else {
        return Ok(None);
    };
    if let Some(port) = d.get(legacy).and_then(|v| v.as_unsigned_integer()) {
        return Ok(Some(u16::try_from(port).context("port out of range")?));
    }
    let Some(port) = d
        .get("streamConnections")
        .and_then(|v| v.as_dictionary())
        .and_then(|v| v.get(connection))
        .and_then(|v| v.as_dictionary())
        .and_then(|v| v.get("streamConnectionKeyPort"))
        .and_then(|v| v.as_unsigned_integer())
    else {
        return Ok(None);
    };
    Ok(Some(u16::try_from(port).context("port out of range")?))
}

fn dictionary(items: Vec<(&str, plist::Value)>) -> plist::Dictionary {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn binary_plist(value: plist::Dictionary) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    plist::Value::Dictionary(value).to_writer_binary(&mut bytes)?;
    Ok(bytes)
}

fn random_u64() -> u64 {
    rand::random()
}

fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: Vec<String> = b.iter().map(|byte| format!("{byte:02x}")).collect();
    let h = h.concat();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// Минимальный NTP-ответчик на `timingPort`: приёмник проверяет по нему
/// доступность часов отправителя. Движок NTP не изменяется.
async fn ntp_responder(socket: Arc<UdpSocket>) {
    let mut buf = [0u8; 128];
    loop {
        let (n, peer) = match socket.recv_from(&mut buf).await {
            Ok(value) => value,
            Err(_) => return,
        };
        if let Some(reply) = timesync::ntp::reply(&buf[..n]) {
            let _ = socket.send_to(&reply, peer).await;
            continue;
        }
        if n < 48 || buf[0] & 0x07 != 3 {
            continue;
        }
        let now = timesync::ntp::now();
        let mut reply = [0u8; 48];
        reply[0] = 0x24;
        reply[1] = 1;
        reply[2] = buf[2];
        reply[12..16].copy_from_slice(b"APL\0");
        reply[16..24].copy_from_slice(&now.to_be_bytes());
        reply[24..32].copy_from_slice(&buf[40..48]);
        reply[32..40].copy_from_slice(&now.to_be_bytes());
        reply[40..48].copy_from_slice(&now.to_be_bytes());
        let _ = socket.send_to(&reply, peer).await;
    }
}

/// Протокольные отказы, после которых doubletake пробует ровно один
/// альтернативный audio-дескриптор. Остальные ответы не меняют форму SETUP.
fn audio_setup_rejected(status: u16) -> bool {
    matches!(status, 400 | 406 | 415 | 455 | 500 | 501)
}

#[cfg(test)]
mod event_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn zero_optional_ports_are_ignored() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let endpoint = socket.local_addr().unwrap();
        let response = plist::Value::Dictionary(dictionary(vec![
            ("eventPort", plist::Value::Integer(0u64.into())),
            ("timingPort", plist::Value::Integer(0u64.into())),
        ]));
        assert!(
            connect_event(&response, endpoint, Duration::from_secs(1), None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            start_timing_probes(&response, endpoint, socket)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn encrypted_event_requests_are_acknowledged() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let shared = [42u8; 32];
        let response = plist::Value::Dictionary(dictionary(vec![(
            "eventPort",
            plist::Value::Integer((endpoint.port() as u64).into()),
        )]));
        let task = connect_event(
            &response,
            endpoint,
            Duration::from_secs(2),
            Some(ControlKeys::controller_events(&shared)),
        )
        .await
        .unwrap()
        .unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut channel = crypto::Channel::new(ControlKeys::accessory_events(&shared));
        let mut request = rtsp::Request::new("POST", "/command", "RTSP/1.0");
        request.headers.insert("CSeq", "17");
        let mut plain = Vec::new();
        request.encode(&mut plain);
        let mut wire = Vec::new();
        channel.seal(&plain, &mut wire).unwrap();
        socket.write_all(&wire).await.unwrap();
        let mut size = [0u8; 2];
        tokio::time::timeout(Duration::from_secs(2), socket.read_exact(&mut size))
            .await
            .unwrap()
            .unwrap();
        let mut raw = size.to_vec();
        raw.resize(2 + u16::from_le_bytes(size) as usize + 16, 0);
        socket.read_exact(&mut raw[2..]).await.unwrap();
        let mut answer = Vec::new();
        channel.open(&mut raw, &mut answer).unwrap();
        let reply = rtsp::decode_response(&mut answer).unwrap().unwrap();
        assert_eq!(reply.status, 200);
        assert_eq!(reply.headers.get("CSeq"), Some("17"));
        drop(task);
    }

    #[test]
    fn only_documented_protocol_rejections_request_an_alternate_descriptor() {
        for status in [400, 406, 415, 455, 500, 501] {
            assert!(audio_setup_rejected(status), "status {status}");
        }
        for status in [200, 204, 401, 403, 404, 409, 503] {
            assert!(!audio_setup_rejected(status), "status {status}");
        }
    }

    #[tokio::test]
    async fn timing_responder_supports_airplay_requests() {
        let timing = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let destination = timing.local_addr().unwrap();
        let task = AbortTask(tokio::spawn(ntp_responder(Arc::new(timing))));
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut request = [0u8; 32];
        request[..4].copy_from_slice(&[0x80, 0xd2, 0, 7]);
        request[24..32].copy_from_slice(&123456u64.to_be_bytes());
        peer.send_to(&request, destination).await.unwrap();
        let mut response = [0u8; 64];
        let (len, _) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(len, 32);
        assert_eq!(response[1], 0xd3);
        assert_eq!(&response[8..16], &request[24..32]);
        drop(task);
    }

    #[tokio::test]
    async fn timing_probes_use_advertised_port_and_shared_clock() {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let endpoint = peer.local_addr().unwrap();
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let source = socket.local_addr().unwrap();
        let response = plist::Value::Dictionary(dictionary(vec![(
            "timingPort",
            plist::Value::Integer((endpoint.port() as u64).into()),
        )]));
        let task = start_timing_probes(&response, endpoint, socket)
            .unwrap()
            .unwrap();
        for sequence in 1u16..=3 {
            let mut packet = [0u8; 64];
            let (n, from) =
                tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut packet))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!((n, from), (32, source));
            assert_eq!(&packet[..2], &[0x80, 0xd2]);
            assert_eq!(u16::from_be_bytes([packet[2], packet[3]]), sequence);
            assert_ne!(&packet[24..32], &[0; 8]);
        }
        drop(task);
    }

    /// Порт события/времени вне u16 — ошибка конфигурации, а не паника.
    #[tokio::test]
    async fn optional_ports_outside_u16_are_rejected() {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let endpoint = socket.local_addr().unwrap();
        let response = plist::Value::Dictionary(dictionary(vec![
            ("eventPort", plist::Value::Integer(70_000u64.into())),
            ("timingPort", plist::Value::Integer(70_000u64.into())),
        ]));
        assert!(
            connect_event(&response, endpoint, Duration::from_secs(1), None)
                .await
                .is_err()
        );
        assert!(start_timing_probes(&response, endpoint, socket).is_err());
    }

    /// Запрос без CSeq завершает обслуживание event-канала и закрывает сокет,
    /// а не оставляет висящую задачу.
    #[tokio::test]
    async fn event_channel_stops_on_a_request_without_cseq() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let response = plist::Value::Dictionary(dictionary(vec![(
            "eventPort",
            plist::Value::Integer((endpoint.port() as u64).into()),
        )]));
        let task = connect_event(&response, endpoint, Duration::from_secs(2), None)
            .await
            .unwrap()
            .unwrap();
        let (mut socket, _) = listener.accept().await.unwrap();

        let request = rtsp::Request::new("POST", "/command", "RTSP/1.0");
        let mut plain = Vec::new();
        request.encode(&mut plain);
        socket.write_all(&plain).await.unwrap();

        let mut byte = [0u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut byte))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read, 0, "event-канал должен закрыться");
        drop(task);
    }

    /// Обрыв event-соединения со стороны приёмника не паникует и не мешает
    /// освободить задачу.
    #[tokio::test]
    async fn event_channel_tolerates_the_peer_closing() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let response = plist::Value::Dictionary(dictionary(vec![(
            "eventPort",
            plist::Value::Integer((endpoint.port() as u64).into()),
        )]));
        let task = connect_event(&response, endpoint, Duration::from_secs(2), None)
            .await
            .unwrap()
            .unwrap();
        let (socket, _) = listener.accept().await.unwrap();
        drop(socket);
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(task);
    }
}
