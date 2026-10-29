//! `airplay serve`: приём соединений на порту 7000 и объявление в Bonjour.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use device::{DeviceInfo, Features};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{info, warn};

use crate::session::{Context, Session};
use crate::state;

pub struct Options {
    pub name: String,
    pub port: u16,
    pub pin: Option<String>,
    pub state_dir: PathBuf,
    pub mdns: bool,
    pub model: String,
}

pub async fn run(opts: Options) -> Result<()> {
    let identity = state::load_identity(&opts.state_dir)?;
    let store = state::FileStore::open(&opts.state_dir)?;
    let device_id = state::device_id(&identity);
    let pin = opts
        .pin
        .unwrap_or_else(|| format!("{:04}", rand::random::<u16>() % 10_000));
    let device = DeviceInfo {
        name: opts.name.clone(),
        device_id: device_id.clone(),
        model: opts.model,
        features: Features::AUDIO_RECEIVER,
        status_flags: 0x4,
        pairing_id: identity.pairing_id.clone(),
        public_key: identity.public_key(),
        source_version: "366.0".into(),
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

    let _mdns = if opts.mdns {
        let host = format!("airplay-{}", device_id.replace(':', "").to_lowercase());
        let services = vec![
            mdns::Service {
                service_type: mdns::AIRPLAY,
                instance: device.name.clone(),
                port: opts.port,
                txt: device.airplay_txt(),
            },
            mdns::Service {
                service_type: mdns::RAOP,
                instance: device.raop_instance(),
                port: opts.port,
                txt: device.raop_txt(),
            },
        ];
        Some(mdns::Advertiser::start(&host, services).context("не запустить Bonjour")?)
    } else {
        None
    };

    let ctx = Arc::new(Context {
        identity,
        device,
        store: Box::new(store),
        pin,
    });

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
            match serve_connection(stream, ctx, peer.clone()).await {
                Ok(()) => info!(%peer, "отключение"),
                Err(e) => warn!(%peer, "соединение закрыто: {e:#}"),
            }
        });
    }
}

async fn serve_connection(mut stream: TcpStream, ctx: Arc<Context>, peer: String) -> Result<()> {
    stream.set_nodelay(true)?;
    let mut session = Session::new(ctx, peer);
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
