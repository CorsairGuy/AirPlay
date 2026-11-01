# Передача работы (WIP)

Ветка `claude/project-thread-gdvx2x-wip` = main + WIP-коммит (PCM и эти заметки).

## Состояние

1. **PR #10** (события приёмника, сдвиги задержки, igl/gcgl в /info) — слит.
2. **Коммит «Карточка „Сейчас играет“ по MRP»** — готов, все проверки проходили
   (fmt, clippy с ffmpeg и без, тесты, кросс-clippy armv7/windows/macos, interop pyatv `mrp()`).
   Состав: `crates/mrp` (кадры, protobuf без кодогенерации, схема из протоколов pyatv,
   `tools/schema.py`), `services/mediaremote` (служба MRP), `bin/airplay/src/card.rs`,
   флаг `serve --mrp` (и прежний `--mrp-bridge`). Слит как PR #11.
3. **WIP: перенос airplay_sender со станции** (решение Кирилла: «он уже играл на HomePod»).
   Сделано: приёмник понимает PCM `ct=1` (`Codec::Pcm` в `crates/codec`, `realtime_format`
   в `bin/airplay/src/media.rs`), тест `pcm_big_endian`.
   Не сделано — сам отправитель.

## Что делать с отправителем

Исходник: репозиторий станции, ветка `feature/group-local-sync`, `src/airplay_sender/src/main.rs`.
Переписать на общих слоях, без своей криптографии:

- сопряжение: `pair::hap::ControllerSetup::new(identity, "", true)` → `m1`/`m3`/`check_m4`
  (POST `/pair-pin-start`, затем `/pair-setup` по HTTP/1.1 с `X-Apple-HKP: 4`);
  `session_key()` — общий секрет;
- канал RTSP: `crypto::Channel::new(ControlKeys::controller(shared))`,
  `rtsp::Request::encode` / `rtsp::decode_response`;
- канал событий: `ControlKeys::controller_events(shared)`, на каждый запрос `200 OK`;
- часы: свой UDP-порт, ответы `timesync::ntp::reply`; добавить в `services/timesync/src/ntp.rs`
  публичное `now()` (NTP-время из `local_ns`) — им же штамповать синхропакеты;
- звук: `shk` = Events-Write-ключ (`ControlKeys::controller_events(shared).write`),
  `rtp::Header` + `rtp::AudioCipher::seal`; поток type 96, ct=1, spf 352, sr 44100, audioFormat 0x800;
- лучше вынести без-IO упаковщик в `crates/rtp` (seq, nonce, первый пакет с marker,
  синхропакет 0xD4 раз в секунду, ответы 0xD6 на 0x55 из журнала на 512 пакетов);
- порядок: SETUP (timingProtocol NTP) → SETUP потока → RECORD → FLUSH с RTP-Info → SET_PARAMETER volume,
  POST `/feedback` каждые 2 с, TEARDOWN при остановке, переподключение через 3 с;
- команда `airplay send --to HOST[:PORT]... --volume ДБ --latency-ms N`, stdin — S16BE 44,1 кГц стерео,
  темп реального времени (в `bin/airplay/src/main.rs` `send` сейчас «ещё не реализовано»);
  `--local-cmd` станции — позже;
- тест: отправитель → наш приёмник (`serve::run` с `audio.sink` из `audio::SinkFactory`,
  проверить, что пришли сэмплы).

Общая карта слоёв: https://claude.ai/artifact/HLPHXg7GYN3nBoqu9hUavq — строка «Отправитель».

## Правила проекта

Общение с Кириллом по-русски. Один код на протокол, без дублей. Репозиторий станции только читать.
На станции тестовый экземпляр только в /data, рядом с shairport, всегда `--audio null`.
Процессы убивать по PID, не `pkill -f`. Без force-push.

## MRP fix verified — 2026-11-01

Original in-process duplex path, no temporary TCP backend. Preserve plaintext MRP frame boundaries before encryption; remove encryption of discarded updateInfo so the first event command uses the correct nonce. Tests cover ACK/DeviceInfo encrypted boundaries, fragmented input, TCP/duplex parity and first remote commands. Shared RTSP encoder test expects explicit Content-Length: 0.

Validation: cargo test --workspace --locked — 128 passed, 0 failed, 0 ignored, no compiler warnings. Ten read-only localhost reconnects received DeviceInfo and state. User confirmed second iPhone opens the card repeatedly and remains connected >=30 seconds. No playback/long soak coverage claimed.

Only isolated Pi diagnostic was used, null audio. Physical Station and Bluetooth untouched. All commits local, no GitHub push. Detailed evidence and backups: outputs/MRP-INTEGRATION-2026-11-01.md and work/mrp-card-investigation-20261101 in the parent Codex workspace.

Realtime state follow-up: media.rs now reports playing on realtime stream replacement and paused when its actual audio owner releases it. Unrelated controller closure and duplicate release do not pause active audio. Buffered anchors unchanged. cargo test -p airplay --locked: 28 passed, 0 failed, no warnings. Live pause/resume UI confirmation pending. No changes to RTP/codec/playout, Station or Bluetooth.

Realtime flicker follow-up: playing is no longer emitted at SETUP. Stream-owned observer announces it only after the first accepted/decrypted realtime audio packet; dropping Stream aborts observer. Receiver 28/28 and audio_stream 4/4 tests pass, no warnings. Regression rejects setup-only and invalid UDP as playing, verifies authenticated audio, audio owner isolation and duplicate release. Live flicker result pending; do not claim HomePod-equivalent UX yet. Station untouched.

Explicit sender playback state handler: 29 receiver tests passed, no warnings; build passed. Trial H now on same null Pi receiver, log trial-H-sender-state.log includes command type and state (no payload dump). User asked to reconnect and compare pause latency/flicker. Live result pending. This is based on pinned Shairport command semantics, not an assumed timeout. No Station install or GitHub push.

Controller immediate state and timeline fix: accepted Play/Pause updates Feed immediately; rejected command leaves it unchanged. Feed stores monotonic position sample time and computes current elapsed position before serializing current timestamp. Pausing materializes/fixes the current position; duplicate paused feedback does not advance it. cargo test -p airplay --locked: 31 passed, no warnings; build passed. Trial I on same null Pi receiver, log trial-I-card-clock.log, user asked to reconnect and check pause for 5sec then Play from second iPhone. Live flicker/jump result pending, no full UX success claimed. Station and Bluetooth untouched, no GitHub push.
