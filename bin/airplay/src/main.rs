//! Демон и утилита AirPlay в одном бинарнике.
//!
//! `airplay serve` запускает демон (по умолчанию все службы в этом процессе),
//! остальные подкоманды управляют работающим демоном через `ipc`.

use std::process::ExitCode;

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
  version   версия";

fn main() -> ExitCode {
    let cmd = std::env::args().nth(1);
    match cmd.as_deref() {
        Some("version" | "--version" | "-V") => {
            println!("airplay {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("help" | "--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(cmd @ ("serve" | "status" | "list" | "send" | "pair" | "set" | "log")) => {
            eprintln!("airplay {cmd}: ещё не реализовано");
            ExitCode::FAILURE
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
