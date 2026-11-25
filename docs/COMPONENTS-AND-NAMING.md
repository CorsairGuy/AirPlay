# Компоненты, протоколы и имена

## Состояние переименования — 2026-11-24

Пакеты библиотек переименованы без `ap_`; передающий транспорт находится в `services/media/src/tx`. Внутренний код использует новые пути компонентов. Для существующих потребителей добавлены переходные пакеты `ap_sender` и `ap_mrp`, которые только реэкспортируют `media::tx` и `mrp`, без собственной реализации. Адаптер Станции закреплён за прежним коммитом и не обновлялся; его NQPTP-интеграция в этом дереве ядра удалена, поэтому переход потребует адаптации; остальные старые имена Cargo-пакетов не являются совместимыми алиасами.

В приложении существующий код физически сгруппирован по областям `discovery`, `sessions`, `identity`, `coordinator`, `remote` и `media::{rx,tx,output}`; `timesync` остаётся общей библиотекой. Внутренние вызовы используют новые пути; прежние пути модулей сохраняются реэкспортами там, где это не нарушает dependency DAG. `discovery` владеет валидированным snapshot устройств/групп и физических адресов; `coordinator` владеет моделями групп, пользовательским выбором, resolver и protocol-neutral runtime policy. `media::tx` владеет только транспортом и физическими передающими сессиями. Это перенос имеющегося кода, а не создание отдельных процессов. Пакеты `discovery`, `identity`, `sessions`, `media`, `remote`, `coordinator`, `timesync` размещены в `services`. Медиа-события, управление локальным выводом и процессом источника вынесены из бинарной библиотеки в `services/media`. MRP output-context разбирается в компоненте `remote` и переводится в типизированный запрос coordinator. `bin/airplay` остаётся host adapter для async request loop, request-scoped authorization, `SessionPreparer` и физического media backend; coordinator принимает `PhysicalSessionBackend`, не импортируя media/session реализации. Discovery реализует read-only `OutputSnapshot` из coordinator; coordinator не ссылается обратно на discovery, поэтому граф не цикличен. Каталог переэкспортируется переходным пакетом `ap_sender::outputs`, но путь `coordinator::outputs::OutputCatalog` удалён вместе с владением каталога. Приёмная RTSP-сессия, медиаприём и часть MRP runtime также остаются в бинарной библиотеке. Консолидация альтернативной реализации MRP `services/remote` остаётся отдельной работой.

NQPTP-специфичные reader/control/ABI-адаптеры удалены из ядра; собственный timesync сохраняется, его полная совместимость с PTP-профилями AirPlay ещё требует подтверждения.

Следующая инвентаризация относится к срезу до переименования; её имена и цифры сохранены как история.

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

## Целевая схема: один daemon, семь логических runtime-компонентов

Число компонентов не определяет число процессов. Базовый deployment — один `airplay`
без подкоманды; библиотеки доступны аппаратным адаптерам без обязательного CLI.
Семь — выбранные границы ответственности, а не обещание семи готовых служб или
вечный лимит. Новый протокол добавляется в существующую границу, пока не доказана
отдельная ответственность/владение ресурсом.

| Runtime | Целевое имя пакета | Единоличное владение | Что не нужно запускать отдельно |
|---|---|---|---|
| discovery | ap_discovery | объявления, browsing, device registry, interface scope | отдельный daemon для каждого mDNS service type/BLE маячка |
| sessions | ap_sessions | listeners, pairing dispatch, RX/TX session lifecycle и выбранный профиль | по процессу на AP1/AP2/RTSP/HTTP |
| timesync | timesync | общий physical clock, PTP/NTP sockets/roles/maps, consumers/leases | по clock-service на группу/получателя |
| media | ap_media | pipeline supervision, decode/capture/render scheduling, backend factories | Apple-подобный mediaserverd только ради имени |
| remote | ap_remote | MRP/Companion/DACP endpoints, metadata/views и translation в команды | отдельный mediaremote daemon и Companion daemon по умолчанию |
| coordinator | ap_coordinator | источники, desired/confirmed membership, group volume, serialized intent/state | второй coordinator поверх уже существующего Coordinator/worker |
| identity | ap_identity | долгоживущие device/controller identities, pairing records, trust/access policy и их сохранение | второй pairing/crypto engine или отдельный key daemon по умолчанию |

Общее configuration storage, local IPC, observability и supervisor — инфраструктура daemon,
не ещё по службе на каждую функцию. Они не должны дублировать состояние coordinator.
Если coordinator публикует интерфейс, remote использует его; не ведёт свою альтернативную группу.

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
| PTP / NTP | разные timing protocols | ap_clock — pure protocol/math; timesync — один runtime owner; не смешивать wire semantics |
| HAP pairing / SRP / TLV8 | authentication profiles/primitives | ap_pair + ap_crypto; Companion не получает второй SRP/HKDF/AEAD engine |
| FairPlay | профиль negotiation/crypto | ap_fairplay, общий для поддержанных RX/video profiles |
| MRP | Media Remote Protocol | ap_mrp — один framing/protobuf/schema; remote::mrp — один server/client runtime |
| Companion Link | самостоятельный control protocol | ap_companion; общий crypto/pairing/command/event model; OPACK — его data codec, не OS service |
| DACP | reverse-control protocol | remote::dacp, shared HTTP/transport; не заменять его MRP и не обобщать «DACP только AP1» |
| DMAP | tagged metadata format | один metadata codec, сейчас events::Metadata::from_dmap; не новая сетевая служба |
| HLS/URL casting | медиа delivery profile | opt-in `hls` принимает V1 `/play` и V2 `NTP SETUP`→`RECORD`→`/play`→`/rate?value=1`, декодирует конечный HTTP(S) VOD в `FrameSink`; пауза, seek, live, аудио и capability bits пока не реализованы |
| MLP1 / local IPC | внутренние контракты | один encoder/decoder и version contract; не протокол Apple и не отдельный AirPlay stack |

## Подтверждённые дубли и места консолидации

| Место | Факт | Решение/критерий удаления |
|---|---|---|
| crates/mrp/src/wire.rs ↔ bin/airplay/src/mrp_service/wire.rs | два protobuf Node/encode/decode/packet parser с разными error API | канонический ap_mrp; перенос недостающих возможностей, differential malformed/boundary/unknown-field tests, затем удалить второй parser |
| crates/mrp/src/schema.rs ↔ mrp_service/schema.rs | две таблицы той же wire-схемы и lookup API | одна генерация из закреплённого schema source; compatibility lookup может быть wrapper без копии таблицы |
| mrp/frame.rs ↔ mrp_service/server.rs framing | общий MRP envelope реализован и в библиотеке, и вручную в сервере | один codec, сервер держит только I/O/session/push; сохранить реальные frame boundaries |
| services/mediaremote ↔ active mrp_service server/card | два MRP server path, generic старый и перенесённый активный | целевой ap_remote::mrp собирается из проверенного active behavior; старый server удалить после feature/differential parity, не поддерживать два |
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
- Runtime packages — `ap_<role>` в services/<role>; seven target roles приведены выше.
  Существующий timesync сохраняет имя пакета и логического компонента;
  переименование в timing не требуется. Другие переименования — move/reexport, затем снятие alias.
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
   построить dependency DAG: protocol/libs → runtime → daemon/adapters, без remote↔coordinator
   циклов. Общая state/event model — только при доказанной потребности.
5. Переименования runtime-пакетов и переносы из bin — отдельными move-коммитами с
   compatibility aliases, audio-only tree check и проверкой Station core pin/lock/package.
6. Companion реализуется в единственном существующем ap_companion, с общими pairing/crypto;
   coordinator не требует нового протокола. HLS сейчас имеет ограниченный
   HTTP(S) VOD-to-`FrameSink` pipeline с opt-in V1/V2 start/stop routes;
   HLS live/audio, pause/seek и capability advertisement не готовы. Capture/IPC
   остаются placeholders. Ни один
   из этих компонентов не рекламировать готовым без сквозной реализации.
7. По каждому profile фиксировать: enabled components, actual processes/binaries, listeners,
   physical-clock owner и failure ownership. Не поддерживать фиксированную рекламную цифру служб.

Никаких запусков/переименований установленной Станции эта архитектурная схема не выполняет.

## Полный реестр Cargo-пакетов — сверено 2026-11-25

Реестр собран из `Cargo.toml` и `find <пакет>/src -name '*.rs'`; предыдущая
таблица (срез 13 ноября, 24 пакета, имена `ap_*`) заменена. Пакеты переименованы
без `ap_`; `crates/sender` растворён в `services/media/src/tx`;
`services/mediaremote` и `services/mediaserver` удалены; добавлены переходные
`compat/ap_mrp` и `compat/ap_sender`.

| Путь | Пакет | Rust src файлов | Строк в src | Статус |
|---|---|---:|---:|---|
| crates/audio | audio | 3 | 527 | есть код; полнота см. STATUS |
| crates/audio_stream | audio_stream | 3 | 813 | есть код; полнота см. STATUS |
| crates/base | base | 1 | 1 | заглушка |
| crates/capture | capture | 1 | 1 | заглушка |
| crates/clock | clock | 12 | 4164 | есть код; полнота см. STATUS |
| crates/codec | codec | 4 | 788 | есть код; полнота см. STATUS |
| crates/companion | companion | 1 | 1 | заглушка |
| crates/crypto | crypto | 7 | 697 | есть код; полнота см. STATUS |
| crates/device | device | 1 | 424 | есть код; полнота см. STATUS |
| crates/fairplay | fairplay | 4 | 7004 | есть код; FairPlay v2 отклоняется в sap.rs |
| crates/hls | hls | 4 | 831 | opt-in HTTP(S) VOD -> FrameSink; live/audio/pause/seek нет |
| crates/ipc | ipc | 1 | 1 | заглушка |
| crates/mdns | mdns | 2 | 242 | есть код; полнота см. STATUS |
| crates/mirror | mirror | 2 | 479 | есть код; полнота см. STATUS |
| crates/mrp | mrp | 7 | 3543 | wire/schema/messages; вторая схема в services/remote |
| crates/pair | pair | 6 | 1109 | есть код; полнота см. STATUS |
| crates/rtp | rtp | 3 | 661 | есть код; полнота см. STATUS |
| crates/rtsp | rtsp | 1 | 290 | есть код; полнота см. STATUS |
| crates/video | video | 4 | 603 | есть код; полнота см. STATUS |
| services/coordinator | coordinator | 6 | 963 | модель групп, resolver, runtime policy |
| services/discovery | discovery | 4 | 743 | browse и валидированный snapshot |
| services/identity | identity | 5 | 1393 | identity, trust, данные сопряжений |
| services/media | media | 18 | 5910 | RX/TX, SessionPool, continuous, local output |
| services/remote | remote | 6 | 3210 | активный MRP server/card runtime |
| services/sessions | sessions | 2 | 308 | подготовка сессий и политика авторизации |
| services/timesync | timesync | 10 | 3684 | PTP/NTP, общие часы, внешние источники |
| bin/airplay | airplay | 29 | 10859 | daemon, CLI и карточка в одном бинарнике |
| compat/ap_mrp | ap_mrp | 1 | 3 | только реэкспорт `mrp` |
| compat/ap_sender | ap_sender | 1 | 12 | только реэкспорт `media::tx` |

`Строк в src` — `wc -l` по всем `.rs` в `src`, включая комментарии, тесты и
generated-таблицы; это инвентаризация, а не метрика полноты. 29 пакетов не
означают 29 процессов: базовый deployment — один daemon `airplay`.


## Согласованное уточнение — 2026-11-13

Пользователь согласовал семь компонентов: discovery, sessions, timesync, media,
remote, coordinator, identity. Это актуальная целевая схема вместо предыдущих шести.
remote — внешний пульт/карточка, coordinator — согласование действий и групп;
identity — долгоживущее состояние и trust/access policy. Sessions проверяет доступ
через этот policy, а существующие ap_pair/ap_crypto исполняют handshake и криптографию.
Identity не реализует второй SRP/HAP engine и не дублирует session state. Новые имена
ap_remote/ap_identity — целевые, пакетов с этими именами пока не создано. Текущие
SenderStore/FileStore/Identity и pairing code должны перемещаться/обобщаться с
сохранением device/controller roles, форматов, ключей и тестов, а не переписываться.
Не выделять семь процессов или семь crates без реальной необходимости.


CLI-решение пользователя2026-11-13: единственный binary `airplay` запускает daemon
без подкоманды. `serve` удалён, alias/fallback не предусмотрен. Help/version/send и
другие явные подкоманды не запускают daemon. `airplay.service` — только необязательная
интеграция systemd (ExecStart=/path/airplay), не binary/runtime-компонент. Станция
использует свой supervisor; service unit ей не навязывается и здесь не устанавливается.

## Решение о будущей реорганизации — 2026-11-13

При общем переименовании убрать `ap_` у библиотечных packages; сейчас имена и
Cargo-зависимости не менять. Отдельный sender не становится вторым стеком:
его lifecycle переносится в sessions::tx, медиатракт — в media::tx;
приём симметрично sessions::rx/media::rx. RTSP/RTP/pairing/crypto/timing общие
для направлений. Coordinator согласует ретрансляцию и группы. Это будущие
границы владения, не требование немедленно создавать отдельные crates.


Уточнение пользователя2026-11-13: timesync — единое имя пакета и логического
компонента вместо timing. Границы ответственности и единый владелец часов прежние.

Дополнение2026-11-13: пользователь согласовал media::rx, media::tx,
media::output (локальный вывод); codec остаётся общим. Текущий audio_stream
разбирается по ответственности, audio становится модулем output, а не
переименовывается в audio_output. Отдельный output crate — только при
необходимости, не обязательный новый процесс или пакет.
