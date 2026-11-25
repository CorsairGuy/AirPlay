# Отправка buffered-аудио (type 103) — 25 ноября 2026

Ветка `feat/buffered-audio-tx` от `main` (`847fe38`). Закрывает пункт
очереди «AP2 buffered-audio sender» на уровне транспорта. Референсы:
§C-RTP/§C-NET `AIRPLAY-WIRE-SPEC.md`, собственный RX (`audio_stream` →
`Stream::buffered`), D57/L-62 (рецепт AP2-realtime и роль `shk`).

## Что сделано

`services/media/src/tx/buffered.rs`, экспорт `media::tx::{BufferedSender,
StreamSetup, adts_frames, AdtsError}`:

- **`BufferedSender<W: AsyncWrite>`** — отправляет уже закодированные кадры
  AAC-LC: RTP-заголовок (`payload_type 0x60`), шифрование `rtp::AudioCipher`
  ключом `shk`, кадрирование `[длина:u16BE][пакет]` через
  `rtp::frame_buffered`. Счётчики `seq`, `timestamp` (+1024), nonce AEAD
  двигаются только при успешной записи; отказ на пустом или слишком большом
  кадре их не меняет.
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
- **Не подключено к CLI/`Session`.** SETUP/RECORD, порты и control-канал
  остаются у существующего `tx::Session`; проводка type 103 в него — отдельный
  шаг, потому что требует согласованного управления буфером и `audioBufferSize`.
- **Поля SETUP не подтверждены на устройстве.** Wire-spec называет состав
  полей для 103 зависящим от отправителя; набор минимальный и принят нашим же
  RX. Apple TV/HomePod не проверялись.
- Буферизованное воспроизведение на приёмнике зависит от `audioBufferSize` и
  control-канала; отправка кадров без управления буфером не является
  полноценной buffered-сессией.
