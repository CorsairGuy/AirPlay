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
