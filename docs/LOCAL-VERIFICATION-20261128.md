# Локальный прогон 28 ноября 2026 (R30)

Срез кода: `33b35ae`, ветка `agent/integration`. `remote` и `timesync`
прогнаны на `4e19eab`, зеркалирование — на `33b35ae`. Это Windows-прогон
в Git Bash, не Linux CI и не аппаратная проверка.

## Команды Windows

```bash
export PATH="/c/msys64/ucrt64/bin:$PATH"
cargo fmt --all --check
cargo clippy --offline -p remote -p timesync --all-targets
cargo test --offline -p remote --lib
cargo test --offline -p timesync --lib ntp::tests
```

## Команды Linux CI

```bash
cargo fmt --all --check
cargo clippy --offline --workspace --all-targets
cargo test --offline --workspace
```

Эти команды не запускались на Linux-раннере в этом ходе.

## Результаты

| Проверка | Результат | Ограничение |
|---|---|---|
| `cargo fmt --all --check` | PASS | нет |
| `cargo clippy` для `remote` и `timesync` | PASS, без предупреждений | не весь workspace |
| `cargo test -p remote --lib` | PASS, 50 тестов | не весь workspace |
| `cargo test -p timesync --lib ntp::tests` | PASS, 16 тестов | фильтр по NTP |
| `cargo test -p media --lib` | PASS, 171 тест | не весь workspace |
| `cargo test -p media --test mirror_tx` | PASS, 4 теста | loopback, не устройство |
| `cargo clippy -p media --test mirror_tx` | PASS | один integration target |
| pyatv-клиент | SKIP | пакет и независимый клиент не установлены |
| iPhone/Apple TV/HomePod | SKIP | нет устройства в этом стенде |
| реальный звук, 60 FPS, sync | SKIP | нет дисплея/выхода |
| FFmpeg/HLS-сегменты | SKIP | нет рабочей FFmpeg-среды |

## Что это не доказывает

- Зелёный Windows-прогон не заменяет Linux CI, полный workspace и pyatv.
- Ни один аппаратный сценарий не проверен; PASS относится только к
  перечисленным командам.
