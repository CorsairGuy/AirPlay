# Companion: жизненный цикл сессии и сбои соединения

26 ноября 2026. Ветка `feat/companion-session` от `origin/main` `b612eb4`.

Референсы: план v12 D13/D41/D42, L-61/L-80; **pyatv 0.18.0, коммит
`b277a4c8222ecdcbaab8a24e3e713ca44765adb4`** (`Release 0.18.0`, 19.06.2026),
файлы `pyatv/protocols/companion/api.py` и
`pyatv/protocols/companion/__init__.py`. Код pyatv не копировался: сверены
последовательность обмена и форма полей.

Аппаратной проверки нет: Apple TV недоступен.

## Что было и чего не было

В `main` уже были OPACK, кадры с ограничением размера (`MAX_PAYLOAD` 4 МиБ) и
12-байтным LE nonce из счётчика, pair-verify (`_pairVerify` с `_pd`, X25519,
ChaCha20-Poly1305 `PV-Msg02`/`PV-Msg03`, подтверждение состояния 4) и сборка
команд. Сетевого слоя не было: [`companion::Link`] только кодировал кадры, а
вызывающий сам решал, чем ответить на обрыв и как сопоставить ответ.

## Добавлено

`crates/companion/src/session.rs` — `Session<S>` поверх `AsyncRead + AsyncWrite`:

- `Session::connect` — pair-verify открытым текстом, затем замена канала на
  зашифрованный (`Link::encrypted`); до подтверждения приёмника зашифрованных
  кадров не отправляется;
- `request` — корреляция строго по `_x`: кадры с чужим `_x` (события без `_x`,
  запоздавшие ответы) пропускаются, ответ с другим `_i` или запрос вместо ответа
  — `Error::Session`, тишина дольше дедлайна — `Error::Timeout`;
- `next_frame` — балансирует `read`/`feed`/`poll`, при EOF или ошибке чтения
  помечает сессию закрытой: следующие запросы сразу дают `Error::Closed` и не
  пишут в мёртвый поток;
- `open_remote`/`close_remote` — `_sessionStart` с `{_srvT, _sid}` и
  `_sessionStop`; `SessionId::wire()` = `(remote << 32) | local`;
- `shutdown` — закрытие потока без обмена; `close_remote` гасит поток в любом
  случае, как и референс.

## Сверка с pyatv 0.18.0

- `_sessionStart`: `{_srvT: "com.apple.tvremoteservices", _sid: <локальный 32-бит>}`;
  ответ даёт `_c._sid` приёмника, а pyatv склеивает `(remote_sid << 32) | local_sid`
  и эту склейку отправляет в `_sessionStop`. Наш прежний `commands::session_stop`
  брал `sid` как есть — исправлено введением `SessionId`; тест проверяет именно
  склейку.
- `_send_command` в pyatv кладёт `_t: 2` (request) — совпадает с нашим
  `commands::REQUEST`.
- Порядок подключения pyatv: `_systemInfo` (с непустым `_i`, иначе приёмник не
  шлёт `TVSystemStatus`) → `_touchStart` → `_sessionStart` → `TVRCSessionStart`
  → `_tiStart` → подписка `_iMC`; отключение — снятие подписок →
  `_sessionStop` → `_touchStop` → `_tiStop`, ошибки глотаются. Наша сессия
  покрывает только транспортный уровень и `_sessionStart`/`_sessionStop`; порядок
  высокоуровневых запросов остаётся за `services/remote`.
- `_hidT` в pyatv несёт `_ns`, `_tFg: 1`, `_cx`, `_cy`, `_tPh` — совпадает с
  нашими командами касания (проверено тестами `commands`).

## Тесты (`crates/companion/tests/session.rs`, локальный TCP-сервер)

- `replies_are_correlated_and_stale_frames_are_ignored` — событие без `_x` и
  запоздавший ответ чужого `_x` не подменяют ответ на наш запрос;
- `a_response_for_another_request_is_rejected` — ответ с нашим `_x`, но другим
  `_i` — `Error::Session`;
- `a_timeout_is_reported_and_the_session_stays_usable` — молчание 250 мс при
  дедлайне 50 мс даёт `Error::Timeout`, сессия остаётся живой и следующий
  запрос проходит;
- `a_disconnect_is_reported_and_blocks_further_requests` — обрыв даёт
  `Error::Closed`, повторный запрос тоже `Closed` без сетевого ввода-вывода;
- `pair_verify_enables_the_encrypted_channel_and_reconnect_starts_over` — полный
  pair-verify против mock-приёмника (M2/M4), зашифрованный `_sessionStart`,
  `_sessionStop` со склейкой SID; второй цикл — новое TCP-соединение и новые
  счётчики nonce;
- `pair_verify_rejection_leaves_nothing_behind` — отказ приёмника на M2 не даёт
  сессии.

```
cargo test --offline -p companion          36 lib + 6 session passed
cargo clippy -p companion --all-targets -D warnings   чисто
```

## Ограничения

- Обмен с настоящим Apple TV не выполнялся; совместимость не заявляется.
- `TVRCSessionStart`, `_systemInfo`, подписки на события, `_touchStart`/`_tiStart`
  и их порядок в сессии не реализованы — это уровень приложения.
- Ответы, пришедшие после истечения дедлайна, остаются в буфере кадра и будут
  отброшены по `_x` при следующем запросе; отдельной очереди «поздних» ответов
  нет.
- Смена ключей на лету и повторный pair-verify внутри одной TCP-сессии не
  поддерживаются: переподключение создаёт новый `Session` и новый `Link`.
