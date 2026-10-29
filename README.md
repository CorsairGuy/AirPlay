# AirPlay

Приём и отправка AirPlay 1/2 на Rust, написанные с нуля: звук с синхронизацией по PTP, зеркалирование экрана 60 fps с аппаратным декодом, видео по HLS, MediaRemote. Цель — работать плавно даже на слабом железе (ARMv7, 512 МБ ОЗУ) и одновременно принимать и отправлять на ПК.

Подробный план: [airplay — план и архитектура](https://claude.ai/artifact/KP4mr81rv4RtaMDqdzWPqi).

## Структура

| Путь | Пакет | Что это |
| --- | --- | --- |
| `bin/airplay` | `airplay` | демон (`airplay serve`) и утилита управления (`airplay status`, `list`, `send`…) в одном бинарнике |
| `services/timesync` | `timesync` | единые часы PTP/NTP |
| `services/mediaserver` | `mediaserver` | конвейеры звука и видео |
| `services/mediaremote` | `mediaremote` | «Сейчас играет», пульт |
| `crates/<имя>` | `ap_<имя>` | библиотеки протоколов и медиа |

Библиотеки называются `ap_<имя>`, а в коде подключаются без префикса: `use rtsp::…`. По умолчанию все службы работают в одном процессе `airplay`.

Библиотеки: `base`, `rtsp`, `pair`, `crypto`, `fairplay`, `mdns`, `device`, `clock`, `rtp`, `audio_stream`, `mirror`, `hls`, `mrp`, `companion`, `codec`, `audio`, `video`, `capture`, `ipc`.

## Сборка

```sh
cargo build --release
# для устройств с маленькой флеш-памятью
cargo build --profile release-small
```

## Что уже работает

Этап 2: приёмник виден в списке AirPlay и проходит сопряжение (transient с PIN 3939, постоянное по PIN, legacy для зеркалирования). Отвечает на `/info`, после сопряжения канал шифруется. `SETUP` и дальше пока отвечают 501, звука ещё нет.

Проверка на iPhone:

```sh
RUST_LOG=debug cargo run --release -p airplay -- serve --name "airplay"
```

Откройте Пункт управления → AirPlay: устройство «airplay» должно появиться в списке. При выборе в логе будет «transient-сопряжение выполнено», затем iPhone получит 501 на `SETUP` и отключится.

Ключ устройства и список сопряжённых контроллеров лежат в `~/.local/state/airplay` (или в `AIRPLAY_STATE_DIR`).

## Тесты

```sh
cargo test --workspace
# независимый клиент: криптография и сопряжение pyatv против живого сервера
pip install pyatv "srptools==1.0.1"
cargo run -p airplay -- serve --port 7123 --pin 1234 --no-mdns &
python tests/interop/pyatv_client.py 127.0.0.1 7123 1234
```

## Лицензия

GPL-3.0-or-later: приём зеркалирования и FairPlay основаны на подходе UxPlay (GPL-3.0).
