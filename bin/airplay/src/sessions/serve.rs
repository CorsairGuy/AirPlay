//! `airplay`: приём соединений на порту 7000 и объявление в Bonjour.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use device::{DeviceInfo, Features};
use media::control::MrpService;
#[cfg(feature = "hls")]
pub use media::hls::HlsOptions;
use media::rx::Player;
use sessions::session::Context;
use tokio::net::TcpListener;
use tracing::info;

use crate::card::Card;
use crate::identity::state;

pub struct Options {
    pub name: String,
    pub port: u16,
    pub ptp_bind: Option<std::net::IpAddr>,
    pub pin: Option<String>,
    pub state_dir: PathBuf,
    pub mdns: bool,
    pub model: String,
    pub features: Option<Features>,
    pub status_flags: Option<u32>,
    pub accessory_type: Option<u64>,
    pub audio: audio_stream::Config,
    /// Только постоянное сопряжение по PIN.
    pub require_pin: bool,
    /// Зеркалирование экрана вместо звука AirPlay 2: набор возможностей
    /// UxPlay, видео уходит в эту фабрику.
    #[cfg(feature = "mirroring")]
    pub mirror: Option<mirror::VideoSinkFactory>,
    /// HLS VOD output; absent means URL casting is unavailable.
    #[cfg(feature = "hls")]
    pub hls: Option<HlsOptions>,
    /// Экран, который объявляем для зеркалирования.
    pub display: device::Display,
    /// Ручка команд отправителю (play/pause/next, громкость, перемотка).
    pub remote: media::control::Remote,
    /// Кто отвечает iPhone по MRP (поток 130).
    pub mrp: Mrp,
    /// Что приёмник сообщает наружу (громкость, метаданные, позиция).
    pub events: Arc<dyn ::media::events::Events>,
    /// Шкала громкости, дБ (по умолчанию 30, как ползунок iPhone).
    pub volume_range_db: f32,
    /// `false` — громкость не применять, только сообщать в `events`.
    pub apply_volume: bool,
    /// `sourceVersion` в `/info` и Bonjour, например 366.0 или 550.10.
    pub source_version: String,
    /// `deviceEnclosureColor` в `/info`.
    pub enclosure_color: Option<String>,
}

/// MediaRemote: карточка «Сейчас играет» на iPhone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mrp {
    /// Поток 130 только регистрируется, карточки нет.
    #[default]
    Off,
    /// Карточка самого приёмника: трек и пульт текущего отправителя.
    Card,
    /// Расшифрованный канал — в службу по TCP (например airplay_mrp на
    /// станции, 127.0.0.1:18902): карточку собирает она.
    Bridge(std::net::SocketAddr),
}

/// `pgid` на запуск, как `generate_random_uuid()` в shairport-sync.
fn random_pgid() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

pub async fn run(opts: Options) -> Result<()> {
    let identity = state::load_identity(&opts.state_dir)?;
    let store = state::FileStore::open(&opts.state_dir)?;
    let device_id = state::device_id(&identity);
    let pin = opts
        .pin
        .unwrap_or_else(|| format!("{:04}", rand::random::<u16>() % 10_000));
    #[cfg(feature = "mirroring")]
    let mirroring = opts.mirror.is_some();
    #[cfg(not(feature = "mirroring"))]
    let mirroring = false;
    let device = DeviceInfo {
        name: opts.name.clone(),
        device_id: device_id.clone(),
        model: opts.model,
        accessory_type: opts.accessory_type,
        features: opts.features.unwrap_or({
            if mirroring {
                Features::MIRROR_RECEIVER
            } else {
                Features::AUDIO_RECEIVER
            }
        }),
        // Как у shairport на станции: «аудиокабель подключён» и доступ
        // по HomeKit (бит 10), 0x404.
        status_flags: opts.status_flags.unwrap_or(0x404),
        pairing_id: identity.pairing_id.clone(),
        public_key: identity.public_key(),
        source_version: opts.source_version,
        display: mirroring.then_some(opts.display),
        // Карточку «Сейчас играет» рисует служба MRP: без неё не обещаем.
        audio_only_ui: opts.mrp != Mrp::Off,
        group: None,
        pgid: random_pgid(),
        enclosure_color: opts.enclosure_color,
    };

    // [::] на Linux и macOS принимает и IPv4; на Windows сокет IPv6 отдельный,
    // поэтому пробуем занять и 0.0.0.0 (на Linux это просто не удастся).
    let mut listeners = Vec::new();
    for addr in ["::", "0.0.0.0"] {
        if let Ok(l) = TcpListener::bind((addr, opts.port)).await {
            listeners.push(l);
        }
    }
    if listeners.is_empty() {
        anyhow::bail!("не занять порт {}", opts.port);
    }
    info!(name = %opts.name, %device_id, port = opts.port, state = %opts.state_dir.display(), "приёмник запущен");

    if mirroring {
        info!("режим зеркалирования экрана");
    }
    // Media direction does not select the clock protocol. A combined receiver
    // owns the same PTP service for screen and audio sessions.
    let time = if device.features.0 & Features::PTP != 0 {
        if let Some(ip) = opts.ptp_bind {
            timesync::TimeSync::start_host(clock_id(&device_id), ip)
                .await
                .context("PTP host startup")?
        } else {
            timesync::TimeSync::start(clock_id(&device_id))
        }
    } else {
        timesync::TimeSync::idle(clock_id(&device_id))
    };
    info!(device = %opts.audio.device, buffer_ms = opts.audio.output_latency.as_millis(), "вывод звука");
    let mut player = Player::new(opts.audio, time);
    player.volume_range_db = opts.volume_range_db;
    player.apply_volume = opts.apply_volume;
    let (mrp, events) = match opts.mrp {
        Mrp::Off => (None, opts.events),
        Mrp::Bridge(addr) => (Some(MrpService::Bridge(addr)), opts.events),
        Mrp::Card => {
            let card = Card::new(
                Card::device(&device),
                opts.remote.clone(),
                opts.events.clone(),
            );
            // Карточка узнаёт о треке из тех же событий и передаёт их дальше.
            let events: Arc<dyn ::media::events::Events> = card.clone();
            (Some(MrpService::Handler(card)), events)
        }
    };
    player.events = events;
    let player = Arc::new(player);

    let ctx = Arc::new(Context {
        identity,
        device,
        store: Box::new(store),
        pin,
        require_pin: opts.require_pin,
        player,
        mdns: std::sync::Mutex::new(None),
        port: opts.port,
        #[cfg(feature = "mirroring")]
        mirror: opts.mirror,
        #[cfg(feature = "hls")]
        hls: opts.hls,
        remote: opts.remote,
        mrp,
        session_group: Default::default(),
    });
    if opts.mdns {
        let host = format!("airplay-{}", device_id.replace(':', "").to_lowercase());
        let advertiser =
            mdns::Advertiser::start(&host, ctx.services()).context("не запустить Bonjour")?;
        *ctx.mdns.lock().unwrap() = Some(advertiser);
    }

    for listener in listeners {
        tokio::spawn(::sessions::transport::accept_loop(listener, ctx.clone()));
    }
    tokio::signal::ctrl_c().await?;
    info!("остановка");
    Ok(())
}

/// Идентификатор часов PTP из MAC-адреса, как EUI-64: `aa:bb:cc` FF FE `dd:ee:ff`.
fn clock_id(device_id: &str) -> u64 {
    let mac: Vec<u8> = device_id
        .split(':')
        .filter_map(|b| u8::from_str_radix(b, 16).ok())
        .collect();
    if mac.len() != 6 {
        return rand::random();
    }
    let eui = [mac[0], mac[1], mac[2], 0xFF, 0xFE, mac[3], mac[4], mac[5]];
    u64::from_be_bytes(eui)
}
