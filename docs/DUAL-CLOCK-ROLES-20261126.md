# Две роли часов на одной паре портов

Ветка `feat/dual-clock-roles`, база — `main` `b612eb4`, 2026-11-26.
Задача **A** из [AGENT-TASKS](AGENT-TASKS.md) (итоги инвентаризации — ветка
`feat/station-core-inventory`, `STATION-CORE-INVENTORY-20261126.md`).

## Что было не так

`services/timesync::engine::Engine` выбирал **одну** роль на весь процесс:
`Snapshot.selection` — это либо `Listening`, либо `Follower(peer)`, либо
`Master`. Патч Станции (`patches/nqptp-sender-grandmaster.patch`, срез
`a1aefc79`) держит в одном процессе и на одной паре UDP 319/320 сразу две
независимые роли: приёмник следует за часами источника, а отправитель этой же
машины служит grandmaster'ом для своих получателей. В ядре такая комбинация
была невыразима: выбрав `Follower`, движок не отдавал Sync/Announce, а выбрав
`Master`, не следовал за источником.

## Что сделано

`services/timesync/src/engine.rs`:

- `Engine::new_dual(...)` создаёт **явный сосуществующий исходящий master**
  (`outbound: Option<Master>`, `outbound_domain: Option<Domain>`) в нашем
  собственном домене. Он живёт независимо от выбора роли: работает и когда
  роль `Listening`, и когда `Follower`, и когда `Master`.
- Выбранная роль больше не создаёт второй исходящий master: `transition` для
  `Role::Master` пропускает создание, если `outbound` уже есть. Дублирующего
  потока под тем же clock ID не возникает.
- `Snapshot` получил `outbound: Option<ClockMap>` — отображение нашей шкалы для
  получателей. Оно пересчитывается в `refresh` **до** ветвления по роли, чтобы
  ранний выход (инвалидация входящего отображения) не публиковал устаревшее
  или пустое значение; `ClockMap` с нулевым rate, домен — наш.
- `receive` для `Delay_Req` (kind 1) и Signaling (kind 0xc) отвечает от
  `outbound` с чтением **нашего** домена, даже когда выбранная роль следует за
  чужим. `complete` закрывает двухшаговый Sync (`Origin::Outbound`) той же
  шкалой.
- `tick` добавляет исходящий поток к эмиссиям выбранной роли.
- `set_peers` обновляет и `outbound`; `fault()` гасит его вместе с остальными
  ролями — после разрыва физических часов пара не продолжает отдавать чужое
  время.

`services/timesync/src/media.rs`:

- `MasterTimeline::new_dual(...)` строит продюсерскую шкалу по
  `Snapshot.outbound`, поэтому relay может отправлять звук, пока узел сам
  следует за входящим источником. Обычный `MasterTimeline::new` по-прежнему
  требует `Role::Master` + `Quality::Local`; `selected: bool` разделяет эти
  режимы, проверка домена и нулевого rate осталась общей.
- Входящее отображение никогда не переиспользуется как исходящее: сравнение
  `mapping.domain() != self.domain` отклонит чужой домен.

`services/timesync/src/service.rs`, `host.rs`, `lib.rs`:

- `Service::bind_dual`, `Host::bind_dual`, `TimeSync::start_host_dual` —
  opt-in варианты на той же паре портов; обычные `bind` не менялись.
- `Host::outbound_reading` / `TimeSync::outbound_reading` — часы, которые узел
  предлагает получателям (доступны и во время следования).
- `host.rs`: конфигурация службы собирается один раз, ветвится только выбор
  `bind`/`bind_dual`.

## Проверки

```
cargo test --offline -p timesync --lib      -> 42 passed
cargo test --offline -p airplay --no-run    -> собирается
cargo clippy --offline --all-targets -p timesync -- -D warnings  -> чисто
cargo fmt --check -p timesync               -> чисто
```

Новые тесты:

- `engine::tests::dual_engine_serves_receivers_while_following_an_inbound_source`:
  исходящий master эмитит ещё до выбора роли и несёт наш clock ID; после
  трёх Announce чужого grandmaster роль становится `Follower(peer)` с
  `Quality::OneWay`, домен `50`, а `Snapshot.outbound` остаётся нашим (`100`,
  rate 0) и не равен входящему; после захвата исходящий поток продолжается, и
  все эмиссии `tick` имеют `Origin::Outbound`. Обычный `Engine::new` публикует
  `outbound: None`.
- `media::tests::dual_timeline_reads_the_outbound_map_while_following_inbound`:
  `new_dual` читает нашу шкалу при `Role::Follower`; `MasterTimeline::new` на
  том же снимке отказывает; исчезновение `outbound` немедленно лишает
  продюсера шкалы.
- `service::tests::dual_service_follows_a_source_and_serves_receivers_on_one_pair`:
  loopback — служба следует за удалённым master'ом (домен 50, OneWay) и с тех
  же сокетов продолжает отдавать Sync с нашим clock ID 100.

Регрессии: 39 прежних тестов `timesync` проходят; `bin/airplay` собирается
(в его тестовом снимке добавлено `outbound: None`).

## Ограничения

- **Аппаратной проверки нет.** Нет ни iPhone, ни Apple TV, ни получателей:
  проверены логика и loopback, не синхронизация и не звук.
- База часов. Ядро считает физическим источником `Instant`-эпоху процесса
  (`clock::now_ns()`), а не `CLOCK_MONOTONIC_RAW`, как патч Станции. Для
  внешних часов это выражается явно (`InstantSource::new(..., network_origin_ns)`
  плюс измеренная корреляция `raw_correlation`), а не подстановкой RAW-значения
  в `InstantSource`. Измеренная корреляция и аттестация демона — задачи C/D
  инвентаризации, здесь не реализованы.
- «Общий осциллятор» не выдаётся за общего сетевого grandmaster'а: входящий и
  исходящий домены сравниваются и никогда не смешиваются, но доказательством
  синхронности это не является.
- Предел `MASTER_MAX_PEERS 16` из патча не переносится: в ядре действует
  общий предел службы (64 пира) и объединение владельцев.
- Probe по-прежнему работает только когда роль не `Master`; в dual-режиме он
  доступен во время следования, отдельного «измеряй, пока сам master» нет.
- `TimeSync::idle`/`start` (без `start_host`) остаются однoролевыми.
