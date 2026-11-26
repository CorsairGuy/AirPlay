# Остаточные возможности Станции в ядре (пункт 4 P1)

**Ревью интеграции:** это историческое сравнение с `b612eb4`, а не текущий список работ. Маркер авто-парирования уже перенесён, отдельные ветки могут быть не приняты; NQPTP ABI/reader в ядро не добавлять. Текущий статус — [AGENT-TASKS](AGENT-TASKS.md).

26 ноября 2026. Срез Станции — `a1aefc790522f271114c80c081a8e07cd6fe1e86` (2026-11-14), ядро — `origin/main` `b612eb4`.

Это инвентаризация исходников, а не подтверждение совместимости с устройством. Аппаратной проверки нет: iPhone, Apple TV, AirPort Express и NQPTP недоступны. Код Станции не копировался (§0.5 v12), проверялись только контракты и wire-факты.

Ветка задачи: `feat/station-core-gaps`. Основание — [AGENT-TASKS](AGENT-TASKS.md), P1 п.4; исходные точки — [CORE_MIGRATION_INVENTORY](https://github.com/CorsairGuy/yandex_station_airplay/blob/a1aefc790522f271114c80c081a8e07cd6fe1e86/docs/CORE_MIGRATION_INVENTORY.md), [CORE_SENDER_ADAPTER](https://github.com/CorsairGuy/yandex_station_airplay/blob/a1aefc790522f271114c80c081a8e07cd6fe1e86/docs/CORE_SENDER_ADAPTER.md), [CLOCK_MIGRATION](https://github.com/CorsairGuy/yandex_station_airplay/blob/a1aefc790522f271114c80c081a8e07cd6fe1e86/docs/CLOCK_MIGRATION.md).

## Таблица

| Функция | Станция (`a1aefc79`) | Ядро (`b612eb4`) | Статус | Задача |
|---|---|---|---|---|
| PTP-группа: один владелец на источник, общий производитель пакетов | `patches/nqptp-sender-grandmaster.patch`, `src/airplay_sender/src/ptp.rs` | `bin/airplay/src/coordinator/ptp_group.rs` (`PtpGroup::new`/`new_external`/`with_clock_service`/`run_*`), `services/media/src/tx/group.rs` | есть на уровне библиотеки; не подключено к CLI/карточке | уже учтено в [CONTINUOUS_RELAY](CONTINUOUS_RELAY.md) «Остаётся»; отдельная активация frontend, не новый контракт |
| SETPEERS на получателя | `src/airplay_sender/src/main.rs:367` — плоский список адресов, `/peer-list-changed` | `services/media/src/tx/session.rs:778,1170` — `SETPEERS` с бинарным plist, `requested`/`applied` watch, отказ получателя; объединение в `services/timesync/src/{service,host,engine}.rs` | есть, ядро строже (обновляемый watch, объединение владельцев, lease в `feat/peer-owner-leases`) | новой задачи не требуется; принятие группы реальным устройством — аппаратный этап |
| Приём SETPEERS/SETPEERSX (RX) | Shairport | `services/media/src/rx.rs:409` — разбор обоих, `other_receivers`, `set_grouped`, `set_session_peers` | есть | — |
| Маркер перемотки в медиапотоке | `src/airplay_sender/src/main.rs:38–52,422–431` — поколение считает seek’и, старая очередь отбрасывается, пустой пакет на следующем frame index | `services/media/src/tx/continuous.rs:81–100` (`pause`/`resume`/`invalidate_source`), `session.rs:1081–1116` (guard по source revision), очистка retransmit cache | **отсутствует**: ядро не отправляет сам маркер перемотки; `FLUSH` с `npt=0-` есть только при старте сессии (`session.rs:888`) | **G1** |
| Импорт identity/pairing | `src/airplay_sender/src/pairing.rs:41–169` — `Store`/`Controller`, ключи accessory, `mark_auto_pair_tried`; перенос — `legacy_state::prepare` → `publish_new` | `services/identity/src/sender_store.rs:90` — атомарный `SenderStore::import_new` (`cfg(target_os = "linux")`), отказ при существующем каталоге и неоднозначном снимке; маркера авто-спаривания нет | частично | **G2** |
| Discovery и физический dedup | `Device::physical_endpoints`, aliases у логических строк, отказ при неоднозначности | `services/discovery/src/catalog.rs`, `snapshot.rs` — aliases, `merge_pinned`, dedup перекрывающихся строк, отказ при конфликте физических identity (тесты есть) | есть; живой feed и Snapshot→Card — в P2 discovery | — |
| MRP/управление и карточка | `src/airplay_mrp`, 3-секундный deadline на RTSP-запрос, coalescing громкости | `crates/mrp`, `services/remote` (`card.rs`, `continuous_control.rs`), `bin/airplay/src/coordinator/output_router.rs:30,80,116` | частично: векторы и карточка — существующая задача P1 MRP | — |
| Непрерывный PCM: gap, pause 600 с, поколения, очереди, stop | поздний sender: gap grace 40 мс, pause window, bounded reader | `services/media/src/tx/continuous.rs`, `continuous_source.rs` (очереди 512, stale-слоты), `source_runtime.rs` (`run_reconfigured:63`, `run_host:144`) | есть на уровне библиотеки; не подключено к реальному источнику | уже учтено в [CONTINUOUS_RELAY](CONTINUOUS_RELAY.md) «Остаётся» |
| `SourceControl` (наблюдаемое состояние источника для pump) | card/нативный адаптер | `services/remote/src/continuous_control.rs` — `State`, revision, grace 1.5 с, Seek/Next/Previous без смены media epoch | есть; проверено тестами карточки в отчёте CONTINUOUS_RELAY | — |
| Громкость: `preserve_volume`, внешний файл-источник | `main.rs:267–300` — polling файла по mtime/ino/size раз в 100 мс, baseline, `preserve_volume` | `services/media/src/tx/session.rs` — `preserve_receiver_volume` в `SessionConfig`, `apply_volume`; внешнего polling нет | частично и **осознанно**: источник громкости — настройка обвязки, не универсального ядра | не переносить в ядро |
| Общие часы | Станция исторически использует NQPTP и 0xD7 | собственные `clock`, `timesync` и `tx/timing` | частично, dual RX/TX требует проверки | не внедрять NQPTP ABI в ядро; проверять собственные часы |

## Новые задачи из пробелов

**G1 — маркер перемотки в медиапотоке.** Добавить в TX явный контракт «перемотка»: старая очередь поколения отбрасывается до отправки, получателю уходит маркер на следующем frame index без расхода sequence/nonce для отброшенных кадров. Отдельная ветка; критерии — негативные тесты (поздний кадр старого поколения не отправляется, маркер не сдвигает media epoch, retransmit старого поколения не отдаёт байты). Wire-факт Станции — наблюдение, не гарантия протокола; проверка на устройстве обязательна. Отдельная ветка требует проверки; wire-совместимость не подтверждена.

**G2 — перенос импорта identity/pairing.** Ядро уже даёт атомарный `import_new`, но только для Linux и без маркера «авто-спаривание уже пробовали». Нужно решить: переносить ли маркер в ядро или оставить его в обвязке, и как ведёт себя `import_new` на не-Linux (явный отказ вместо тихого отсутствия). Маркер и явный отказ импорта вне Linux перенесены; реальная миграция и другие платформы остаются открыты.

**G3 — активация PTP-группы во frontend.** Уже зафиксировано в [CONTINUOUS_RELAY](CONTINUOUS_RELAY.md) «Остаётся»: подключить единый runtime к default frontend/карточке и реальным source/control adapters. Отдельная ветка, зависит от приёмки часов; **не начата**.

## Границы

- Quasar/Glagol, голос Алисы, LED, ALSA-маршруты, focus, установка/откат и device-specific источники остаются в Станции.
- Внешний NQPTP и его патчи в ядро не встраиваются; `import_new` не создаёт identity и не переносит живое состояние.
- Библиотечный API не означает подключённый frontend; mock/loopback не доказывает синхронность ЦАП.

## Проверка

Изменений кода в этой ветке нет — только документ и статус в `AGENT-TASKS.md`. Тесты не запускались: они не могут подтвердить или опровергнуть таблицу, собранную чтением исходников. Аппаратной проверки нет.
