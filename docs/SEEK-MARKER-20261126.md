# Маркер перемотки в realtime TX (G1)

26 ноября 2026. Ветка `feat/seek-marker` от `origin/main` `b612eb4`. Задача G1 из [STATION-CORE-GAPS-20261126](STATION-CORE-GAPS-20261126.md).

Аппаратной проверки нет: iPhone, Apple TV, AirPort Express и NQPTP недоступны.

## Что сделано

Раньше ядро при смене поколения источника (`!previous.same_source_revision(&chunk)`) очищало retransmit-кэш, но никак не сообщало получателю о разрыве в медиапотоке. Теперь первый аудиопакет нового поколения помечается стандартным RTP marker bit — началом нового talkspurt.

- `crates/rtp/src/packetizer.rs`:
  - `PcmPacketizer::restart_spurt()` — «следующий пакет помечается»; ничего не отматывает.
  - `packetize` ставит `marker = first || marker_pending` и сбрасывает флаг только после успешной упаковки. Отвергнутый кадр (немонотонный index, неверная длина PCM) не расходует ни sequence, ни nonce, ни ожидающий маркер.
  - `Packetized` получил `first: bool` — признак первого пакета потока отделён от header marker.
- `services/media/src/tx/timing.rs`: `PtpPacketizer::restart_spurt()`, а d7-anchor получает `packetized.first`, поэтому смена поколения не помечает якорь как первый и не двигает отображение часов.
- `services/media/src/tx/session.rs`: `TargetPacketizer::restart_spurt()`; в `stream()` вызывается сразу после `clear_retransmit_cache()` при обнаружении новой source revision.

## Контракт

- Sequence и nonce продолжают общий счётчик: отброшенные кадры старого поколения не отправляются и не занимают номера.
- Timestamp/media epoch не сдвигаются: `first_index`/origin и `mapping_lead_frames` не меняются, frame index остаётся общим.
- Retransmit старого поколения после очистки кэша не отдаёт байты; повторных маркеров для одного поколения нет.
- `sync`/d7 first-флаг остаётся только у самого первого пакета потока.

## Границы

- Станция на перемотку дополнительно отправляет пустой пакет на следующем frame index (`src/airplay_sender/src/main.rs:38–52,422–431`). Это наблюдение конкретной реализации, а не подтверждённая гарантия протокола; в ядре такой пакет **не** отправляется. Для его введения нужен захват с реального получателя.
- Как именно iPhone/HomePod/Apple TV трактуют marker bit при перемотке, не проверено: нет устройства.
- Сетевые pause/seek-операции, согласование буферов получателя и `played_through` остаются открытыми (см. [CONTINUOUS_RELAY](CONTINUOUS_RELAY.md)).

## Проверка

```
rustup run stable-x86_64-pc-windows-gnu cargo test --offline -p rtp
  17 passed; 0 failed
rustup run stable-x86_64-pc-windows-gnu cargo test --offline -p media --lib
  126 passed; 0 failed
rustup run stable-x86_64-pc-windows-gnu cargo clippy --offline -p rtp -p media --lib -- -D warnings
  ok (только прежние предупреждения манифеста workspace)
```

Новый тест `seek_marks_one_new_talkspurt_without_moving_the_media_epoch` проверяет: marker только у первого пакета после перемотки, `sync[0]` остаётся `0x80`, sequence `9`, timestamp `first + 1000`, nonce `2`, retransmit старого пакета пуст, отвергнутый поздний кадр не расходует маркер.
