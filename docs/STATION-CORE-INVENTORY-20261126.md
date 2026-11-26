# Инвентаризация переноса возможностей Станции в ядро

**Ревью интеграции:** это снимок `b612eb4`; некоторые элементы после него изменились. Историческая возможность работы через NQPTP SHM описана исключительно для понимания Станции: пользователь запретил NQPTP-специфичный backend в ядре. Задача C ниже исправлена соответственно. Актуальный статус — [AGENT-TASKS](AGENT-TASKS.md).

Ветка `feat/station-core-inventory`, база — `main` `b612eb4`, 2026-11-26.
Задача P1 из [AGENT-TASKS](AGENT-TASKS.md): зафиксировать таблицу
**функция / файл и ревизия Станции / файл ядра / состояние / доказательство /
конкретная задача**, затем вести перенос отдельными задачами.

## Что и как сверялось

- Станция: закреплённый срез
  [`a1aefc79`](https://github.com/CorsairGuy/yandex_station_airplay/tree/a1aefc790522f271114c80c081a8e07cd6fe1e86)
  (приватный репозиторий; срез склонирован локально только для чтения).
  Документы: `docs/CORE_MIGRATION_INVENTORY.md`, `docs/CLOCK_MIGRATION.md`,
  `docs/CORE_SENDER_ADAPTER.md`, `docs/RELAY_TIMING_CANDIDATE.md`,
  `patches/nqptp-sender-grandmaster.patch`.
- Ядро: фактический `main` (`b612eb4`), чтение кода, а не документов о нём.
- Это инвентаризация исходников, **не** проверка установленного устройства и не
  подтверждение совместимости. Loopback-тесты ядра не заменяют аппаратный этап.
- Код Станции не копировался: фиксируются только наблюдаемые wire-факты и
  границы, как требует §0.5 плана v12.

## Таблица

| # | Функция | Станция (`a1aefc79`) | Ядро (`b612eb4`) | Состояние | Доказательство | Задача |
|---|---|---|---|---|---|---|
| 1 | Одна пара UDP 319/320 на процесс | `patches/nqptp-sender-grandmaster.patch`: `nqptp-master.c`, «one process, one pair of ports, both roles at once» | `services/timesync/src/service.rs:1-5`: «Exactly one socket pair and one injected physical ClockSource serve all selected roles»; `services/timesync/src/host.rs:37-52` привязывает 319/320 на явном адресе | **частично** | пара портов одна, но роль выбирается одна (см. №2) | №2 |
| 2 | Одновременные входящий follower + исходящий master | патч: мастер-роль живёт внутри того же демона, который принимает часы отправителя | `services/timesync/src/engine.rs:1`: «One role controller and one selected mapping»; `Snapshot.selection` — одна `role::{Listening,Follower,Master}` | **отсутствует** | выбора двух ролей одновременно в типе нет | A |
| 3 | Sync 125 мс, Follow_Up, Announce 1 с, Delay_Resp | комментарий патча: unicast two-step Sync каждые 125 мс, Follow_Up, Announce раз в секунду, Delay_Resp на каждый Delay_Req | `crates/clock/src/ptp_sender.rs:17-18` (`SYNC_NS=125_000_000`, `ANNOUNCE_NS=1_000_000_000`), `follow_up()`:110, `announce()`:94, `receive_peer_delay()`:347 | **есть** | константы и генераторы сообщений совпадают с описанием патча | — |
| 4 | Signaling/TLV (запрос интервалов) | не описан в патче отдельно | `crates/clock/src/ptp_sender.rs:358-387`: grants для `0xb`/interval 0 и Sync/interval −3 | **есть; требует сверки с устройством** | код принимает только эти два интервала | B |
| 5 | Часы мастера = CLOCK_MONOTONIC_RAW, offset 0 | патч: `get_time_now`, «same base NQPTP's clients read local time in» | ядро использует `clock::now_ns()` (Instant-эпоха процесса), не RAW | **расхождение** | `crates/clock/src/lib.rs:74-79`; `crates/clock/src/external.rs:1-4` прямо требует измеренной корреляции | A |
| 6 | Владение пирами, объединение | патч: один `owner`, `MASTER_MAX_PEERS 16`, keepalive 4 с, timeout 15 с | `services/timesync/src/peer_interests.rs`; `service::set_group_peers` (`service.rs:568-571`) — union по владельцам, явное освобождение | **есть, шире Станции** | у ядра нет предела 16 и одного владельца | — |
| 7 | Чтение состояния NQPTP по SHM (ABI10/ABI11) | `src/airplay_sender/src/ptp.rs:19-68` (по `CLOCK_MIGRATION.md`): два одинаковых снимка, 72/88 байт, `clock_change_offset` в ABI11 | `grep -rn nqptp crates services bin` — **ничего**; есть только контракт `clock::external::Snapshot` (clock_id, local_time, offset, clock_change_offset, master_start_time) и квалификатор | **частично** | квалификатор и корреляция есть (`raw_correlation.rs`, `external_media.rs`), живого чтения SHM нет | C |
| 8 | Квалифицированный внешний timeline для исходящего PTP | адаптер Станции подаёт внешние часы в группу | `bin/airplay/src/coordinator/ptp_group.rs:11-22` (`GroupTimeline::Local|External`), `timesync::external_media::Timeline`, `MasterLease::spawn_external` (`media.rs:89`) | **есть на уровне библиотеки** | внешняя шкала не подменяется на `MasterTimeline` | D (attestation) |
| 9 | Relay timing RX → TX | `docs/RELAY_TIMING_CANDIDATE.md` | `services/timesync/src/media.rs` (`InstantSource`, `MasterTimeline`), `ptp_group.rs` `ContinuousSource { timeline, clock, input, controls }` | **частично** | библиотечная композиция есть; сквозного хоста и измерений нет | E |
| 10 | Пауза/перезапуск/смена источника без сброса медиавремени | станционный источник и `LocalSync` | `services/media/src/tx/continuous.rs:55-102` (`generation`, `pause`, `resume`, `invalidate_source`), тест `pause_resume_and_source_changes_never_reset_media_time` | **есть** | отдельного `seek` как API нет — перемотка выражается сменой источника/generation | — |
| 11 | Очередь и stop/await источника | `source_broker`, `runtime::Session` | `services/media/src/tx/source_runtime.rs:18-58` (`run_source`), `:63` (`run_reconfigured`), `continuous_source.rs` | **есть** | отмена future = broadcast stop, очистка — у вызывающего | — |
| 12 | SETUP/SETPEERS и PTP-профиль отправителя | sender adapter | `services/coordinator/src/group.rs` (`ptp_setup`, `peers`), `services/media/src/tx/timing.rs` (`PtpPlan::setup/setpeers/anchor`), тесты `setup_peers_and_anchor_use_one_clock_and_correct_frame_mapping` | **есть** | — | — |
| 13 | Локальный вывод: PCM + deadline, отдельный local lead | адаптер `RendererProfile`, ALSA/DSP | `services/media/src/tx/local_pipe.rs` (`LocalPcmPipe`), `tx/local.rs` (`LocalSink`), `tx/packet.rs` render lead | **есть контракт; калибровка — адаптер** | калибровки ЦАП в ядре нет и не должно быть | — |
| 14 | Выбор устройств, физический dedup | `outputs.rs`, bridge discovery | `services/discovery/src/catalog.rs:1-140` (`physical` map, `duplicate physical receiver identity`), `snapshot.rs:123-160` (разворот alias в физические) | **есть** | неоднозначность отвергается, а не угадывается | — |
| 15 | Persistent identity/pairing и импорт состояния | `legacy_state` → `ImportPlan` (адаптер) | `services/identity/src/sender_store.rs:87-95` (`import_new`, только Linux, без перезаписи) | **есть** | сопоставление accessory ID → physical UID остаётся задачей frontend'а миграции | — |
| 16 | Громкость: запрос/подтверждение, группа | ALSA mixer, сохранённые уровни | `services/coordinator/src/group_volume.rs`, `remote::output_control::VolumeBackend`, `media::tx::Volume` | **есть модель; hardware — Станция** | — | — |
| 17 | Карточка воспроизведения, MRP | Glagol/Alice, `airplay_mrp` | `crates/mrp`, `services/remote` (`card.rs`, `model.rs`), `bin/airplay/output_router.rs` | **есть; независимые векторы открыты** | отдельная задача P1 MRP | — |
| 18 | Guard/recovery, route restore, autostart, LED, Quasar/Glagol, ALSA-маршруты | собственные модули Станции | — | **остаётся в Станции** | по границам задачи P1 | — |

## Выявленные пробелы и предложенные задачи

Каждая подзадача — отдельная ветка от `main`, со своей записью в
[AGENT-TASKS](AGENT-TASKS.md); области общего PTP параллельно не менять.

**A. Две роли на одной паре портов.** Сейчас `Engine` выбирает одну роль и одно
отображение времени, а патч Станции держит входящий follower и исходящий master
одновременно. Нужен явный контракт: независимые `inbound` и `outbound`
состояния на одном сокете, разные `Domain` (источник ≠ наш clock ID), общий
физический осциллятор при разных сетевых grandmaster'ах, отсутствие «общего»
offset. Тесты: follower получает часы источника и одновременно отдаёт
Sync/Follow_Up получателям; смена источника отзывает исходящий lease; смена
роли не перезаписывает чужой домен. Аппаратная часть — отдельно, с реальным
отправителем и несколькими приёмниками.

**B. Проверка Signaling/TLV на устройстве.** Ядро принимает ровно два варианта
grant'а (`0xb`/0 и Sync/−3). Нужны независимые векторы и запись реального
обмена с iPhone/Apple TV; без захвата не расширять набор молча.

**C. Переход от старого внешнего clock backend.** SHM ABI10/11 остаётся исторической особенностью Станции, не задачей на новый backend ядра. Проверить потребности миграции в адаптере Станции и разработать собственную общую службу часов ядра; не внедрять зависимости NQPTP или его ABI в ядро.

**D. Аттестация источника внешних часов.** `external_media::Timeline` доверяет
переданным доказательствам роли/демона. Нужен явный, ограниченный по времени
провайдер доказательств и запрет выдавать это за измеренную синхронизацию.

**E. Сквозной маршрут RX → relay → TX.** Библиотечные части есть, но нет
сквозного теста и хоста: сохранение timestamp/epoch при пересчёте границ
доменов, local lead против network lead, пауза/перезапуск, invalidation при
потере источника или часов. Нужен loopback-тест «приём → relay → отправка» с
проверкой, что метки не сдвигаются дважды.

## Проверки

Код этой веткой не менялся. Для оценки, на что опираются строки «есть»,
прогнаны локальные тесты затронутых модулей:

```
cargo test --offline -p clock -p timesync --lib
   clock    — 66 passed
   timesync — 39 passed
```

Это подтверждает наличие и жизнеспособность локальных тестов, а не
синхронизацию с устройством.

## Ограничения

- Аппаратной проверки нет: в окружении нет ни iPhone, ни Apple TV, ни
  AirPlay-приёмников; ни одна строка таблицы не подтверждена устройством.
- Станция сверена по документам и патчу среза `a1aefc79`; её установленная
  сборка, ABI NQPTP на устройстве и заявленные Station-тесты не проверялись.
- Строки «есть» означают наличие кода и локальных тестов, а не
  wire-совместимость: `CLOCK_MIGRATION.md` и `CORE_SENDER_ADAPTER.md` прямо
  предупреждают, что loopback не доказывает синхронность ЦАП.
- Предел пиров 16 из патча не переносится: в ядре объединение владельцев без
  искусственного лимита, это сознательное расхождение.
- `seek` в `continuous.rs` нет: перемотка выражается `invalidate_source` и
  новой generation; если источнику нужен отдельный seek-сигнал, это правка
  контракта, а не «уже есть».
