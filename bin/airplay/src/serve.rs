//! `airplay serve`: приём соединений на порту 7000 и объявление в Bonjour.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use device::{DeviceInfo, Features};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

use crate::media::Player;
use crate::session::{Context, Session};
use crate::state;

pub struct Options {
    pub name: String,
    pub port: u16,
    pub pin: Option<String>,
    pub state_dir: PathBuf,
    pub mdns: bool,
    pub model: String,
    pub audio: audio_stream::Config,
    /// Только постоянное сопряжение по PIN.
    pub require_pin: bool,
    /// Зеркалирование экрана вместо звука AirPlay 2: набор возможностей
    /// UxPlay, видео уходит в эту фабрику.
    pub mirror: Option<mirror::VideoSinkFactory>,
    /// Экран, который объявляем для зеркалирования.
    pub display: device::Display,
    /// Ручка команд отправителю (play/pause/next, громкость, перемотка).
    pub remote: crate::remote::Remote,
    /// Служба MRP (например airplay_mrp на 127.0.0.1:18902): туда идёт
    /// расшифрованный канал потока 130. `None` — поток 130 только
    /// регистрируется.
    pub mrp_bridge: Option<std::net::SocketAddr>,
}

pub async fn run(opts: Options) -> Result<()> {
    let identity = state::load_identity(&opts.state_dir)?;
    let store = state::FileStore::open(&opts.state_dir)?;
    let device_id = state::device_id(&identity);
    let pin = opts
        .pin
        .unwrap_or_else(|| format!("{:04}", rand::random::<u16>() % 10_000));
    let mirroring = opts.mirror.is_some();
    let device = DeviceInfo {
        name: opts.name.clone(),
        device_id: device_id.clone(),
        model: opts.model,
        features: if mirroring {
            Features::MIRROR_RECEIVER
        } else {
            Features::AUDIO_RECEIVER
        },
        status_flags: 0x4,
        pairing_id: identity.pairing_id.clone(),
        public_key: identity.public_key(),
        source_version: "366.0".into(),
        display: mirroring.then_some(opts.display),
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

    // Зеркалирование идёт по часам NTP, PTP ему не нужен (и не нужен root).
    let time = if mirroring {
        info!("режим зеркалирования экрана");
        timesync::TimeSync::idle(clock_id(&device_id))
    } else {
        timesync::TimeSync::start(clock_id(&device_id))
    };
    info!(device = %opts.audio.device, buffer_ms = opts.audio.output_latency.as_millis(), "вывод звука");
    let player = Arc::new(Player::new(opts.audio, time));

    let ctx = Arc::new(Context {
        identity,
        device,
        store: Box::new(store),
        pin,
        require_pin: opts.require_pin,
        player,
        mdns: std::sync::Mutex::new(None),
        port: opts.port,
        mirror: opts.mirror,
        remote: opts.remote,
        mrp_bridge: opts.mrp_bridge,
    });
    if opts.mdns {
        let host = format!("airplay-{}", device_id.replace(':', "").to_lowercase());
        let advertiser =
            mdns::Advertiser::start(&host, ctx.services()).context("не запустить Bonjour")?;
        *ctx.mdns.lock().unwrap() = Some(advertiser);
    }

    for listener in listeners {
        tokio::spawn(accept_loop(listener, ctx.clone()));
    }
    tokio::signal::ctrl_c().await?;
    info!("остановка");
    Ok(())
}

async fn accept_loop(listener: TcpListener, ctx: Arc<Context>) {
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(a) => a,
            Err(e) => {
                warn!("accept: {e}");
                continue;
            }
        };
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let peer = addr.to_string();
            info!(%peer, "подключение");
            match serve_connection(stream, ctx, addr).await {
                Ok(()) => info!(%peer, "отключение"),
                Err(e) => warn!(%peer, "соединение закрыто: {e:#}"),
            }
        });
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    ctx: Arc<Context>,
    peer: std::net::SocketAddr,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let local = stream.local_addr()?.ip();
    let mut session = Session::new(ctx, peer, local);
    let mut buf = vec![0u8; 16 * 1024];
    let mut out = Vec::new();
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        session.on_data(&buf[..n], &mut out)?;
        if !out.is_empty() {
            stream.write_all(&out).await?;
            out.clear();
        }
    }
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
