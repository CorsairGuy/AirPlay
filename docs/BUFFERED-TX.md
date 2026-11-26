# Buffered audio TX: транспорт, соединение, сессия и CLI

Сводка четырёх интегрированных частей на 26 ноября 2026. Тесты ниже датированы исходными изменениями и не подтверждают совместимость с реальным приёмником. Актуальные ограничения — в [STATUS](STATUS.md).

## Отправка buffered-аудио (type 103) — 25 ноября 2026

При интеграции: после ошибки или отмены записи поток помечается непригодным для повторного использования; разбор ADTS принимает только AAC-LC 44,1 кГц стерео с одним блоком на кадр, как в SETUP. Проверка на реальном приёмнике остаётся открытой.


## Что сделано

`services/media/src/tx/buffered.rs`, экспорт `media::tx::{BufferedSender,
StreamSetup, adts_frames, AdtsError}`:

- **`BufferedSender<W: AsyncWrite>`** — отправляет уже закодированные кадры
  AAC-LC: RTP-заголовок (`payload_type 0x60`), шифрование `rtp::AudioCipher`
  ключом `shk`, кадрирование `[длина:u16BE][пакет]` через
  `rtp::frame_buffered`. При ошибке или отмене записи поток становится непригодным к повторному использованию, чтобы не повторить AEAD nonce. Ошибка валидации кадра до записи не отправляет данные.
- **`adts_frames`** — разбор ADTS (7 байт, с CRC 9): синхрос `0xFFF`, длина
  кадра 13 бит, полезная нагрузка без заголовка. Ошибки: `Truncated`,
  `BadSync`, `BadLength`. Позволяет отправлять `.aac` без кодировщика.
- **`StreamSetup`** — словарь стрима для SETUP: `type=103`, `ct=4`,
  `isMedia`, `shk`, `sr`, `spf`, `controlPort`, `streamConnectionID`,
  `supportsDynamicStreamID=false`, и тело `{streams: [...]}`.

## Проверки

```
cargo test --offline -p media      # 79 passed (73 прежних + 6 buffered)
cargo clippy --offline -p media --all-targets -- -D warnings
cargo fmt -p media -- --check
```

Тесты: ADTS (обычный/CRC/обрезанный/битый синхрос/длина меньше заголовка),
поля SETUP, сквозной loopback TCP (два кадра приходят, расшифровываются,
`seq`/`timestamp`/`ssrc` и полезная нагрузка совпадают), отказ на пустом и
переразмерном кадре, переход `seq`/`timestamp` через границу.

Один прогон полного набора упал на известном плавающем тесте
`continuous_source::tests::control_changes_mute_the_frame_waiting_for_output_capacity`
(зафиксирован в `CONTINUOUS_RELAY.md` как открытый intermittent loopback);
три отдельных прогона и повторный полный — зелёные, 79/79. Это не связывают
с изменением: код buffered-отправителя в тесте не участвует.

## Границы

- **Нет AAC-энкодера из PCM.** Модуль принимает готовые кадры AAC-LC; энкод
  потребовал бы libavcodec-энкодер, которого нет в offline-сборке. ADTS-файл
  закрывает путь «отправить готовый `.aac`».
- Первоначально транспорт существовал отдельно; ниже описаны добавленные `BufferedSession` и CLI. Полноценное управление буфером приёмника остаётся открытым.
- **Поля SETUP не подтверждены на устройстве.** Wire-spec называет состав
  полей для 103 зависящим от отправителя; набор минимальный и принят нашим же
  RX. Apple TV/HomePod не проверялись.
- Буферизованное воспроизведение на приёмнике зависит от `audioBufferSize` и
  control-канала; отправка кадров без управления буфером не является
  полноценной buffered-сессией.

---

## Buffered-аудио: ссылка на data-порт приёмника — 25 ноября 2026


## Зачем

`BufferedSender` умел писать кадры в любой `AsyncWrite`, но вызывающему
нечего было с этим сделать: ответ SETUP у приёмника надо разобрать, а
TCP-соединение к его `dataPort` — открыть и закрыть. Это слой соединения, который использует отдельная `BufferedSession` и CLI.

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
  переподключается. В следующем разделе описана отдельная `BufferedSession`.
- AAC-энкодера нет: на вход идут уже закодированные кадры или ADTS-файл.
- Аппаратной проверки нет: loopback против собственного разбора, не iPhone
  и не HomePod. Формат `[len:u16BE]` и поля ответа взяты из нашего RX и
  §C-RTP/§C-NET wire-spec.

---

## Buffered-аудио: сессия SETUP→RECORD→FLUSH→TEARDOWN — 25 ноября 2026


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

---

## `airplay send-buffered`: CLI для buffered-аудио — 25 ноября 2026


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
