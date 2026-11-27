//! Тестовый harness для независимого клиента (C27), не приложение ядра.
//!
//! Запускает один приёмник через публичный `runtime::AirPlay`, печатает
//! `READY <port>` и работает до Ctrl+C, после чего освобождает порт и ресурсы.
//! Предназначен только для CI/локального interop-теста с
//! `tests/interop/pyatv_client.py`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use runtime::lifecycle::receiver as serve;

struct Args {
    name: String,
    port: u16,
    pin: String,
    state_dir: PathBuf,
    /// Advertise over Bonjour so a picker (iPhone) can find the receiver.
    mdns: bool,
    /// `null`, `default` or a device name; `default` uses WASAPI on Windows.
    device: String,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut name = "Interop".to_string();
        let mut port = 7123;
        let mut pin = "1234".to_string();
        let mut state_dir =
            std::env::temp_dir().join(format!("airplay-interop-{}", std::process::id()));
        let mut mdns = false;
        let mut device = "null".to_string();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--name" => name = args.next().unwrap_or_default(),
                "--port" => port = args.next().unwrap_or_default().parse()?,
                "--pin" => pin = args.next().unwrap_or_default(),
                "--state-dir" => state_dir = PathBuf::from(args.next().unwrap_or_default()),
                "--mdns" => mdns = true,
                "--device" => device = args.next().unwrap_or_default(),
                other => anyhow::bail!("unknown argument {other}"),
            }
        }
        if port == 0
            || name.is_empty()
            || pin.len() != 4
            || !pin.bytes().all(|b| b.is_ascii_digit())
            || device.is_empty()
        {
            anyhow::bail!(
                "--port must be nonzero, --pin must be 4 digits, --name and --device nonempty"
            );
        }
        std::fs::create_dir_all(&state_dir)?;
        Ok(Self {
            name,
            port,
            pin,
            state_dir,
            mdns,
            device,
        })
    }
}

fn options(args: &Args) -> serve::Options {
    serve::Options {
        name: args.name.clone(),
        port: args.port,
        ptp_bind: None,
        pin: Some(args.pin.clone()),
        state_dir: args.state_dir.clone(),
        mdns: args.mdns,
        model: "AppleTV3,2".into(),
        features: Some(device::Features(
            device::Features::AUDIO_RECEIVER.0 & !device::Features::PTP,
        )),
        status_flags: None,
        accessory_type: None,
        audio: audio_stream::Config {
            device: args.device.clone(),
            ..audio_stream::Config::default()
        },
        require_pin: false,
        #[cfg(feature = "mirroring")]
        mirror: None,
        #[cfg(feature = "hls")]
        hls: None,
        display: device::Display::default(),
        photo: None,
        profile: None,
        ownership: Default::default(),
        remote: Default::default(),
        mrp: serve::Mrp::Card,
        events: Arc::new(media::events::NoEvents),
        volume_range_db: 30.0,
        apply_volume: true,
        source_version: "366.0".into(),
        enclosure_color: None,
        time: Some(timesync::TimeSync::idle(1)),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse()?;
    let receiver = runtime::AirPlay::start(options(&args)).await?;
    println!("READY {}", receiver.receiver().port());
    // CI завершает процесс сигналом; Ctrl+C даёт штатный stop и освобождение
    // порта, чтобы harness не оставлял состояние между запусками.
    let _ = tokio::signal::ctrl_c().await;
    let _ = tokio::time::timeout(Duration::from_secs(2), receiver.stop()).await;
    Ok(())
}
