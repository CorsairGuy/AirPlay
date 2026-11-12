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
