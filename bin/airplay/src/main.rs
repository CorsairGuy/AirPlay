//! Демон и утилита AirPlay в одном бинарнике.
//!
//! `airplay` запускает демон (по умолчанию все службы в этом процессе),
//! остальные подкоманды управляют работающим демоном через `ipc`.

use std::process::ExitCode;

#[cfg(feature = "mirroring")]
mod video_process;

use airplay::identity::state;
#[cfg(feature = "mirroring")]
use airplay::mirror;
use airplay::sessions::serve;
#[cfg(all(feature = "mirroring", feature = "ffmpeg"))]
use airplay::video;
use airplay::{audio_stream, device};

use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Использование: airplay [параметры демона] | airplay <команда>

Команды:
  status    состояние демона и активные сессии
  list      AirPlay-устройства в сети
  send      отправить PCM-звук на приёмники (realtime NTP; без автопереподключения)
  pair      сопряжение с устройством
  set       изменить настройку на лету
  log       логи и отладка
  version   версия

Без подкоманды запускается демон.

Параметры демона:
  --name ИМЯ      имя в списке AirPlay (по умолчанию имя компьютера)
  --port ПОРТ     порт RTSP (по умолчанию 7000)
  --pin PIN       PIN для сопряжения (по умолчанию случайный, пишется в лог)
  --state КАТАЛОГ где хранить ключи (по умолчанию ~/.local/state/airplay)
  --model МОДЕЛЬ  модель для iOS (по умолчанию AirPlay)
  --profile ПУТЬ  профиль устройства (plist): model, features, status_flags,
                  source_version, accessory_type, display, audio_only_ui,
                  enclosure_color и raop (объявлять _raop._tcp); значения
                  профиля заменяют соответствующие параметры выше
  --no-mdns       не объявлять себя в Bonjour
  --ptp-bind IP   использовать интерфейс IP для общей службы PTP
  --require-pin   пускать только после сопряжения по PIN (без transient)
  --mrp           карточка «Сейчас играет» на iPhone: трек, обложка, пульт
                  и громкость текущего отправителя (MediaRemote, поток 130)
  --mrp-bridge АДРЕС
                  вместо этого пересылать канал MediaRemote в свою службу MRP
                  по TCP, например 127.0.0.1:18902
  --mirror        режим зеркалирования экрана вместо звука AirPlay 2
  --video-dump ПУТЬ
                  писать видео зеркалирования в файл H.264 или H.265 (Annex B);
                  «-» — в stdout: airplay --mirror --video-dump - | ffplay -f h264 -
                  (H.265: ffplay -f hevc -)
  --video-player ПУТЬ
                  запускать отдельный viewer на каждый видеопоток (Annex B stdin)
  --video-player-framed
                  передавать границы кадров и PTS в формате MRV1
  --video-player-arg ЗНАЧЕНИЕ
                  аргумент viewer; можно повторять, shell не используется
  --video-decoder ДЕКОДЕР
                  декодировать видео зеркалирования и писать в лог к/с и
                  время декодера: auto — встроенный декодер libavcodec,
                  или имя, например h264_v4l2m2m (сборка с --features ffmpeg)
  --video-delay МС
                  выдавать кадры по часам iPhone с этой задержкой, вровень
                  со звуком; без параметра — сразу после декодирования
  --display ШxВ@К экран для зеркалирования (по умолчанию 1920x1080@60)
  --audio ВЫВОД   куда играть: default, null (никуда) или устройство ALSA,
                  например hw:0,0 (по умолчанию default)
  --buffer МС     буфер звуковой карты, мс (по умолчанию 100; меньше —
                  быстрее отклик, больше — устойчивее на слабом железе)
  --latency МС    задержка, если часов отправителя нет (по умолчанию 250)
  --latency-offset МС
                  сдвиг воспроизведения: меньше нуля — раньше (задержка после
                  звуковой карты), больше — позже (по умолчанию 0)
  --group-latency-offset МС
                  тот же сдвиг, когда в сессии есть другие приёмники
  --volume-range ДБ
                  на сколько дБ растянуть ползунок громкости (по умолчанию 30)
  --ignore-volume громкость не менять, только сообщать (её ведёт внешний микшер)
  --source-version В
                  версия AirPlay в /info и Bonjour (по умолчанию 366.0)
  --features МАСКА маска возможностей (десятичная или 0x...)
  --status-flags МАСКА флаги статуса (десятичные или 0x...)
  --accessory-type N тип устройства для TXT и /info
  --enclosure-color Ц
                  цвет корпуса для значка в iOS: 1 — белый, 2 — чёрный

Параметры send:
  --to HOST[:PORT] можно повторять (до 8 приёмников; порт по умолчанию 7000)
  --volume ДБ     от -30 до 0, либо -144 (mute); по умолчанию -29.7
  --latency-ms N  250..2000 мс; по умолчанию 500
  --local-program /PATH  необязательный MLP1 renderer (Linux, без shell)
  --local-arg VALUE      аргумент renderer; можно повторять
  --local-gain N         компенсация только локальной копии, 1..4; по умолчанию 1
  --local-gate /PATH     локально тишина, пока файл готовности отсутствует
  --control-socket /PATH локальное управление STATUS/VOLUME (Unix, private directory)
                  stdin: S16BE, 44100 Гц, стерео. Один общий источник времени;
                  это не управление группой AP2/PTP. Ошибка приёмника завершает запуск.

Подробность логов задаёт RUST_LOG, например RUST_LOG=debug.";

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "AirPlay".into())
}

fn parse_unsigned(value: &str, flag: &str) -> Result<u64, String> {
    let parsed = if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16)
    } else {
        value.parse()
    };
    parsed.map_err(|_| format!("{flag}: нужно неотрицательное целое число"))
}

fn parse_serve(args: &[String]) -> Result<serve::Options, String> {
    let mut opts = serve::Options {
        name: hostname(),
        port: 7000,
        ptp_bind: None,
        pin: None,
        state_dir: state::default_dir(),
        mdns: true,
        profile: None,
        model: "AirPlay".into(),
        features: None,
        status_flags: None,
        accessory_type: None,
        audio: audio_stream::Config::default(),
        require_pin: false,
        #[cfg(feature = "mirroring")]
        mirror: None,
        #[cfg(feature = "hls")]
        hls: None,
        remote: Default::default(),
        mrp: serve::Mrp::Off,
        events: std::sync::Arc::new(media::events::NoEvents),
        volume_range_db: 30.0,
        apply_volume: true,
        source_version: "366.0".into(),
        enclosure_color: None,
        display: device::Display {
            width: 1920,
            height: 1080,
            fps: 60,
        },
    };
    let mut mirror = false;
    let mut dump: Option<String> = None;
    let mut player: Option<String> = None;
    let mut player_args = Vec::new();
    let mut framed_player = false;
    let mut decoder: Option<String> = None;
    let mut delay = None;
    let ms = |v: String, name: &str| {
        v.parse::<u64>()
            .map(std::time::Duration::from_millis)
            .map_err(|_| format!("{name}: нужно число миллисекунд"))
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if !cfg!(feature = "mirroring")
            && matches!(
                arg.as_str(),
                "--mirror"
                    | "--video-dump"
                    | "--video-decoder"
                    | "--video-delay"
                    | "--display"
                    | "--video-player"
                    | "--video-player-arg"
                    | "--video-player-framed"
            )
        {
            return Err(format!("{arg}: build with --features mirroring"));
        }
        let mut value = || it.next().cloned().ok_or(format!("{arg}: нужно значение"));
        match arg.as_str() {
            "--name" => opts.name = value()?,
            "--port" => {
                opts.port = value()?
                    .parse()
                    .map_err(|_| "--port: нужно число".to_string())?
            }
            "--pin" => opts.pin = Some(value()?),
            "--state" => opts.state_dir = value()?.into(),
            "--model" => opts.model = value()?,
            "--profile" => {
                opts.profile = Some(
                    device::Profile::load(std::path::Path::new(&value()?))
                        .map_err(|e| format!("--profile: {e}"))?,
                )
            }
            "--no-mdns" => opts.mdns = false,
            "--require-pin" => opts.require_pin = true,
            "--source-version" => opts.source_version = value()?,
            "--features" => {
                opts.features = Some(device::Features(parse_unsigned(&value()?, "--features")?))
            }
            "--status-flags" => {
                opts.status_flags = Some(
                    u32::try_from(parse_unsigned(&value()?, "--status-flags")?)
                        .map_err(|_| "--status-flags: значение превышает u32".to_string())?,
                )
            }
            "--accessory-type" => {
                opts.accessory_type = Some(parse_unsigned(&value()?, "--accessory-type")?)
            }
            "--enclosure-color" => opts.enclosure_color = Some(value()?),
            "--volume-range" => {
                opts.volume_range_db = value()?
                    .parse()
                    .ok()
                    .filter(|v: &f32| (1.0..=144.0).contains(v))
                    .ok_or("--volume-range: нужно число дБ от 1 до 144".to_string())?
            }
            "--ignore-volume" => opts.apply_volume = false,
            "--latency-offset" => {
                opts.audio.latency_offset_ns = signed_ms(&value()?, "--latency-offset")?
            }
            "--group-latency-offset" => {
                opts.audio.group_latency_offset_ns =
                    Some(signed_ms(&value()?, "--group-latency-offset")?)
            }
            "--ptp-bind" => {
                opts.ptp_bind =
                    Some(value()?.parse().map_err(|_| {
                        "--ptp-bind: expected a specific local IP address".to_string()
                    })?)
            }
            "--mrp" => opts.mrp = serve::Mrp::Card,
            "--mrp-bridge" => {
                opts.mrp = serve::Mrp::Bridge(value()?.parse().map_err(|_| {
                    "--mrp-bridge: нужен адрес, например 127.0.0.1:18902".to_string()
                })?)
            }
            "--mirror" => mirror = true,
            "--video-dump" => dump = Some(value()?),
            "--video-player-framed" => framed_player = true,
            "--video-player" => player = Some(value()?),
            "--video-player-arg" => player_args.push(value()?),
            "--video-decoder" => decoder = Some(value()?),
            "--video-delay" => delay = Some(ms(value()?, "--video-delay")?),
            "--display" => opts.display = parse_display(&value()?)?,
            "--audio" => opts.audio.device = value()?,
            "--buffer" => opts.audio.output_latency = ms(value()?, "--buffer")?,
            "--latency" => opts.audio.fallback_latency = ms(value()?, "--latency")?,
            other => return Err(format!("неизвестный параметр {other}")),
        }
    }
    if player.is_some() && (!mirror || dump.is_some() || decoder.is_some() || delay.is_some()) {
        return Err("--video-player requires --mirror and excludes other video outputs".into());
    }
    if framed_player && player.is_none() {
        return Err("--video-player-framed requires --video-player".into());
    }
    if !player_args.is_empty() && player.is_none() {
        return Err("--video-player-arg requires --video-player".into());
    }
    if (dump.is_some() || decoder.is_some() || delay.is_some()) && !mirror {
        return Err(
            "--video-dump, --video-decoder и --video-delay только вместе с --mirror".into(),
        );
    }
    if dump.is_some() && decoder.is_some() {
        return Err("--video-dump и --video-decoder вместе нельзя".into());
    }
    if delay.is_some() && decoder.is_none() {
        return Err("--video-delay только вместе с --video-decoder".into());
    }
    #[cfg(feature = "mirroring")]
    if mirror {
        if opts.model == "AirPlay" {
            // Как у UxPlay: под этой моделью iOS предлагает «Повтор экрана».
            opts.model = "AppleTV3,2".into();
        }
        opts.mirror = Some(if let Some(program) = player {
            video_process::factory(program.into(), player_args, framed_player)?
        } else {
            match (dump, decoder) {
                (Some(path), _) => mirror::VideoSinkFactory::new(move |_| dump_sink(&path)),
                (None, Some(name)) => decode_factory(name, delay)?,
                (None, None) => {
                    mirror::VideoSinkFactory::new(|_| Box::new(mirror::CountingSink(0)))
                }
            }
        });
    }
    Ok(opts)
}

fn starts_daemon(args: &[String]) -> bool {
    match args.first().map(String::as_str) {
        None => true,
        Some(value) => {
            value.starts_with('-') && !matches!(value, "--help" | "-h" | "--version" | "-V")
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        _ if starts_daemon(&args) => {
            let opts = match parse_serve(&args) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("airplay: {e}\n\n{USAGE}");
                    return ExitCode::from(2);
                }
            };
            // Логи в stderr: stdout может занимать видео (--video-dump -).
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_env_filter(
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
                )
                .init();
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("не создать среду выполнения tokio");
            match runtime.block_on(serve::run(opts)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("airplay: {e:#}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("version" | "--version" | "-V") => {
            println!("airplay {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("send") => {
            let options = match airplay::media::tx::Options::parse(&args[1..]) {
                Ok(options) => options,
                Err(error) => {
                    eprintln!("airplay send: {error:#}\n\n{USAGE}");
                    return ExitCode::from(2);
                }
            };
            tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_env_filter(
                    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
                )
                .init();
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("не создать среду выполнения tokio");
            let result = runtime.block_on(async {
                let (stop, stopped) = tokio::sync::watch::channel(false);
                let interrupt = tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        stop.send_replace(true);
                    }
                });
                let result = airplay::media::tx::run(options, tokio::io::stdin(), stopped).await;
                interrupt.abort();
                result
            });
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("airplay send: {error:#}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(cmd @ ("status" | "list" | "pair" | "set" | "log")) => {
            eprintln!("airplay {cmd}: ещё не реализовано");
            ExitCode::FAILURE
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Миллисекунды со знаком в наносекунды.
fn signed_ms(v: &str, name: &str) -> Result<i64, String> {
    v.parse::<i64>()
        .map(|ms| ms * 1_000_000)
        .map_err(|_| format!("{name}: нужно число миллисекунд, можно отрицательное"))
}

fn parse_display(v: &str) -> Result<device::Display, String> {
    let err = || format!("--display: нужно ШxВ@К, например 1920x1080@60, а не {v}");
    let (size, fps) = v.split_once('@').unwrap_or((v, "60"));
    let (w, h) = size.split_once('x').ok_or_else(err)?;
    Ok(device::Display {
        width: w.parse().map_err(|_| err())?,
        height: h.parse().map_err(|_| err())?,
        fps: fps.parse().map_err(|_| err())?,
    })
}

/// Декодирование без показа: проверить, тянет ли железо поток.
#[cfg(all(feature = "mirroring", feature = "ffmpeg"))]
fn decode_factory(
    name: String,
    delay: Option<std::time::Duration>,
) -> Result<mirror::VideoSinkFactory, String> {
    let opts = video::Options {
        decoder: video::VideoOptions {
            decoder: (name != "auto").then_some(name),
            low_delay: false,
        },
        delay,
    };
    Ok(mirror::VideoSinkFactory::new(move |info| {
        Box::new(video::DecodingSink::new(
            info,
            opts.clone(),
            Box::new(video::Discard),
        ))
    }))
}

#[cfg(all(feature = "mirroring", not(feature = "ffmpeg")))]
fn decode_factory(
    _name: String,
    _delay: Option<std::time::Duration>,
) -> Result<mirror::VideoSinkFactory, String> {
    Err("--video-decoder: соберите с --features ffmpeg".into())
}

/// Каждый новый видеопоток пишется в файл заново.
#[cfg(feature = "mirroring")]
fn dump_sink(path: &str) -> Box<dyn mirror::VideoSink> {
    if path == "-" {
        return Box::new(mirror::AnnexBWriter::new(std::io::stdout()));
    }
    match std::fs::File::create(path) {
        Ok(f) => Box::new(mirror::AnnexBWriter::new(std::io::BufWriter::new(f))),
        Err(e) => {
            tracing::warn!("{path}: {e}");
            Box::new(mirror::CountingSink(0))
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn parses_configurable_advertisement() {
        let opts = super::parse_serve(&[
            "--features".into(),
            "0x8300527FFEE6".into(),
            "--status-flags".into(),
            "0x4".into(),
            "--accessory-type".into(),
            "1".into(),
        ])
        .unwrap();
        assert_eq!(opts.features.unwrap().0, 0x8300527FFEE6);
        assert_eq!(opts.status_flags, Some(4));
        assert_eq!(opts.accessory_type, Some(1));
        assert!(super::parse_serve(&["--features".into(), "0xno".into()]).is_err());
    }
    use super::*;
    #[test]
    fn profile_flag_loads_a_device_profile() {
        let options = parse_serve(&[
            "--profile".into(),
            "../../docs/profiles/tv.plist".into(),
        ])
        .unwrap();
        let profile = options.profile.expect("профиль загружен");
        assert_eq!(profile.model, "AppleTV6,2");
        assert_eq!(profile.display.unwrap().width, 1920);
    }

    #[test]
    fn daemon_is_default_and_old_subcommand_is_rejected() {
        assert!(starts_daemon(&[]));
        assert!(starts_daemon(&["--port".into(), "7123".into()]));
        for value in [
            "serve",
            "send",
            "status",
            "version",
            "help",
            "--help",
            "-h",
            "--version",
            "-V",
        ] {
            assert!(!starts_daemon(&[value.into()]));
        }
        assert!(parse_serve(&["serve".into()]).is_err());
        assert_eq!(
            parse_serve(&["--port".into(), "7123".into()]).unwrap().port,
            7123
        );
    }

    #[cfg(not(feature = "mirroring"))]
    #[test]
    fn audio_build_rejects_mirroring_options_before_starting_services() {
        for option in [
            "--mirror",
            "--video-dump",
            "--video-decoder",
            "--video-delay",
            "--display",
        ] {
            let error = parse_serve(&[option.into()])
                .err()
                .expect("unsupported option accepted");
            assert!(error.contains("--features mirroring"), "{error}");
        }
        assert!(parse_serve(&["--audio".into(), "null".into(), "--no-mdns".into()]).is_ok());
    }

    #[cfg(feature = "mirroring")]
    #[test]
    fn default_build_keeps_mirroring_factory_and_model() {
        let options = parse_serve(&["--mirror".into()]).unwrap_or_else(|e| panic!("{e}"));
        assert!(options.mirror.is_some());
        assert_eq!(options.model, "AppleTV3,2");
    }
}
