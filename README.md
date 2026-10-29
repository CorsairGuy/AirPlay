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

Этап 3: звук AirPlay 2 с iPhone, iPad и Mac. Приёмник виден в списке AirPlay, проходит сопряжение (transient, по PIN, legacy) и играет буферизованный поток (AAC, музыка) и realtime (ALAC, системные звуки). Время берётся от отправителя по PTP, расхождение с его часами выбирается по одному кадру, без ресемплинга. Громкость, пауза и перемотка работают.

Проверка на iPhone:

```sh
sudo RUST_LOG=info ./target/release/airplay serve --name "airplay"
```

`sudo` (или `setcap cap_net_bind_service+ep`) нужен для портов PTP 319/320: без них звук идёт, но по своим часам, а iOS дольше подключается. Откройте Пункт управления → AirPlay → «airplay» и включите музыку.

Параметры звука:

- `--audio default|null|hw:0,0` — куда играть (ALSA на Linux, системный вывод на Windows и macOS; `null` — никуда, для тестов);
- `--buffer 100` — буфер звуковой карты в мс: на слабом железе больше, для малой задержки меньше;
- `--latency 250` — задержка в мс, если часов отправителя нет.
- `--require-pin` — пускать только после сопряжения по PIN, без transient (PIN пишется в лог или задаётся `--pin`).

### Зеркалирование экрана

```sh
./target/release/airplay serve --mirror --video-dump - | ffplay -f h264 -
```

`--mirror` переключает приёмник на набор возможностей UxPlay: только с ним iOS 18 отдаёт ключ видео. Звук AirPlay 2, PTP и мультирум в этом режиме недоступны, поэтому выбирать режим нужно при запуске. Видео (H.264 и H.265) расшифровывается и отдаётся в `mirror::VideoSink`: `--video-dump` пишет его в файл или в stdout как есть. С `--features ffmpeg` (нужны `libavcodec-dev` и clang) libavcodec декодирует звук (AAC-ELD) и видео: `video::DecodingSink` декодирует кадры в своём потоке — встроенным декодером или аппаратным по имени (`h264_v4l2m2m` на Raspberry Pi) — и отдаёт их в `video::FrameSink` сразу или по часам iPhone, вровень со звуком, отбрасывая опоздавшие. Показ (KMS, D3D11) зависит от железа и живёт в репозиториях под него: они реализуют `FrameSink`. `airplay serve --mirror --video-decoder auto` декодирует без показа и раз в 5 с пишет в лог к/с и время декодера — чтобы проверить, тянет ли железо 60 к/с. Без фичи `ffmpeg` видео идёт без звука. `--display 1280x720@60` задаёт экран, который видит iPhone.

Программы под конкретное железо подключают приёмник как библиотеку (`airplay::serve::run`) и могут передать свой вывод звука через `audio::SinkFactory`. Если вывод управляет громкостью сам, `Sink::set_volume` возвращает `true`, и сэмплы приходят без масштабирования.

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
