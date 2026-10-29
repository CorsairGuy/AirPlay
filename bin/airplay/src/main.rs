//! Демон и утилита AirPlay в одном бинарнике.
//!
//! `airplay serve` запускает демон (по умолчанию все службы в этом процессе),
//! остальные подкоманды управляют работающим демоном через `ipc`.

use std::process::ExitCode;

use airplay::{audio_stream, device, mirror, serve, state};

use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Использование: airplay <команда>

Команды:
  serve     запустить демон
  status    состояние демона и активные сессии
  list      AirPlay-устройства в сети
  send      отправить звук или экран на устройство
  pair      сопряжение с устройством
  set       изменить настройку на лету
  log       логи и отладка
  version   версия

Параметры serve:
  --name ИМЯ      имя в списке AirPlay (по умолчанию имя компьютера)
  --port ПОРТ     порт RTSP (по умолчанию 7000)
  --pin PIN       PIN для сопряжения (по умолчанию случайный, пишется в лог)
  --state КАТАЛОГ где хранить ключи (по умолчанию ~/.local/state/airplay)
  --model МОДЕЛЬ  модель для iOS (по умолчанию AirPlay)
  --no-mdns       не объявлять себя в Bonjour
  --require-pin   пускать только после сопряжения по PIN (без transient)
  --mirror        режим зеркалирования экрана вместо звука AirPlay 2
  --video-dump ПУТЬ
                  писать видео зеркалирования в файл H.264 (Annex B);
                  «-» — в stdout: airplay serve --mirror --video-dump - | ffplay -f h264 -
  --display ШxВ@К экран для зеркалирования (по умолчанию 1920x1080@60)
  --audio ВЫВОД   куда играть: default, null (никуда) или устройство ALSA,
                  например hw:0,0 (по умолчанию default)
  --buffer МС     буфер звуковой карты, мс (по умолчанию 100; меньше —
                  быстрее отклик, больше — устойчивее на слабом железе)
  --latency МС    задержка, если часов отправителя нет (по умолчанию 250)

Подробность логов задаёт RUST_LOG, например RUST_LOG=debug.";

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "AirPlay".into())
}

fn parse_serve(args: &[String]) -> Result<serve::Options, String> {
    let mut opts = serve::Options {
        name: hostname(),
        port: 7000,
        pin: None,
        state_dir: state::default_dir(),
        mdns: true,
        model: "AirPlay".into(),
        audio: audio_stream::Config::default(),
        require_pin: false,
        mirror: None,
        display: device::Display {
            width: 1920,
            height: 1080,
            fps: 60,
        },
    };
    let mut mirror = false;
    let mut dump: Option<String> = None;
    let ms = |v: String, name: &str| {
        v.parse::<u64>()
            .map(std::time::Duration::from_millis)
            .map_err(|_| format!("{name}: нужно число миллисекунд"))
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
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
            "--no-mdns" => opts.mdns = false,
            "--require-pin" => opts.require_pin = true,
            "--mirror" => mirror = true,
            "--video-dump" => dump = Some(value()?),
            "--display" => opts.display = parse_display(&value()?)?,
            "--audio" => opts.audio.device = value()?,
            "--buffer" => opts.audio.output_latency = ms(value()?, "--buffer")?,
            "--latency" => opts.audio.fallback_latency = ms(value()?, "--latency")?,
            other => return Err(format!("неизвестный параметр {other}")),
        }
    }
    if dump.is_some() && !mirror {
        return Err("--video-dump только вместе с --mirror".into());
    }
    if mirror {
        if opts.model == "AirPlay" {
            // Как у UxPlay: под этой моделью iOS предлагает «Повтор экрана».
            opts.model = "AppleTV3,2".into();
        }
        opts.mirror = Some(match dump {
            Some(path) => mirror::VideoSinkFactory::new(move || dump_sink(&path)),
            None => mirror::VideoSinkFactory::new(|| Box::new(mirror::CountingSink(0))),
        });
    }
    Ok(opts)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => {
            let opts = match parse_serve(&args[1..]) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("airplay serve: {e}\n\n{USAGE}");
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
                    eprintln!("airplay serve: {e:#}");
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
        Some(cmd @ ("status" | "list" | "send" | "pair" | "set" | "log")) => {
            eprintln!("airplay {cmd}: ещё не реализовано");
            ExitCode::FAILURE
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
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

/// Каждый новый видеопоток пишется в файл заново.
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
