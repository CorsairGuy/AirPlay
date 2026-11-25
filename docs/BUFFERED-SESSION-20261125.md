# Buffered-аудио: сессия SETUP→RECORD→FLUSH→TEARDOWN — 25 ноября 2026

Ветка `feat/buffered-session`, стеком поверх `feat/buffered-link` и
`feat/buffered-audio-tx` (тип 103 в `main` отсутствует). Изменения вне
`tx/mod.rs` не пересекаются с предыдущей веткой, так что после влияния
`feat/buffered-audio-tx` стек перебазируется на `main` без конфликтов.

## Что сделано

`services/media/src/tx/buffered_session.rs` — RTSP-последовательность
buffered-сессии поверх готового управляющего соединения:

- **`BufferedConfig`** — адрес приёмника, `controlPort` отправителя, `shk`,
  `streamConnectionID`, `ssrc` (по умолчанию `AAC_LC_44100_SSRC =
  0x1600_0000`), стартовые `seq`/`timestamp`, срок запроса. Проверки:
  ненулевые порты, срок 1..30 с.
- **`BufferedSession::open`** — TCP, `TCP_NODELAY`, pair-verify по
  сохранённым `PairingCredentials` (тот же `verify_pairing`, что у realtime
  `Session`; в `session.rs` стал `pub(crate)`), шифрование канала, затем
  `start`.
- **`BufferedSession::start`** — внутренняя, тестируемая часть:
  `SETUP` с телом `StreamSetup::setup_plist()` →
  разбор ответа `StreamReply::from_setup_reply` → `require_buffered` (отказ
  на чужой тип до подключения) → TCP к `dataPort` через `BufferedLink` →
  `RECORD`.
- **`finish`** — `FLUSH` с `Range: npt=0-`, `Session` и `RTP-Info
  seq=<следующий>;rtptime=<следующий>`, закрытие data-сокета, `TEARDOWN`;
  возвращает число отправленных кадров. `send_frame`/`send_frames`/
  `send_adts` делегируются в `BufferedLink`.

## Проверки

```
cargo test --offline -p media buffered   # 16 passed
cargo test --offline -p media            # полный пакет
cargo clippy --offline -p media --all-targets -- -D warnings
cargo fmt -p media -- --check
```

Новые тесты (3): полная последовательность против фейкового RTSP-приёмника
по loopback — проверены порядок `SETUP`, `RECORD`, `FLUSH seq=102;rtptime=4048`,
`TEARDOWN`, разбор `dataPort`/`audioBufferSize` и расшифровка обоих кадров
из data-сокета; отказ на подтверждённый `type: 96` до `RECORD`; валидация
конфига.

## Границы

- PIN-pairing и transient-режим (3939) для buffered не сделаны: `open`
  требует сохранённые ключи. Тесты идут по незашифрованному каналу с
  фейковым приёмником — pair-verify в этом пути не исполняется.
- Нет AAC-энкодера, управления громкостью, событийного канала и
  `SETRATEANCHORTIME`: приёмник получит кадры без явного якоря
  воспроизведения от отправителя. Наш RX в этом случае играет с первого
  пакета, но поведение сторонних приёмников не проверено.
- CLI не подключён: `bin/airplay/src/media/tx.rs` остаётся realtime-PCM
  фронтендом. Следующий шаг — параметр `--send-aac <файл> --control-port`,
  который вызовет `BufferedSession::open` и подаст ADTS-кадры.
- Аппаратной проверки нет: только loopback, всё помечено «непроверено».
