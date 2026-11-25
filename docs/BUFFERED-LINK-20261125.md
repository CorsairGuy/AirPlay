# Buffered-аудио: ссылка на data-порт приёмника — 25 ноября 2026

Ветка `feat/buffered-link`, **стеком поверх `feat/buffered-audio-tx`**
(`6e322b7`): тип 103 и `BufferedSender` живут в той ветке, в `main` их нет.
Это единственное отступление от правила «каждая задача от `main`»; после
влияния `feat/buffered-audio-tx` ветку можно перебазировать на `main` без
конфликтов (изменён только `services/media/src/tx/mod.rs`).

## Зачем

`BufferedSender` умел писать кадры в любой `AsyncWrite`, но вызывающему
нечего было с этим сделать: ответ SETUP у приёмника надо разобрать, а
TCP-соединение к его `dataPort` — открыть и закрыть. Это и есть мост к
`tx::Session`/CLI, без которого buffered-отправитель нельзя включить.

## Что сделано

`services/media/src/tx/buffered_link.rs`:

- **`StreamReply::from_setup_reply`** — разбор `{streams: [{type, dataPort,
  controlPort, audioBufferSize}]}`. `dataPort` обязателен и проверяется на
  диапазон `u16` (нулевой/огромный — ошибка, а не усечение); `controlPort`
  `0`/отсутствие — `None`; `audioBufferSize` сохраняется как есть.
- **`StreamReply::require_buffered`** — отказ, если приёмник подтвердил не
  `type: 103` (например, realtime 96).
- **`BufferedLink::connect(addr, setup, ssrc, seq, ts, deadline)`** — TCP к
  `dataPort` с `TCP_NODELAY` и сроком; `ssrc` передаётся явно (для AAC-LC
  44,1 кГц стерео — `0x1600_0000`), а не выводится из `StreamSetup`.
- **`send_frame`/`send_frames`/`send_adts`** — отправка и счётчик дошедших
  кадров; `send_adts` режет ADTS тем же `adts_frames`, ошибка разбора
  помечена как ADTS.
- **`close`** — `shutdown` записи и ожидание отправки; `remote`,
  `next_sequence`, `next_timestamp`, `frames_sent` для диагностики.
- Экспорт: `tx::{BufferedLink, StreamReply}`.

## Проверки

```
cargo test --offline -p media buffered   # 13 passed
cargo test --offline -p media            # полный пакет
cargo clippy --offline -p media --all-targets -- -D warnings
cargo fmt -p media -- --check
```

Новые тесты (6): разбор полного ответа, отказы (нет `streams`,
отсутствующий/нулевой/слишком большой `dataPort`, подтверждённый type 96),
`controlPort = 0` как отсутствие, реальная отправка двух кадров по loopback
с расшифровкой `rtp::split_buffered` + `AudioCipher`, отправка ADTS-потока
(2 кадра), недоступный data-порт с понятной ошибкой.

## Границы

- RTSP-запросы (`ANNOUNCE`/`SETUP`/`RECORD`/`FLUSH`/`TEARDOWN`,
  `RTP-Info`, `audioBufferSize` в теле запроса) по-прежнему отправляет
  вызывающий: слой не владеет управляющим соединением и не
  переподключается. Полная интеграция с `tx::Session` требует вынести
  `Rtsp` из `session.rs` в общий модуль и добавить buffered-ветку в
  `Session::connect`/`run` — это отдельная задача, не сделанная здесь.
- AAC-энкодера нет: на вход идут уже закодированные кадры или ADTS-файл.
- Аппаратной проверки нет: loopback против собственного разбора, не iPhone
  и не HomePod. Формат `[len:u16BE]` и поля ответа взяты из нашего RX и
  §C-RTP/§C-NET wire-spec.
