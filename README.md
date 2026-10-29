# airplay

Приём и отправка AirPlay 1/2 на Rust, написанные с нуля: звук с синхронизацией по PTP, зеркалирование экрана 60 fps с аппаратным декодом, видео по HLS, MediaRemote. Цель — работать плавно даже на слабом железе (ARMv7, 512 МБ ОЗУ) и одновременно принимать и отправлять на ПК.

Подробный план: [airplay — план и архитектура](https://claude.ai/artifact/KP4mr81rv4RtaMDqdzWPqi).

## Структура

| Путь | Пакет | Что это |
| --- | --- | --- |
| `bin/airplay` | `airplay` | демон: RTSP, сопряжение, шифрование, сессии приёма и отправки |
| `bin/airplayutil` | `airplayutil` | утилита командной строки |
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

## Лицензия

GPL-3.0-or-later: приём зеркалирования и FairPlay основаны на подходе UxPlay (GPL-3.0).
