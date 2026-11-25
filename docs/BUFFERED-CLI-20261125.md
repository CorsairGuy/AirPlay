# `airplay send-buffered`: CLI для buffered-аудио — 25 ноября 2026

Ветка `feat/buffered-cli`, стеком поверх `feat/buffered-session` →
`feat/buffered-link` → `feat/buffered-audio-tx`. В `main` типа 103 нет,
поэтому все три нижние ветки обязательны; после влияния
`feat/buffered-audio-tx` стек перебазируется на `main`.

## Что сделано

`bin/airplay/src/media/buffered.rs` — новый фронтенд, отдельная подкоманда
(не смешивается с realtime-PCM `send`):

```
airplay send-buffered --to HOST[:PORT] --aac FILE.aac [--control-port N] [--timeout-s N]
```

- `Options::parse` — `--to` обязателен и проверяется тем же разбором
  `HOST[:PORT]`, что у `send` (порт по умолчанию 7000); `--aac` обязателен;
  `--control-port` (0 — свободный); `--timeout-s` 1..30.
- `read_frames` — читает файл и проверяет ADTS тем же `adts_frames`
  (пустой/битый файл — ошибка).
- `run` — TCP к приёмнику, управляющий TCP-порт на локальном адресе
  маршрута (порт живёт, входящие соединения принимаются и закрываются:
  управляющий канал не реализован), `BufferedSession::pair_transient`,
  отправка кадров с темпом реального времени (`frame_deadline`: 1024 сэмпла
  = 23,2 мс), затем `finish` (FLUSH + TEARDOWN).
- `BufferedSession::pair_transient` (в `media`) — transient-сопряжение по
  PIN `3939`, как у realtime `Session` без сохранённых ключей: тот же
  `ControllerSetup`/`setup_proof`, ключи канала живут только в соединении.
- USAGE дополнен блоком «Параметры send-buffered».

## Проверки

```
cargo test --offline -p airplay media::buffered   # 4 passed
cargo test --offline -p media                     # 88 passed
cargo clippy --offline -p airplay -p media --all-targets -- -D warnings
cargo fmt -p airplay -p media -- --check
```

Тесты: разбор параметров (обязательные `--to`/`--aac`, значения по
умолчанию), отказы (неизвестный/повторный флаг, `--timeout-s 31`, нулевой
порт), темп кадра (1024/44100 и `frame_deadline`), чтение ADTS (мусор —
ошибка, валидный кадр — те же байты).

## Границы

- Аппаратной проверки нет: transient-сопряжение и воспроизведение на
  реальном приёмнике не выполнялись. Тесты покрывают только параметры,
  файл и арифметику темпа.
- Управляющий канал не реализован: порт открыт, но команды/события не
  обрабатываются. Аналогично нет громкости, `SETRATEANCHORTIME` и
  переподключения.
- `audioBufferSize` из ответа только логируется: единицы не подтверждены,
  поэтому pacing — ровно реальное время, без предзаполнения буфера.
- AAC-энкодера нет: нужен готовый ADTS-файл (AAC-LC 44,1 кГц).
- `--control-port` на не-local адрес не привязывается: слушаем локальный
  адрес маршрута.
