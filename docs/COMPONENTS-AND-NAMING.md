# Компоненты, протоколы и имена

Срез 2026-11-13. Архитектурная инвентаризация workspace и целевая схема, а не
заявление о завершённом рефакторинге или установленной конфигурации Станции.
Проверены все Cargo manifests и выполнен проход по всем src/*.rs, затем вручную
сверены точки входа/вызовы, transport/pairing/clock/MRP и план v12.
Это анализ границ и повторных реализаций, не построчный аудит корректности всего кода.
reference/ — архив, не активные workspace members; чужие незавершённые diagnostics не изменялись.

## Что реально существует

-24 workspace-пакета:20 crates,3 пакета в services/,1 пакет bin/airplay.
-123 Rust-файла в src,45 751 строка в инвентаризированном рабочем срезе, включая
  generated schema, FairPlay tables, unit tests и новые clock modules. Число меняется с кодом.
- Один application binary `airplay`; он также предоставляет Rust library.
  Cargo-пакет в services/ — библиотека, не автоматически отдельный процесс/OS unit.
- `timesync` содержит runtime и используется приёмником. `mediaremote` содержит
  альтернативный MRP server, но прямых production-вызовов из текущего приложения не найдено.
- `mediaserver`, `ap_companion`, `ap_hls`, `ap_capture`, `ap_ipc`, `ap_base` — заглушки
  без исполняемой реализации. Companion существует как запланированный протокол/пакет,
  но не как работающая отдельная служба.
- `ap_sender::group::Coordinator` существует: чистая модель Selected/Ready/Confirmed/
  Diverged с attempt generations. Она не является процессом и не включена в production
  output_worker. Уже имеющиеся output_worker/PtpGroup/selection/volume нужно связать,
  а не создавать второй независимо управляющий coordinator.
- CLI status/list/pair/set/log перечислены в help, но main пока отвечает «ещё не реализовано».
  `ap_ipc` пуст; фактический send IPC находится в приложении send_ipc.
- В core нет отдельных main для трёх каталогов services и нет оснований считать их
  тремя запущенными демонами. Bonjour service record — также не OS service.

## Целевая схема: один daemon, шесть логических runtime-компонентов

Число компонентов не определяет число процессов. Базовый deployment — один `airplay`
с командой `serve`; библиотеки доступны аппаратным адаптерам без обязательного CLI.
Шесть — выбранные границы ответственности, а не обещание шести готовых служб или
вечный лимит. Новый протокол добавляется в существующую границу, пока не доказана
отдельная ответственность/владение ресурсом.

| Runtime | Целевое имя пакета | Единоличное владение | Что не нужно запускать отдельно |
|---|---|---|---|
| discovery | ap_discovery | объявления, browsing, device registry, interface scope | отдельный daemon для каждого mDNS service type/BLE маячка |
| sessions | ap_sessions | listeners, pairing dispatch, RX/TX session lifecycle и выбранный профиль | по процессу на AP1/AP2/RTSP/HTTP |
| timing | ap_timing | общий physical clock, PTP/NTP sockets/roles/maps, consumers/leases | по clock-service на группу/получателя |
| media | ap_media | pipeline supervision, decode/capture/render scheduling, backend factories | Apple-подобный mediaserverd только ради имени |
| control | ap_control | MRP/Companion/DACP endpoints, metadata/views и translation в команды | отдельный mediaremote daemon и Companion daemon по умолчанию |
| coordinator | ap_coordinator | источники, desired/confirmed membership, group volume, serialized intent/state | второй coordinator поверх уже существующего Coordinator/worker |

Configuration, storage, local IPC, observability и supervisor — инфраструктура daemon,
не ещё по службе на каждую функцию. Они не должны дублировать состояние coordinator.
Если coordinator публикует интерфейс, control использует его; не ведёт свою альтернативную группу.

Отдельный timing process допустим по подтверждённым причинам привилегий/изоляции,
но не обязателен. Пока Shairport остаётся receiver Станции, NQPTP — внешний существующий
владелец319/320; новый reader/cached timeline не превращается во второй PTP daemon.
Аппаратные helpers/adapter Станции не включаются в число универсальных служб ядра.

## Термины протоколов и кодеков

| Имя | Тип | Каноническое место/переиспользование |
|---|---|---|
| AirPlay1 / AirPlay2 | семейства session profiles | sessions; явная матрица wire/codec/auth/timing, не два монолитных стека |
| RAOP | аудио-профиль/совместимость, не имя daemon | общие ap_rtsp/ap_rtp + session profile; AP1 ещё не завершён |
| RTSP / используемый HTTP | сообщения/сессионное управление | ap_rtsp уже общий RX/TX codec; не писать ещё parser под Companion/casting |
| RTP / retransmit / anchors | медиапакеты | ap_rtp; RX/TX state machines отдельны, wire primitives общие |
| PTP / NTP | разные timing protocols | ap_clock — pure protocol/math; ap_timing — один runtime owner; не смешивать wire semantics |
| HAP pairing / SRP / TLV8 | authentication profiles/primitives | ap_pair + ap_crypto; Companion не получает второй SRP/HKDF/AEAD engine |
| FairPlay | профиль negotiation/crypto | ap_fairplay, общий для поддержанных RX/video profiles |
| MRP | Media Remote Protocol | ap_mrp — один framing/protobuf/schema; control::mrp — один server/client runtime |
| Companion Link | самостоятельный control protocol | ap_companion; общий crypto/pairing/control model; OPACK — его data codec, не OS service |
| DACP | reverse-control protocol | control::dacp, shared HTTP/transport; не заменять его MRP и не обобщать «DACP только AP1» |
| DMAP | tagged metadata format | один metadata codec, сейчас events::Metadata::from_dmap; не новая сетевая служба |
| HLS/URL casting | медиа delivery profile | ap_hls после реализации + sessions/media; capabilities объявлять по фактической готовности |
| MLP1 / local IPC | внутренние контракты | один encoder/decoder и version contract; не протокол Apple и не отдельный AirPlay stack |

## Подтверждённые дубли и места консолидации

| Место | Факт | Решение/критерий удаления |
|---|---|---|
| crates/mrp/src/wire.rs ↔ bin/airplay/src/mrp_service/wire.rs | два protobuf Node/encode/decode/packet parser с разными error API | канонический ap_mrp; перенос недостающих возможностей, differential malformed/boundary/unknown-field tests, затем удалить второй parser |
| crates/mrp/src/schema.rs ↔ mrp_service/schema.rs | две таблицы той же wire-схемы и lookup API | одна генерация из закреплённого schema source; compatibility lookup может быть wrapper без копии таблицы |
| mrp/frame.rs ↔ mrp_service/server.rs framing | общий MRP envelope реализован и в библиотеке, и вручную в сервере | один codec, сервер держит только I/O/session/push; сохранить реальные frame boundaries |
| services/mediaremote ↔ active mrp_service server/card | два MRP server path, generic старый и перенесённый активный | целевой ap_control::mrp собирается из проверенного active behavior; старый server удалить после feature/differential parity, не поддерживать два |
| TimeSync/Ptp legacy ↔ unified timesync::Service | переходные receiver loop и новый RX/TX service | один выбранный backend на хосте; не объявлять уже объединёнными, завершить clock migration до удаления legacy |
| Coordinator model ↔ worker/PtpGroup/volume/selection | разные части групповой логики, не две готовые равноценные службы | сохранить roles: model/serialized owner/session supervisor; связать attempt/evidence, не начинать новый независимый state engine |
| Station MRP/sender ↔ core | временные две реализации в разных repos во время миграции | default остаётся legacy до приёмки; затем Station использует core libraries, hardware overlay не содержит второй protocol parser |
| reference/Station drafts | архив не входит в workspace | не включать в build, не считать второй active implementation; provenance/history сохранить |

Не являются лишними дублями сами по себе: PTP RX vs TX state machines; PairSetup vs
ControllerSetup; NTP и PTP wire; RX audio buffering vs TX pacing; capture vs decode vs
hardware sink. Общие packet/math/crypto/schema переиспользуются, role-specific state
сохраняется раздельно.

Уже правильное переиспользование: sender transport использует crypto::Channel;
pair::hap обеих сторон использует единый crypto::srp и TLV8; sender NTP responder
использует timesync::ntp::reply/now, а не второй NTP codec; RTSP codec общий.

## Соглашение имён и размещение

- Binary/CLI сохраняем `airplay`; не вводим сейчас одновременно airplayd, companiond,
  coordinatord и ещё один CLI. Если появится основание разделить CLI/daemon, это отдельное
  решение с IPC/config/service migration, а не автоматическое следствие новых протоколов.
- Protocol/data libraries — `ap_<protocol/domain>`: уже имеющиеся ap_rtsp/ap_rtp/ap_mrp/
  ap_companion/ap_crypto/ap_pair/ap_clock и т.д. Имена протоколов в сети не переименовываются.
- Runtime packages — `ap_<role>` в services/<role>; six target roles приведены выше.
  Существующий timesync переводится в timing через package/import compatibility alias,
  а не копированием каталога. Все переименования — move/reexport, затем снятие alias.
- mediaserver/mediaremote — исторические Apple-подобные названия, а не обоснование
  новых процессов. Empty mediaserver не должен стать параллельным media engine.
- ap_base пока пуст: не создавать ещё один общий base рядом. Общие IDs/commands/state
  при реальной необходимости получают осмысленное model API; пустые crates не плодить.
- ap_audio/ap_audio_stream/ap_codec/ap_video/ap_mirror имеют разные роли: device sink,
  incoming audio stream, codecs, video decode/render, mirroring transport. Не объединять
  их по сходству имени и не создавать вторые equivalents в services/media.
- Compatibility station_mrp/station_elog — aliases, не отдельные реализации. Удаляются
  после перехода клиентов. Mini2/Quasar/ALSA/sysfs остаются в Station adapter.

## Порядок работ и защита от второго стека

1. Завершить проверяемый clock срез и текущую миграцию, не массово переименовывать runtime.
2. В каждом новом изменении сначала owner lookup по этой карте. Если функциональность
   уже есть, расширять/перемещать её; внешний источник — reference по §0.5 v12, не замена
   существующему протоколу. Проверенные external math/backend libraries допустимы без
   создания второй нашей реализации wire protocol.
3. Первое реальное устранение дублей: MRP codec/schema/framing, затем единый control
   runtime. Сначала сохранить обложку/команды/group/control behavior в regression fixtures.
4. Выделить coordinator из уже существующих model/worker/supervision, не написать заново;
   построить dependency DAG: protocol/libs → runtime → daemon/adapters, без control↔coordinator
   циклов. Общая state/event model — только при доказанной потребности.
5. Переименования runtime-пакетов и переносы из bin — отдельными move-коммитами с
   compatibility aliases, audio-only tree check и проверкой Station core pin/lock/package.
6. Companion реализуется в единственном существующем ap_companion, с общими pairing/crypto;
   coordinator не требует нового протокола. HLS/capture/IPC placeholders реализовывать
   только при старте соответствующих фаз, не рекламировать готовыми.
7. По каждому profile фиксировать: enabled components, actual processes/binaries, listeners,
   physical-clock owner и failure ownership. Не поддерживать фиксированную рекламную цифру служб.

Никаких запусков/переименований установленной Станции эта архитектурная схема не выполняет.

## Полный реестр текущих Cargo-пакетов

| Путь | Пакет сейчас | Rust src файлов | Code lines* | Статус |
|---|---|---:|---:|---|
| crates/audio | ap_audio | 3 | 421 | есть код; полнота см. STATUS |
| crates/audio_stream | ap_audio_stream | 3 | 680 | есть код; полнота см. STATUS |
| crates/base | ap_base | 1 | 0 | заглушка |
| crates/capture | ap_capture | 1 | 0 | заглушка |
| crates/clock | ap_clock | 13 | 3985 | есть код; полнота см. STATUS |
| crates/codec | ap_codec | 4 | 619 | есть код; полнота см. STATUS |
| crates/companion | ap_companion | 1 | 0 | заглушка |
| crates/crypto | ap_crypto | 7 | 573 | есть код; полнота см. STATUS |
| crates/device | ap_device | 1 | 323 | есть код; полнота см. STATUS |
| crates/fairplay | ap_fairplay | 4 | 6610 | есть код; полнота см. STATUS |
| crates/hls | ap_hls | 1 | 0 | заглушка |
| crates/ipc | ap_ipc | 1 | 0 | заглушка |
| crates/mdns | ap_mdns | 2 | 216 | есть код; полнота см. STATUS |
| crates/mirror | ap_mirror | 2 | 312 | есть код; полнота см. STATUS |
| crates/mrp | ap_mrp | 7 | 3323 | есть код; полнота см. STATUS |
| crates/pair | ap_pair | 6 | 957 | есть код; полнота см. STATUS |
| crates/rtp | ap_rtp | 3 | 585 | есть код; полнота см. STATUS |
| crates/rtsp | ap_rtsp | 1 | 243 | есть код; полнота см. STATUS |
| crates/sender | ap_sender | 14 | 5780 | есть код; полнота см. STATUS |
| crates/video | ap_video | 3 | 377 | есть код; полнота см. STATUS |
| services/mediaremote | mediaremote | 2 | 512 | есть код; полнота см. STATUS |
| services/mediaserver | mediaserver | 1 | 0 | заглушка |
| services/timesync | timesync | 9 | 3283 | есть код; полнота см. STATUS |
| bin/airplay | airplay | 33 | 13512 | есть код; полнота см. STATUS |

*Без пустых и отдельных comment lines; включает tests/generated data. Числа — инвентаризация, не число работающих служб.
