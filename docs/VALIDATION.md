# Проверки и границы подтверждений

Обновлено12ноября2026. Текущий код и ограничения — [STATUS](STATUS.md).

| Уровень | Подтверждено | Не следует из результата |
|---|---|---|
| Unit / loopback | 311 библиотечных тестов workspace прошли на Pi/Linux, offline, без сети контейнера | Полный cargo test с binaries/integration/doctest; все ОС/feature sets; вся GitHub CI |
| RTSP/RTP mock | Pairing/encrypted RTSP, SETPEERS, d7, UDP во время задержанного ACK, EOF/cleanup | Реальное принятие группы HomePod/Apple TV, слышимый звук |
| PTP lifecycle | Loopback/ephemeral ports; общий source, role/epoch/freshness, union owners и release | LAN PTP/BMCA completeness, follower TX servo, акустический drift |
| Карточка | Wire/replay, artwork/timeline и worker regressions | Исправление всех iPhone UI/ползунков/подписи группы |
| Станционный код | Срез4ead45f:137 Rust tests + C focus-state check | Установка этого среза и сохранность звука на физической Станции |

Локальные журналы координатора: CORE-MAIN-MERGE-CHECK-20261111.log и STATION-MAIN-MERGE-CHECK-20261111.log. Они не встроены в релиз. Исторический разовый сбой PCM loopback зарегистрирован в PTP-LOCAL-CHECK-20261111.log; повтор без изменения сборки и последний workspace прогон прошли.

Рекомендуемые проверки разработки: cargo fmt --all --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace. Эти команды являются требованиями CI, а не заявлением, что полный набор уже выполнен12ноября. FFmpeg и целевые ОС/архитектуры проверяются отдельно.

Стенд не должен запускать LAN discovery, PTP319/320 или аудио устройства автоматически. Для физической проверки требуется отдельное разрешение; сохраняются откат и отдельные сценарии Alice/AirPlay/BT. Контролировать память/rootfs, процессы и восстановление маршрута, а не только наличие UI-галочки.

Последующее дополнение12ноября: source continuous policy —68 sender library tests passed, включая5 новых случаев. Полный workspace311 выше относится к предшествующему исходному срезу; включённая continuous-сессия и аппаратный interop этим не подтверждены.

Continuous async source pump:73 sender library tests passed, включая5 новых async-сценариев; только память, bounded-каналы и таймеры. Сетевой continuous runtime и установленная Станция этим не подтверждены.

Continuous broadcast/revision guards:78 sender library tests passed; дополнили encrypted-loopback и local renderer проверки. Hardware/receiver buffers и сетевой pause/seek автомат остаются отдельной незавершённой работой.

Continuous retransmit protection:94 tests passed (16 RTP,78 sender), включая cache clear без сброса nonce/sequence/media epoch. Это не проверка remote PAUSE/FLUSH или hardware buffer reset.

Unified continuous/PTP/local lifecycle:185 library tests passed (80 application,78 sender,27 timesync). Связка source→PTP stamp→local sink проверена без выбранных remote устройств; EOF, sink failure с открытым источником, неверная эпоха и one-way cancellation. Default frontend/Station integration и hardware buffers не подтверждены.

MRP/continuous source binding:162 library tests passed (84 application,78 sender). Новый opt-in follower и реальные Control.action callbacks проверены автономно; native/BT source adapter и аппаратная задержка не подтверждены.

Card/continuous pipeline composition:164 library tests passed (86 application,78 sender). Реальные MRP Pause/Play кадры через Card::serve и in-memory duplex управляют PCM общей pipeline; проверены совместное завершение и отказ control channel другой карточки. Тестовый clock/local capture, без выбранных удалённых получателей, LAN-сервисов и аудиооборудования. Это не проверка установленной Станции.

Continuous fanout/SessionPool: 165 tests passed (application86 + sender79). Two simulated workers receive the same Arc<AudioPacket>; removing one preserves the survivor, generation changes invalidate old packets, resumed PCM keeps frame_index continuity, EOF ends the pool. This is not a two-device RTSP/PTP or native-group interop result. The first run hit the previously observed local_and_remote_outputs_share_pcm_and_presentation_deadlines failure (0 exact frames); an unchanged-build repeat passed all tests. The intermittent loopback failure remains open.

Loopback capture follow-up: replaced the first-100,000-samples recording with constant-memory streaming sample/exact-stereo-pair counters. Leading silence can no longer exhaust the capture window; split stereo writes preserve pairing. The existing exact-PCM threshold and local presentation-deadline assertions are unchanged. 166 tests passed (application87 + sender79), followed by five unchanged-build repetitions of the formerly intermittent local/remote test, all passing. This fixes a measurement defect; it does not prove that every historical failure had that cause or establish physical-device timing.

PIN owner cancellation:167 library tests passed (application88 + sender79). Explicit stop and an already closed false-valued stop channel return without PIN prompt, pairing I/O or credential writes, for both direct and persistent authorization entry points. Stop has priority when the authorization exchange is also ready. Connection-scoped interactive output selection remains unfinished.

Controller authorization scopes:168 library tests passed (application89 + sender79). Each connection router permits one active scope with a fresh bounded queue (4 responses). Two-controller isolation, overlapping-scope rejection, overflow, fresh-request queue reset, owner-drop cancellation and survival of the other controller are covered. AuthorizationScope delegates to the existing persistent PIN exchange. The MRP reader/output worker do not yet create or route these scopes automatically; this is an explicit API, not a completed interactive selection path.

Interactive selection wiring:170 library tests passed (application91 + sender79). MRP selection carries its connection context; the reader forwards bounded PIN messages/responses without awaiting pairing, and the output worker/PTP wrapper forwards context to prepare_authorized. StoredSessionPreparer adds explicit InteractivePinForUnpaired policy, rejects absent/dead context before connecting, and never falls back to transient. Existing PIN setup/store tests and controller-scope tests passed. The new selection test checks dispatch/context ownership, not a complete MRP-to-paired-media exchange; full wire integration and real iPhone interop remain pending.

PIN wire integration:172 library tests passed (application93 + sender79). Actual framed MRP over duplex reaches the output worker and interactive StoredSessionPreparer; real loopback receiver requires PIN, persists pairing, accepts a new encrypted RTSP/persistent pair-verify/media setup, selection/removal and re-selection without another PIN prompt. A separate controller-disconnect-during-PIN case saves no key, selects no output and awaits cleanup. No PCM is generated; receiver uses null audio. This is not iPhone/native-group/PTP/acoustic evidence.

Two-receiver control failure:173 library tests passed (application94 + sender79). Two actual encrypted RTSP loopback sessions receive shared PCM; aborting only the second receiver causes its worker to fail, while the first remains Running and sends a second packet at the original MediaClock/frame_index. Removing the failed worker reports its error; shutdown of the survivor returns two sent packets and Ended. Null receivers, NTP timing, no real devices or acoustic/PTP-native-group evidence. The first new-test run used an artificial500ms pairing-proof timeout and failed during setup; using the unchanged default deadline passed. Production transport code was not changed.

Build profiles: audio-only cargo check and dependency-tree exclusion passed; cargo test -p airplay --no-default-features passed94 library tests +1 CLI test (0 doctests). Default profile passed94 application +79 sender library tests +1 CLI test. Mirroring remains enabled by default; unsupported audio-only video flags and stream110 setup are rejected. NTP realtime audio support was preserved instead of copying the old draft's NTP gating. Three test-only API warnings were then fixed with cfg(test); cargo check for both profiles passed without warnings. Full CI/FFmpeg/cross-target matrix was not run.

2026-11-12 — source 37e1790c98db288040301b5b300726eca6fde9e2:178 library tests passed (application95 + sender83), Pi/Linux, --network none. Raw PCM acquisition tests cover byte preservation/aligned tail padding, partial reads across coalesced Pause/Play, invalidated queued frames, foreign control channel, cancellation during blocked reads/full queues. The raw host composition tests EOF drain and malformed-tail cancellation with resource Drop. Its first test version incorrectly required finish on cancellation; corrected to the existing sink contract (finish on EOF, Drop on cancellation). No real remote receiver was selected in the new host cases; clocks/capture sinks are fixtures. Station bootstrap, source buffers upstream of acquisition, physical audio and PTP native adoption remain unverified.


## Будущая измерительная приёмка v12 (не выполнена)

D9/L-83: кросс-корреляция известного sweep/импульса на реальных выходах,
регрессия offset по времени и прогон ≥2ч; указать sample rate, захват, калибровку
задержки ADC/DAC и доверительную погрешность. soundcheck/signal-estimator — референсы
методики с provenance-тегом, не скопированный код. Отдельно измерять scheduling jitter
и фактический выход. Заявленные авторами точности не являются точностью нашего стенда.
Bit-exact проверяется отдельно от временной синхронизации: lossless вход/выход без gain/ASRC,
побитовое сравнение и объяснение преобразований; нулевой null-test сам по себе не доказывает timing.
Добавить долгий RTP wraparound, packet loss, feedback под нагрузкой и матрицу iOS×приложение×profile.


## Calibrated PCM host — 2026-11-13

Source commit `91fa31c`: 95 application library tests passed on Pi/Linux in an offline
container (`cargo test -p airplay --lib --no-default-features --locked --offline`).
The raw-PCM host test uses 250ms source lead and asserts local write deadlines at
packet.produced_at()+375ms; EOF drain and malformed-tail Drop cleanup remain checked.
No real remote, NQPTP bridge, ALSA timing, Station installation or acoustic synchronization
was tested. Log: coordinator outputs/CORE-CALIBRATED-HOST-CHECK-20261113.log.


## Confirmed output projection — 2026-11-13

Source ec1a90a:99 audio-only application library tests passed on Pi/Linux offline
(`cargo test -p airplay --lib --no-default-features --locked --offline`). Four new
output_card tests check canonical stable UID/order, missing/duplicate descriptors,
volume updates and alias rejection in verified descriptors. Existing95 tests reran
because a new module was added. Caller must supply independently verified descriptors;
Snapshot/catalog alone do not establish topology/adoption or observed volume.
No production observer, device UI fix or native grouping is claimed.
Log: coordinator outputs/CORE-OUTPUT-CARD-CHECK-20261113.log.


## Offline state import and external clock qualification — 2026-11-13

- Source2a2c94f:103 audio-only application library tests passed in Pi/Linux offline
  container. Four new import fixtures cover exact ID/seed/public-key preservation,
  restart, permissions, existing/dangling destinations and ambiguous/unsafe snapshots.
  Direct Linux SYS_renameat2 preserves no-replace publication under musl; no live state.
  Log: coordinator outputs/CORE-STATE-IMPORT-CHECK-20261113.log.
- Sourcef6600a0:68 ap_clock library tests passed offline, including8 external-clock
  fixtures for measured rate, signed modular offsets, stale/reversed samples, changed
  epoch/daemon/role, uncertainty and RAW↔Instant correlation discontinuity.
  Log: coordinator outputs/EXTERNAL-CLOCK-CHECK-20261113.log.
  Independent source review confirmed the correlation-step regression is closed.

External qualification consumes supplied decoded observations, trusted selection/role
evidence and MEASURED correlations. It does not sample actual clocks, read live SHM,
attest the daemon, bind319/320, implement complete servo or plug into MasterTimeline.
Observed readiness/software arithmetic is not measured synchronization at the DAC.


## NQPTP ordered reader / measured correlation — 2026-11-13

Source e3be6e4:69 ap_clock and34 timesync library tests passed in Pi/Linux
offline container (`cargo test -p ap_clock -p timesync --lib --locked --offline`).
New checks: patched-master start-marker refresh without epoch reset; ABI10/11 file
fixtures, torn/unpublished/unsupported data, truncation/replacement/symlink/owner
checks; synthetic positive/negative RAW rate, extrapolation uncertainty and invalid
timespec/bracket bounds. Log: coordinator
outputs/NQPTP-READER-CORRELATION-CHECK-20261113.log.

Reader uses explicit read-only file operations, main before secondary and full
ordering fences; no mmap, default SHM path, daemon commands or PTP sockets. Header
reference: [pinned NQPTP ABI11](https://github.com/mikebrady/nqptp/blob/ab8fb67483411640fd7f1e193889482f2421c37c/nqptp-shm-structures.h).
Provenance: declared GPL-2.0, layout/ordering facts only, new code written independently.
Actual clock_gettime sampler is compiled; estimation tests use synthetic brackets,
not hardware accuracy measurements. No installed Station binary/ABI, live SHM,
role attestation, acoustic sync or full frontend transition was checked.


## External timeline/lease integration — 2026-11-13

Source1d112ca:39 timesync and104 audio-only application library tests passed
on Pi/Linux in offline container. Separate commands run timesync first, then
`cargo test -p airplay --lib --no-default-features --locked --offline`.
Cached external timeline drives a real PtpProducer packet with expected PTP value
and unchanged PCM/Instant. Tests cover unpublished construction, owner loss,
permanent invalidation, group cancellation preserving shared owner and silent
expiry with loss-watch read guard released before revocation (deadlock regression).
Log: coordinator outputs/EXTERNAL-TIMELINE-INTEGRATION-CHECK-20261113.log.

An earlier full application attempt failed the existing NTP local/remote loopback
with zero exact captured pairs; the unchanged assertion passed in the repeat run.
Root cause is not established; do not interpret the repeat as a diagnosed/fixed
NTP timing issue. No device audio/SHM or native group adoption was tested.

Architecture inventory is static source/manifests/call-site analysis, not runtime
service enumeration on the installed Station and not proof every source line is correct.


## CLI daemon default — 2026-11-13

Source a8b32e9:2 audio-only binary tests passed on Pi/Linux offline
(cargo test -p airplay --bin airplay --no-default-features --locked --offline).
Checks: empty args/daemon flags route to daemon parsing; help/version/management
commands do not; removed serve is rejected. Existing audio-only option rejection
also passes. No daemon, socket or audio output was started by tests. Default-feature
CLI build and full GitHub CI were not rerun. Examples and CI interop launch updated.
Log: coordinator outputs/CLI-DEFAULT-DAEMON-CHECK-20261113.log.

## External acquisition — 2026-11-13

Pi isolated Docker, offline, no device/socket/audio startup: 41 timesync library
 tests passed including daemon-instance/epoch/role mismatch and permanent loss
on attestation failure. Initial implementation plus existing pipeline passed all
104 audio-only application tests; final change only adds a timing test (41 vs40).
Logs: outputs/EXTERNAL-ACQUISITION-CHECK-20261113.log and
outputs/EXTERNAL-ACQUISITION-FINAL-CHECK-20261113.log in the local workspace.
These are fixture/regression tests, not live NQPTP daemon attestation or physical
synchronization measurements. No Station adapter pin/backend changed.
