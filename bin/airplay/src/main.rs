//! Демон и утилита AirPlay в одном бинарнике.
//!
//! `airplay serve` запускает демон (по умолчанию все службы в этом процессе),
//! остальные подкоманды управляют работающим демоном через `ipc`.

use std::process::ExitCode;

use airplay::{audio_stream, serve, state};

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
    };
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
            "--audio" => opts.audio.device = value()?,
            "--buffer" => opts.audio.output_latency = ms(value()?, "--buffer")?,
            "--latency" => opts.audio.fallback_latency = ms(value()?, "--latency")?,
            other => return Err(format!("неизвестный параметр {other}")),
        }
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
            tracing_subscriber::fmt()
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
