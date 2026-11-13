# Передача работы — 12 ноября 2026

Актуальная база — [STATUS](STATUS.md), [архитектура](ARCHITECTURE.md), [протокол](PROTOCOL.md). Старые указания «sender/MRP не сделаны» заменены текущей матрицей; предыдущий handoff сохранён в docs/history.

Приоритет пользователя: сначала закончить перевод Станции на ядро и проверки; затем переходить к будущим этапам v12, включая отложенные фазы2–3. План будущих этапов: [AP1/AP2 v12](AGENT-PLAN-AP12-20261112-v12.md), [эксперименты групповой кнопки](GROUP-BUTTON-EXPERIMENTS.md). Исходная v10 сохранена в истории Git; v12 объединяет v11.1, L-1–L-100 и 17 патчей; непроверенные внешние утверждения отделены от критериев и выполненного кода. План и выполненные задачи учитываются отдельно.

## Следующая работа

1. Комплексно проверить PTP group selection/peer reconciliation/cancellation; opt-in continuous/Card pipeline и raw PCM host уже есть (PCM_HOST.md), frontend/Station integration остаётся.
2. Проверить PIN на iPhone (сквозной loopback уже проходит); довести native topology/button/slider на основании захватов.
3. Подключить адаптер Станции к pin ядра, сохранив native PCM tap, AirPlay/BT, локальный голос/duck и готовое recovery/autostart.

## Ограничения

- Пользователь разрешил push и объединение в main. Установка, перезапуск и звук на домашних устройствах требуют отдельного согласия. Разрешены автономные Pi/null/loopback тесты.
- Браузер не трогать. Писать кратко по-русски.
- Одна общая PTP-служба; группа её не создаёт и не выключает. Stop + await, не hard abort для штатного cleanup.
- Голос Алисы только локально; HomePod-контент не приглушать при ответе. Не возвращать второй CDN/decode.
- Не терять отдельные незакоммиченные диагностические events.rs/audio_stream/net.rs, clock integration test и перестановку импортов card.rs. Они исключены из последнего опубликованного/tested source.
- В другом worktree airplay-core остались старый исходный audio-only черновик (уже портирован выборочно), mediaremote/MRP/sender и документы. Сверять полезные отличия, не накатывать целиком.
- Station source: поздняя ld5dy4 a538a30; интеграционная ветка сохраняет её runtime и опубликованные архивы/документацию. Локальные station-* часто являются контекстными портами/альтернативами, не списком готовых независимых функций.

Полные базовые проверки:311 core library tests;137 Station Rust tests + C focus-state. Последующие scoped-прогоны перечислены в VALIDATION.md. Glagol/deadline и standalone MRP probe перенесены в Station main ce8277f (25+6 тестов, controller check); audio-only ядра готов. Это не полный cross-platform CI и не новая физическая проверка.


## Приоритет после параллельного аудита 2026-11-13

Перенос нужных наработок Станции в ядро НЕ завершён. Сборка v12 — обновление плана,
не переключение приоритета. Сначала: общий владелец часов RX/TX, bootstrap core-host,
связка PCM/renderer с существующим guard/recovery, точная инвентаризация оставшихся
адаптерных функций и автономные проверки. Установки/реальные тесты — только с согласием.
Будущие фазы v12 пока ограничены исследованиями и подготовкой независимых тестов.
Параллельные аудиты 2026-11-13 — статический анализ, не запущенные тесты и не готовые исправления.


## Migration slice — 2026-11-13

- Core91fa31c separates raw-source/network lead from calibrated local lead; original API retained.
- Coreec1a90a adds pure output_card projection; not a production Snapshot→Card observer.
- Station main add976a contains explicit RendererProfile::run composition, pure legacy
  identity/key ImportPlan and opt-in package preflight. Default backend unchanged.
- Canonical remaining inventory lives in Station docs/CORE_MIGRATION_INVENTORY.md;
  clock seam in docs/CLOCK_MIGRATION.md. Preserve patched NQPTP sole319/320 owner while
  Shairport remains receiver; qualified external bridge and measured clock correlation
  are not implemented. No hidden RAW→Instant relabel or second listener.
- Next integration: external clock qualification/timeline seam, identity-preserving store
  transaction/import, live source/control/discovery, confirmed outputs+observed descriptors
  to Card, existing stats/guard handoff and executable packaging. No recovery rewrite.
- No device install/connect/audio test in this slice. Future v12 features remain secondary.


Next migration slice (2026-11-13):2a2c94f adds explicit Linux SenderStore::import_new
into a new private directory, preserves supplied identity, atomically refuses replacement;
f6600a0 adds external clock qualification. These are NOT live state migration or active
SHM/timeline wiring. Next clock work is real ordered reader/correlation sampler and a
qualified external timeline contract; next control work is serialized worker publication
with verified descriptor inventory, not racing two unrelated watches.


Clock acquisition update e3be6e4: ordered read-only ABI10/11 reader and Linux
RAW/Instant bracket sampler + two-sample rate estimator exist, scoped tests pass.
Do NOT treat current Station files as verified: this stage compares repository
main/pinned sources and local patch evidence only. Installed NQPTP ABI/patch/process
still needs a separately permitted read-only snapshot before activation.
Next work: combine acquisition, independent daemon/role evidence and qualified
external timeline/lease; keep existing NQPTP sole319/320 owner. Adapter remains
pinned to2a2c94f until the new clock API is actually integrated/tested there.


1d112ca connects cached external timeline to PtpProducer and group lease (39 timing,
104 application tests; limits in VALIDATION). Next: actual attested owner combining
reader/correlation scheduling and legacy peer control, then Station frontend/guards.
Re-audit naming: COMPONENTS-AND-NAMING.md inventories24 packages and recommends
7 logical runtime roles within1 daemon. Companion is a stub, existing Coordinator
is not a daemon; MRP wire/schema/frame/server duplicates are priority consolidation.
Approved roles: discovery/sessions/timing/media/remote/coordinator/identity.
remote replaces the proposed control name; identity owns persistent trust/state,
not a second pairing implementation.
Do not implement a second protocol or independent coordinator. Move/reuse existing
code, preserve profile-specific state and test parity before removing legacy.


CLI a8b32e9: airplay without subcommand now starts daemon; serve is removed without
alias. Help/version/send etc remain explicit commands. No binary split/airplayd.
Optional systemd unit would ExecStart=/path/airplay; none installed on Station.

External acquisition owner (2026-11-13): timesync::external_acquisition combines
read-only Reader, measured bracket/rate estimation and sole Publisher. Explicit
host poll validates independent selection/daemon/role evidence before/after I/O;
errors latch revocation. Torn/unpublished SHM alone may retry without extending
freshness. Host must schedule on a blocking/acquisition path, supply validated
platform error bounds and actual evidence, and await first publication before
creating an external PtpGroup. No tasks, PTP sockets, peer control or activation
are started. Remaining: real daemon evidence/peer-control owner, Station frontend,
existing renderer guard handoff and acceptance. This is not installed on Station.

Continuation2026-11-13: Station adapter main23b7412 pins core5c21292 and connects
external acquisition to PtpGroup without a new timing implementation/listener.
11 adapter tests and10 package preflight fixtures passed; no device activation.
Remaining: actual attested daemon/role provider and acquisition scheduler, owner-safe
peer control, source/control/discovery frontend wiring, confirmed descriptor/Card
publication and existing guard/readiness integration. Do not replace recovery.

CI repair also found generic RX omission of render interval: realtime reply now
explicitly reports latencyMin=0 (1388569). HomePod TX fallback/calibration unchanged.
Linux/Windows/macOS plus audio-only/interop/ARM checks pass on a3f0980. Local
all-target Clippy passes after moving engine tests below implementation; see
VALIDATION and current GitHub Actions for the final full matrix.
Names agreed: timesync for both component and package; media::rx, media::tx,
media::output; common codec. Remove ap_ from library/target package names only
at the later coordinated rename, not now.
