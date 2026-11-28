# `agent/integration`: что здесь лежит и что нет

Отчёт агента: наблюдения требуют независимой проверки; тест часов проверен локально 28 ноября, это не сквозной relay. Правила работы — в
[AGENT-TASKS](AGENT-TASKS.md); статусы задач — там же, здесь только состав
этой ветки.

## Состав

| Что | Где | Чем подтверждено |
|---|---|---|
| Маршрут приём → relay → отправка: метка входящего домена пересчитывается один раз и совпадает с исходящим штампом | `relay_round_trip_converts_the_inbound_stamp_exactly_once`, `services/timesync/src/media.rs` | чтение кода `MasterTimeline::reading_at_now`, `clock::domain::translate`, `ClockMap::{to_local,to_network}`; первоначально не запускался у агента; координатор запустил локально 28 ноября: 1 тест прошёл |
| Статус задачи 3 (E) | `docs/AGENT-TASKS.md` | — |
| Ручной прогон iPhone → Windows: команды, W01–W10, границы стенда | [TEST-PLAN-IPHONE-WINDOWS-20261127](TEST-PLAN-IPHONE-WINDOWS-20261127.md) | текущее состояние и отказ 27 ноября — в C27 |

Ветка собрана от `origin/main` (`c27f331`); других изменений нет — только тест,
статус задачи и эта страница.

## Чего здесь нет и почему

Сводные ветки `integration/all-new-20261127` и `review/today-20261127` целиком
не переносятся (правило в `AGENT-TASKS.md`). Оттуда сознательно не взято:

- `crates/clock/src/nqptp.rs` (существует только в сводной ветке, здесь его нет) — чтение SHM внешнего NQPTP, в ядро не встраиваем.
- `crates/clock/src/external.rs` — аттестация внешних часов с `Claim` от вызывающего.
- `services/timesync/src/{engine,host,service}.rs`, `Snapshot.outbound`, `MasterTimeline::new_dual` — экспериментальный dual PTP ведёт координатор в `wip/dual-clock-peer-roles-20261126`.
- `flush_classic` + `pending`/эпоха в `crates/audio_stream/src/playout.rs` — в `main` нет ни `rebase()`, ни кандидата эпохи, поэтому тесты оттуда перенести некуда: они проверяют механизм, которого в `main` не будет.
- `runtime/examples/interop_receiver.rs` (`--mdns`, `--device`) — уже в `main` коммитом `9445520`.
- ветки `feat/c01…c87` — многие изменения перенесены выборочно; состояние каждой оставшейся ветки проверять отдельно.

## Сверка со сводной веткой

`git diff origin/main...origin/integration/all-new-20261127` — 15 файлов, и весь
этот набор совпадает с отклонённым по плану, ничего проверенного там не осталось:

- `crates/clock/src/nqptp.rs` и его `pub mod nqptp` — чтение SHM внешнего NQPTP;
- `crates/clock/src/external.rs` — аттестация внешних часов;
- `crates/audio_stream/src/playout.rs` — `consider_new_epoch`/`rebase`, кандидат
  эпохи и их тесты;
- `services/timesync/src/{engine,host,service,media}.rs` — `new_dual`,
  `outbound_reading`, `Snapshot.outbound`;
- `runtime/src/relay/group.rs` — одна строка `outbound: None` в тесте, следствие
  того же поля;
- четыре документа об этих же возможностях и старая версия статусов.

То есть утверждение `AGENT-TASKS` «проверенные изменения перенесены отдельно»
отражает вывод агента на момент проверки; полноту переноса каждой оставшейся ветки необходимо подтверждать отдельно.

## Сверка семантики RAOP `FLUSH`

`AGENT-TASKS.md` требует сверить `FLUSH`/RTP-Info по независимому источнику.
Сверено с shairport-sync, тег [`5.5.2`](https://github.com/mikebrady/shairport-sync/tree/5.5.2)
(релиз 14 сентября, `7bad231`), файл [`player.c`](https://github.com/mikebrady/shairport-sync/blob/5.5.2/player.c),
функция `do_flush`:

```c
conn->flush_rtp_timestamp = timestamp; // flush all packets up to, but not including, this one.
```

В `main` этому соответствует `flush_classic(boundary)` в
`crates/audio_stream/src/playout.rs`: `retain(ts >= until)`, где `until = unwrap.peek(boundary)`,
и ватермарк `played_through = max(old, until - 1)`, чтобы запоздавшие пакеты
до границы не проигрались повторно; `boundary == 0` — очистка буфера.
Граница не включается в сброс — как в референсе. Покрыто `mod s13_tests`
(граница, опоздавший пакет, переполнение 32 бит).

Отклонение сводной ветки по этому пункту обосновано: там `FLUSH` дополнительно
объявлял новую шкалу (`rebase`/кандидат эпохи), чего референс не делает — он лишь
отбрасывает пакеты до границы и не переопределяет RTP-время.

Код не копировался: взята только семантика из комментария и проверена против
своей реализации. Лицензия: по файлу `LICENSES` репозитория собственный код
Shairport Sync — MIT (в дистрибутивных пакетах при этом указывают GPL-2.0),
сторонний код — BSD/ISC/GPLv3+.

## FLUSH при смене источника (`seek-marker`)

Тот же референс подтверждает и отправляющую сторону. `do_flush` в shairport-sync
трактует `flush_rtp_timestamp` как метку первого пакета, который остаётся жить,
поэтому `RTP-Info: seq=…;rtptime=…` в `FLUSH` должен указывать на первый пакет
нового потока, а не на последний отброшенный. В `main` это так:

- `services/media/src/tx/session.rs`: при `discontinuity` вызываются
  `clear_retransmit_cache()` и `restart_spurt()`, затем текущий кадр пакуется и его
  же `seq`/`timestamp` уходят в `FlushRequest` → `FLUSH` с `RTP-Info`;
- `crates/rtp/src/packetizer.rs::restart_spurt` ставит `marker_pending`, и ровно этот
  пакет получает `marker = true` и sync-пакет с `first = true`;
- `initial_timestamp()` = `latency_frames` — начальный `FLUSH` до старта звука
  объявляет ту же метку, что и первый пакет потока.

То есть «метка, новый sync и marker на одном и том же пакете» — не догадка, а
следствие семантики границы из референса. Тесты `buffered_session`
(`FLUSH seq=102;rtptime=4048`, отказ FLUSH, пропуск TEARDOWN) в `main` есть, но в
этом окружении не запускались.

## `Apple-Challenge` в TX: что говорит референс

Открытый вопрос задачи AirPlay 1 TX — «должен ли `Apple-Response`
проверяться» — разобран по двум независимым реализациям, обе закреплённой
ревизией:

- отправитель OwnTone [`612d6b3c`](https://github.com/owntone/owntone-server/blob/612d6b3cf34ae38bb71b6edfcd69fd7afe6ab25c/src/outputs/raop.c),
  `raop_send_req_announce`: `Apple-Challenge` — 16 случайных байт, base64 без
  паддинга, и **только при шифрованной сессии**: `if (rs->encrypt)` с
  комментарием «important for ATV3 after update 6.0». Ответ приёмника нигде не
  проверяется — в этом файле нет ни чтения, ни сравнения `Apple-Response`;
- приёмник shairport-sync [`5.5.2`](https://github.com/mikebrady/shairport-sync/blob/5.5.2/rtsp.c),
  `apple_challenge()`: подписывает `challenge || адрес сокета` (16 байт для
  IPv6, 4 для IPv4) закрытым ключом AirPort Express и отвечает заголовком
  `Apple-Response` без паддинга.

Вывод: в просмотренном коде OwnTone проверка ответа не обнаружена. Это не доказывает, что проверка невозможна или не нужна другим клиентам; безопасность и совместимость остаются открытыми. Текущее поведение ядра сопоставлено с этим референсом; аппаратной проверки нет.

Лицензии: OwnTone — GPL-2.0-or-later, shairport-sync — см. выше; код не
копировался, сверены только wire-факты.

## `et=4` и `/auth-setup`

Тот же файл OwnTone (`raop.c`, `raop_device_cb`) разбирает `et` как список
токенов через запятую и ставит флаг только на точное значение:

```c
// Value of 4 seems to indicate support (!= requirement) for auth-setup
if (strcmp(token, "4") == 0)
  re->supports_auth_setup = 1;
```

Сам шаг выполняется лишь при двух условиях — в ответе OPTIONS есть метод `POST`
(`rs->supports_post`) **и** `et` содержит 4; причина в комментарии кода: «some
AirPlay 2 speakers (Sonos beam, Airport Express fw 7.8) require this step
anyway, otherwise we get a 403 to our ANNOUNCE». Тело запроса — флаг `0x01`
(no encryption) и 32-байтный публичный ключ, ответ устройства не проверяется.

В ядре `may_support_auth_setup` (`services/media/src/tx/airplay1.rs`) проверяет
только `et`, и вызывается только из тестов; сам `/auth-setup` не реализован.
Значит для устройств из этого класса ANNOUNCE без этого шага получит 403 — это
известное ограничение совместимости, а не скрытый дефект; поддержка требует
отдельной задачи с Curve25519-обменом и `Public: POST`. Биты `ft`/`sf` из
плана v12 (например, bit 26) — другое объявление (AirPlay 2 feature flags) и с
RAOP-списком `et` не смешиваются.

## Приём (RX) по shairport-sync `5.5.2`

`rtsp.c`, `handle_setup` для classic AirPlay: ответ
`Transport: RTP/AVP/UDP;unicast;interleaved=0-1;mode=record;control_port=%d;timing_port=%d;server_port=%d`
и `Session: 1` (константа), плюс `200`.

Наш `services/media/src/raop.rs` отвечает `RTP/AVP/UDP;unicast;mode=record;server_port=…;control_port=…;timing_port=…`
и `Session: 1` — номера портов и константа сессии совпадают, **не хватает
`interleaved=0-1`** (то же расхождение, что и в TX).

`handle_announce` референса возвращает **456** («Header Field Not Valid for
Resource»), если в ANNOUNCE нет пары `aesiv`/`rsaaeskey` или их длина
после декодирования не 16 байт. Наш приём отклоняет шифрованный ANNOUNCE
кодом **501**, а открытый без ключа принимает — совпадает с логикой референса
(шифрование определяется ключом, а не списком `et`), но код отказа другой.

Шифрованный приём и FairPlay в ядре не реализованы — это записано в шапке
модуля и в статусе задачи; проверка на реальном iPhone остаётся открытой.

## SDP и SETUP по OwnTone `612d6b3c`

`raop_make_sdp` в референсе выдаёт такой набор строк (шаблон
`SDP_PLD_FMT`/`SDP_PLD_FMT_NO_ENC`):

```
v=0
o=iTunes <session_id> 0 IN <af> <local>
s=iTunes
c=IN <rs_af> <receiver address>
t=0 0
m=audio 0 RTP/AVP 96
a=rtpmap:96 AppleLossless
a=fmtp:96 352 0 16 40 10 14 2 255 0 0 44100
a=rsaaeskey:…
a=aesiv:…
```

Ключи и IV добавляются только при шифровании («important for ATV3 update 6.0»).
Совпадает у нас: порядок строк, `m=audio 0 RTP/AVP 96`, `a=rtpmap:96 AppleLossless`,
`a=fmtp:96 352 0 16 40 10 14 2 255 0 0 44100` байт-в-байт, `a=rsaaeskey`/`a=aesiv`
без паддинга только в шифрованном профиле (тесты `sdp_vectors_match_the_declared_profile`).

Расхождения (низкий риск, но записаны — приёмники обычно не опираются на эти поля):

- `o=` у нас несёт SSRC, у референса — `session_id` ANNOUNCE;
- `c=` у нас — локальный адрес и его семейство, у референса — семейство и адрес
  **приёмника**;
- в `Transport` при SETUP у референса есть `interleaved=0-1`, у нас строка
  `RTP/AVP/UDP;unicast;mode=record;control_port=…;timing_port=…` без него.

Ни одно из трёх не менялось: без сборки и реального приёмника правка была бы
непроверенной.

## mirroring TX по doubletake `5e2f1cbe`

Сверен `services/media/src/tx/mirror.rs` с референсом
[doubletake `5e2f1cbe`](https://github.com/omarroth/doubletake/tree/5e2f1cbe),
`internal/airplay/mirror.go` (лицензия — LGPL-3.0 по `LICENSE` репозитория;
код не копировался).

Совпадает: единственный альтернативный дескриптор звука при отказе (у нас на
`400|406|415|455|500|501`; в референсе — тоже ровно одна попытка другой формы),
`skipRecord` учитывается в обоих ответах SETUP, event-канал опционален, RECORD
идёт с `Session`, `Range: npt=0-`, `RTP-Info: seq=0;rtptime=0`, видео — отдельный
SETUP с одним дескриптором `type=110` (`streamConnectionID`, `latencyMs`,
`timestampInfo`) и TCP-подключением к `dataPort`, звук — `type=96` с `ct`, `spf`,
`sr`, `audioFormat`, `audioMode: default`, `usingScreen: true`, `latencyMin: 0`.

Два расхождения (записаны как открытые, код без сборки не менялся):

1. В референсе **каждый** SETUP-плист содержит `isScreenMirroringSession: true`
   и `osBuildVersion`; в нашем `audio_setup`/`video_setup` нет ни того, ни другого —
   признак «это зеркалирование» есть только как `usingScreen` в дескрипторе звука.
2. Порядок у референса — control-first: сначала SETUP только сессии с
   `combinedGetInfoWithControlSetup: true` и `updateSessionRequest: false`,
   и только если приёмник такую форму отверг — ровно один переход к media-first.
   Наш код сразу идёт legacy-путём: сессия и звук в одном SETUP. Это законный,
   но второй путь референса; приёмники, требующие control-first, не проверены.

## Эпоха воспроизведения в `main`

У открытого пункта `epoch-recovery` («серия из трёх запоздавших пакетов способна
ошибочно сбросить эпоху») в `main` нет соответствующего кода: `State.epoch` в
`crates/audio_stream/src/playout.rs` растёт только по явным сигналам —
`set_anchor` при `jump` (смена `playing`/`timeline` или расхождение якоря больше
`RESYNC_NS = 100_000_000` нс), `play_from`, `flush` (FLUSHBUFFERED), `flush_classic`
и `clear` (FLUSH/TEARDOWN). Запоздавшие пакеты не имеют пути к смене эпохи: они
отсекаются по `played_through`/`packets` в `push`, а опоздание вывода
обрабатывается курсором (`next_chunk` перепрыгивает `due`), без нового
`epoch`. Эвристика «N запоздавших пакетов → новая эпоха» существует только в
отклонённой ветке `long-session-recovery`; поэтому в `main` она не нужна и не
переносится.

Обратная сторона того же решения — открытый пункт P0: обратный скачок RTP
без `FLUSH` и без нового `SETUP` не восстанавливается. `Unwrap::extend` считает
`rtp::ts_diff` как знаковую разность (`a.wrapping_sub(b) as i32`), так что новые
пакеты получают метки **ниже** `played_through`, а `played_through` в `main` нигде
не уменьшается (только `max` и `cursor - 1`); `push` навсегда отбрасывает их как
«уже сыгранные». Восстановление сейчас — только новая сессия (новый `SETUP`
создаёт новый `Playout`) или новый `anchor`/`FLUSH` там, где они есть в протоколе;
`flush_classic` сам по себе не поможет, он только поднимает ватермарк.
Это не баг в пользу эвристики — это цена отказа от неё; но оно не покрыто
тестом. Сценарий для будущего теста: `push(1000)`, отыграть до курсора, затем
`push(200)` в том же `Playout` — пакет не должен ставиться в очередь (текущее
поведение), и это должно быть зафиксировано явно, а не оставаться случайностью.

## Проверки

```bash
cargo test --offline -p timesync --lib     # включает новый тест
cargo clippy --offline -p timesync --all-targets -- -D warnings
cargo fmt --all --check
```

Ни одна из них в этом окружении не запускалась: `cargo`/`rustc` не установлены,
установка запрещена. Тест написан по фактическим сигнатурам `main`, но
непроверенная компиляция — открытый критерий, а не выполненный.

Что всё-таки сверено: формулы `ClockMap::to_local`/`to_network` и
`translate` отдельно перенесены в Python и прогнаны на числах теста —
одиночный пересчёт 9,025 с даёт 1,025 с и совпадает с исходящим штампом,
смена входящей эпохи даёт ту же метку, «сдвиг дважды» дал бы 2,025 с,
устаревшее отображение отклоняется, а дрейф входящего домена (+250 ppb)
даёт 1 024 999 993 нс вместо 1 025 000 000 — поправка не теряется.
Это проверка арифметики контракта,
а не сборки Rust.

Дополнительно тест сверен по сигнатурам вызов-за-вызовом с определениями
`main`, без сборки: `ClockMap::new(LocalInstant, Reading, i64, u64)`
(`crates/clock/src/domain.rs`), `translate(Reading, &ClockMap, &ClockMap,
LocalInstant)`, `Reading::{domain,ns}` (`PartialEq + Debug`),
`MasterTimeline::reading_at_now(Instant, Instant) -> io::Result<Reading>`
(`services/timesync/src/media.rs`), `LocalClock::new(u128)`,
`Domain::new(u64, u64)`, `Snapshot: Copy` с полем `mapping`,
`clock::Error: Debug`. Все `.unwrap()` применимы, `ClockMap` уже импортирован в
`mod tests`, лишних импортов тест не требует. Для `reading_at_now` условия
timeline совпадают с `fixture` (role `Master`, домен `7/2`, quality `Local`,
`rate_ppb == 0`), а при `mapping = None` он возвращает `WouldBlock` — именно это и
проверяет последний `assert`.

Аппаратная проверка (iPhone ↔ Windows) этой веткой не проводилась и не
заявляется; ручной прогон 27 ноября и его отказ записаны в `AGENT-TASKS.md`
(C27).
