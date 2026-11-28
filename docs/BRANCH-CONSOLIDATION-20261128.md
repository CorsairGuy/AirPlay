# Консолидация веток в agent/integration, 2026-11-28

База: `agent/integration` на `af45129` (28 коммитов поверх `main` `fe41f82`).
Проверены все локальные ветки, кроме `main` и самой `agent/integration`.

Во время сверки удалённый `main` сдвинулся на `ec1a17a` («Provide four named external device profiles»); этот коммит уже был предком `agent/integration`, поэтому сравнение с `fe41f82` остаётся корректным и ничего не пересчитывалось.

## Метод

- `git merge-base --is-ancestor` — что уже влито.
- `git cherry main <branch>` — есть ли эквивалентный патч в `main` (все 104 оказались уникальными по patch-id, поэтому одного этого признака мало).
- `git merge-tree --write-tree` — конфликты с текущим деревом.
- Сверка имён функций и модулей: добавленные веткой функции ищутся в дереве `agent/integration`; отдельно сверены файлы, куда ветка вносит production-код.
- Ни одна ветка не проверялась на устройстве: аппаратных результатов в этой серии нет.

## Итог

- Всего веток: **135**.
- Уже влиты в `agent/integration`: **31**.
- Не влиты: **104**, из них **9** сознательно не сливаются (таблица 3), остальные **95** перекрыты текущим деревом и консолидированы слиянием `-s ours` без изменения файлов.
- Дубли определялись по совпадению добавленных функций и по прямому сравнению файлов: у большинства веток все добавленные функции уже есть в дереве, часто под другими именами и в другом модуле.

## 1. Уже влитые ветки

| Ветка | Tip | Тема |
| --- | --- | --- |
| feat/airplay1-tx | `ec50d3f` | Sender: add AirPlay 1 RAOP audio profile |
| feat/audio-codec-matrix | `90aca50` | Centralize the audio codec matrix and validate stream fields |
| feat/buffered-audio-tx | `6e322b7` | Add buffered audio sender transport with ADTS parsing |
| feat/buffered-cli | `6a22149` | Add a send-buffered CLI for ADTS AAC files |
| feat/buffered-link | `f0c8779` | Add a buffered stream reply parser and data-port link |
| feat/buffered-session | `fc8b6fd` | Add a buffered audio session RTSP sequence |
| feat/companion-remote | `ba9b91c` | Add Companion Link codec, framing and pair-verify |
| feat/companion-touch | `81d641f` | Add Companion touchpad events and tap helper |
| feat/device-profiles | `1c5076f` | Load device identity from an external plist profile |
| feat/discovery-profiles | `9f0edcd` | Discovery: merge AirPlay/RAOP records and select a proven sender profile |
| feat/extended-display | `5bc8ff9` | Advertise a full display descriptor for mirroring |
| feat/hls-vod | `342be92` | Add HLS VOD audio, pause/resume and segment seek |
| feat/live-group | `4f7c84f` | Live group: cover failed-member rejoin without restarting survivors |
| feat/mirror-fanout | `33aa74a` | Add mirroring fan-out to several video sinks |
| feat/mirror-tx | `60aafd5` | Add AirPlay mirroring sender session |
| feat/mrp-card-phase6 | `f999a29` | MRP card: replay current track to reconnecting controllers |
| feat/multi-profile | `0f57e87` | Add multi-instance receiver lifecycle |
| feat/multichannel-audio | `168afe1` | Downmix 5.1 and 7.1 to stereo with defined levels |
| feat/photo-phase8 | `13683e2` | Photo: add bounded AirPlay photo receive profile |
| feat/raop-rx | `9979974` | Add classic AirPlay 1 RAOP receive path |
| feat/reconnect-s3-s4-s6 | `588032a` | Reconnect: clear stale sender session on abrupt disconnect |
| feat/remote-control | `c17e0ec` | Remote: deduplicate retransmitted MRP commands |
| feat/rtp-loss-s13 | `751f56e` | RTP receive: drop duplicates and late packets |
| feat/session-observability | `847fe38` | Integrate receiver services, HLS and audio fixes |
| feat/session-ownership | `3ad27e4` | Add a reject policy for a second audio sender |
| feat/tx-latency | `bcdca95` | Sender: negotiate receiver latency with a bounded policy |
| feat/tx-metadata | `889e661` | Sender: document metadata protocol coverage |
| fix/audio-receive | `e002e11` | Fix audio receive truncation, stalled reconnects and unbounded queues |
| fix/robust-input | `203fed9` | Reject hostile plist, stream parameters and RTSP framing |
| refactor/receiver-extraction | `e644443` | Extract receiver components from bin/airplay into service crates |
| research/fairplay-v2 | `9ed85c7` | FairPlay: reject v2 explicitly and document what v2 needs |

## 2. Перекрытые текущим деревом (консолидированы без изменения файлов)

Для каждой ветки ниже её добавленные функции уже присутствуют в `agent/integration`; расхождения — форматирование, переименования и более поздние правки. Слияние `-s ours` только фиксирует факт интеграции и не меняет файлы.

| Ветка | Tip | Основной файл ветки | Тема |
| --- | --- | --- | --- |
| docs/audit-fixes-20261125 | `c96a520` | `README.md` | Docs: fix stale README test command, index gaps and package registry |
| feat/airplay1-auth | `42a898d` | `bin/airplay/tests/multi_profile.rs` | Stop implying an Apple-Response check a RAOP sender never performs |
| feat/airplay1-vectors | `eb415c1` | `docs/AIRPLAY1-SDP-VECTORS-20261126.md` | Pin the AirPlay 1 SDP form with hand-written sender and receiver vectors |
| feat/c01-cbc-openssl-vector | `4d85a17` | `crates/crypto/src/cbc.rs` | Add independent OpenSSL CBC vector for RAOP audio |
| feat/c02-ntp-boundaries | `3e40744` | `services/timesync/src/ntp.rs` | Test NTP reply filtering and sample expiry |
| feat/c03-sdp-matrix | `e5fb9a2` | `services/media/src/raop.rs` | Test RAOP SDP field order and format bounds |
| feat/c04-realtime-source | `1024cb0` | `crates/audio_stream/src/net.rs` | Test realtime audio source filtering |
| feat/c05-mirror-event-lifecycle | `86f0498` | `services/media/src/tx/mirror.rs` | Test mirror event channel lifecycle edges |
| feat/c06-mirror-framing | `2e392b3` | `crates/mirror/src/receiver.rs` | Test mirror receiver framing limits and truncation |
| feat/c07-ffmpeg-blocker | `af9fb70` | `docs/AGENT-TASKS.md` | Record FFmpeg 8 header blocker for C07 |
| feat/c08-mrp-wire-limits | `525ed14` | `crates/mrp/src/wire.rs` | Test MRP wire limits and preserve unknown fields |
| feat/c09-card-audit | `2a25435` | `docs/AGENT-TASKS.md` | Record MRP card coverage audit for C09 |
| feat/c10-opack-vectors | `79168ad` | `crates/companion/src/opack.rs` | Add independent OPACK wire vectors and negative tests |
| feat/c11-session-refusals | `cfef98b` | `crates/companion/tests/session.rs` | Test Companion session protocol refusals |
| feat/c12-discovery-endpoints | `2bc2b50` | `services/discovery/src/catalog.rs` | Test discovery endpoint identity and address retention |
| feat/c13-refusal-table | `786c074` | `services/discovery/src/capabilities.rs` | Add discovery profile refusal table test |
| feat/c14-discontinuity-blocker | `2f7515b` | `docs/AGENT-TASKS.md` | Record decoder dependency for C14 discontinuity |
| feat/c15-hls-fetch-limits | `49398a5` | `crates/hls/src/fetch.rs` | Test HLS fetch body limits and redirect cap |
| feat/c16-buffered-nonce | `8f0c328` | `services/media/src/tx/buffered.rs` | Test buffered audio nonce progression |
| feat/c17-tlv8-boundaries | `253dc36` | `crates/pair/src/tlv8.rs` | Test TLV8 fragment boundaries and repeated fields |
| feat/c18-identity-audit | `d94f7b5` | `docs/AGENT-TASKS.md` | Record identity import coverage audit for C18 |
| feat/c19-packetizer-audit | `bb4d29b` | `docs/AGENT-TASKS.md` | Record packetizer coverage audit for C19 |
| feat/c20-pool-audit | `65a509e` | `docs/AGENT-TASKS.md` | Record pool coverage audit for C20 |
| feat/c21-clock-wire | `4d9e82c` | `crates/clock/src/ptp.rs` | Test PTP timestamps and reject overflowing values |
| feat/c22-photo-bounds | `28eead6` | `services/sessions/src/photo.rs` | Test photo asset and dimension limits |
| feat/c23-long-run-limits | `13dce12` | `crates/audio_stream/tests/long_play.rs` | Bound long playback stand and test duplicate packet drops |
| feat/c24-parser-smoke | `214234e` | `crates/rtsp/src/plist.rs` | Smoke-test malformed plist parsing limits |
| feat/c25-track-queue | `7b6ad61` | `services/sessions/src/lib.rs` | Add provider-neutral track queue model |
| feat/c27-interop-harness | `f473384` | `.github/workflows/ci.yml` | Add a runtime API interop harness and restore the pyatv job |
| feat/c29-card-output-empty | `9e554e5` | `services/remote/src/card.rs` | Check the card withdraws rows and stays empty after outputs drop |
| feat/c30-queue-playback-boundary | `9cfa3bd` | `services/media/src/track_queue.rs` | Add playback position and cancellable queue loads |
| feat/c32-clock-domain-vectors | `d668bdd` | `crates/clock/tests/domain_matrix.rs` | Add independent PTP domain translation vectors |
| feat/c34-discovery-interface-flap | `2faee51` | `services/discovery/tests/interface_flap.rs` | Test discovery catalog across interface flaps |
| feat/c35-hls-url-policy | `0c6ed9a` | `crates/hls/tests/url_policy.rs` | Test HLS URL and redirect policy |
| feat/c36-runtime-library-api | `4206f09` | `runtime/tests/library_api.rs` | Add runtime library API integration tests |
| feat/c37-station-adapter | `e30f9ce` | `docs/STATION-CORE-ADAPTOR-20261127.md` | Clarify the C37 status wording |
| feat/c38-pair-verify-replay | `a7101d7` | `crates/pair/tests/verify_replay.rs` | Cover pair-verify replay and identity substitution |
| feat/c39-control-channel-limits | `1954ac0` | `crates/crypto/tests/control_channel.rs` | Test control channel framing and nonce boundaries |
| feat/c40-ownership-teardown | `f40e563` | `services/media/src/rx.rs` | Test receiver ownership rejection and parallel teardown |
| feat/c41-mirror-fanout-config | `b9cb337` | `crates/mirror/tests/fanout_config.rs` | Cover mirror fanout config changes and backpressure |
| feat/c42-rtsp-framing-bounds | `a9ce5b4` | `crates/rtsp/tests/framing_bounds.rs` | Test RTSP sequence boundaries and inter-frame data |
| feat/c43-pair-revoke | `7b003fc` | `crates/pair/src/store.rs` | Add revocable pairing store with fail-closed default |
| feat/c43-pair-revoke-file | `7a73bf7` | `crates/pair/src/lib.rs` | Add atomic pairing revocation for memory and file stores |
| feat/c44-session-partial-setup | `bd15138` | `services/media/src/tx/session.rs` | Test cleanup after partial SETUP and fresh keys on retry |
| feat/c45-profile-txt-consistency | `2a4dea2` | `crates/device/tests/profile_info_txt.rs` | Verify profile consistency in TXT and /info |
| feat/c46-mdns-service-lifecycle | `7fc1c66` | `crates/mdns/tests/service_table.rs` | Test mDNS service table lifecycle without Bonjour |
| feat/c47-video-pacer-timeline | `7f97873` | `crates/video/tests/pacer_timeline.rs` | Test pacer pause, format change and late frames |
| feat/c48-audio-queue-bounds | `620217f` | `crates/audio_stream/tests/queue_bounds.rs` | Test audio playout queue bounds and flush |
| feat/c49-pcm-source-lifecycle | `df1c7c4` | `services/media/tests/pcm_source_lifecycle.rs` | Test PCM source EOF, cancellation and closed consumers |
| feat/c50-buffered-session-failures | `ef340e1` | `services/media/src/rx.rs` | Test buffered session cleanup when RECORD and FLUSH fail |
| feat/c51-continuous-source-generation | `bcad87e` | `services/media/tests/continuous_source_generation.rs` | Test continuous source generation boundaries |
| feat/c52-mrp-correlation | `a2554d7` | `services/remote/src/mrp_service/server.rs` | Test MRP request correlation across clients |
| feat/c53-runtime-profile-isolation | `88cfe10` | `runtime/tests/profile_isolation.rs` | Test runtime profile and key isolation |
| feat/c54-ntp-fallback | `d64ee56` | `services/timesync/src/ntp.rs` | Test NTP sample freshness and fallback mapping |
| feat/c55-source-process-limits | `f962db2` | `services/media/tests/source_process_limits.rs` | Test external PCM process limits and cleanup |
| feat/c56-capability-bits-audit | `8893890` | `crates/device/tests/capability_bits.rs` | Document and test capability bit consistency |
| feat/c57-runtime-nested-cancel | `b4ab56b` | `runtime/tests/library_api.rs` | Test bounded runtime stop during unfinished pairing |
| feat/c58-channel-write-exhaustion | `b4f9cfb` | `crates/crypto/src/channel.rs` | Repair the C58 status text |
| feat/c59-channel-post-error | `4c7bcbb` | `crates/crypto/tests/channel_stream.rs` | Test control channel behavior after a stream error |
| feat/c60-pairing-crash-consistency | `091b486` | `crates/pair/src/lib.rs` | Test pairing crash consistency and temporary residue |
| feat/c61-pairing-revoke-race | `e0577b3` | `crates/pair/src/lib.rs` | Document pairing revoke policy and test verify race |
| feat/c62-queue-item-identity | `5ef9851` | `services/media/src/track_queue.rs` | Give each queue occurrence its own identity |
| feat/c63-queue-async-cancel | `4b2c867` | `services/media/src/track_queue.rs` | Test async queue load cancellation |
| feat/c64-mrp-queue-range | `968676a` | `crates/mrp/src/messages.rs` | Honor partial playback queue ranges and bound artwork |
| feat/c65-mrp-queue-commands | `9c6ca81` | `docs/MRP-QUEUE-COMMANDS.md` | Document MRP queue commands and refuse them explicitly |
| feat/c67-retransmit-after-flush | `3700273` | `services/media/src/tx/session.rs` | Test stale retransmits after a flush and nonce advance |
| feat/c68-raop-rx-crypto | `bae971c` | `docs/RAOP-RX-CRYPTO.md` | Document what RAOP RX knows about encrypted audio |
| feat/c69-session-retry-all-stages | `6847973` | `services/media/src/tx/session.rs` | Test retry after SETUP failure and a successful fresh session |
| feat/c70-buffered-partial-writes | `6a463f6` | `services/media/tests/buffered_link_partial.rs` | Test buffered audio partial writes and stream closure |
| feat/c71-clock-epoch-vectors | `7e0ea29` | `crates/clock/tests/domain_epoch.rs` | Test clock domain translation across epochs |
| feat/c72-ptp-correction | `fd14b55` | `crates/clock/tests/ptp_correction.rs` | Test signed PTP correction and malformed packets |
| feat/c74-mdns-stale-txt | `99ac885` | `crates/mdns/tests/txt_refresh.rs` | Test stale TXT and removal in the mDNS table |
| feat/c75-ipv6-scope-endpoints | `c2116d7` | `services/discovery/src/capabilities.rs` | Test IPv6 endpoint families and scope handling in discovery |
| feat/c76-hls-fetch-bounds | `1269121` | `crates/hls/src/fetch.rs` | Test HLS fetch memory, truncation and cancellation |
| feat/c77-hls-relative-cycles | `06dac36` | `crates/hls/src/fetch.rs` | Test relative HLS segments and bounded master cycles |
| feat/c79-photo-cache-bounds | `a1fb79b` | `services/sessions/tests/photo_cache_bounds.rs` | Test photo cache bounds and stale display |
| feat/c80-mirror-session-restart | `04ad49a` | `crates/mirror/tests/receiver_session_restart.rs` | Test mirror receiver restart after config-only session |
| feat/c81-mirror-cancel-retry | `2f72a72` | `services/media/tests/mirror_tx.rs` | Test mirror retry after cancellation at pairing and audio SETUP |
| feat/c82-discovery-duplicate-names | `18c43f3` | `services/discovery/tests/duplicate_names.rs` | Test discovery identity independent of display names |
| feat/c83-session-error-privacy | `1811553` | `services/sessions/src/session.rs` | Keep the pairing PIN out of session logs |
| feat/c84-artwork-bounds | `b47b2ee` | `services/remote/src/card.rs` | Test artwork dimension and MIME bounds in the card |
| feat/c85-runtime-shared-clock | `6430b05` | `runtime/tests/library_api.rs` | Test shared and separate clocks across runtime instances |
| feat/c86-runtime-pipeline-contracts | `cf867e2` | `docs/STATION-CORE-ADAPTOR-20261127.md` | Compare runtime pipeline contracts with the continuous source |
| feat/c87-ci-library-api | `4242303` | `.github/workflows/ci.yml` | Extend the library API CI job to the runtime feature matrix |
| feat/companion-session | `2e31097` | `Cargo.lock` | Add a Companion session with correlated replies and explicit failures |
| feat/long-play-stand | `712b0d3` | `crates/audio_stream/src/playout.rs` | Add an accelerated stand for long audio sessions |
| feat/mirror-partial-setup | `a2935af` | `docs/MIRROR-PARTIAL-SETUP-20261126.md` | Test that a rejected video SETUP tears down the partial mirroring session |
| feat/mrp-wire-vectors | `59f00a6` | `crates/mrp/tests/wire_vectors.rs` | Pin the MRP wire format with hand-written byte vectors |
| feat/p2-profiles-audit | `b5b75ba` | `crates/hls/src/lib.rs` | Vector the cast URL safety rules and per-family discovery endpoints |
| feat/pairing-import | `2366793` | `docs/PAIRING-IMPORT-20261126.md` | Keep auto-pair attempts one-shot and refuse non-atomic imports off Linux |
| feat/peer-owner-leases | `227ee03` | `docs/PEER-OWNER-LEASES-20261126.md` | Lease peer ownership so a silent consumer releases its peers |
| feat/ptp-wire-vectors | `c204808` | `crates/clock/tests/ptp_wire_vectors.rs` | Check the PTP profile against hand-written wire vectors |
| feat/seek-marker | `60a8701` | `crates/rtp/src/packetizer.rs` | Mark the first packet after a seek as a new talkspurt |
| feat/station-core-gaps | `ffc6d7c` | `docs/STATION-CORE-GAPS-20261126.md` | Inventory remaining Station core gaps and split them into tasks |
| feat/station-core-inventory | `ad1e773` | `docs/STATION-CORE-INVENTORY-20261126.md` | Inventory Station core capabilities against current main |

## 3. Сознательно не слиты

Эти ветки противоречат зафиксированным решениям аудита или затрагивают зону координатора. Их содержимое не переносится и в дереве не появляется.

| Ветка | Tip | Причина |
| --- | --- | --- |
| feat/c66-card-stale-artwork | `265d99e` | Использует хеш байтов как принадлежность обложки; принят Card::artwork_for по content id (D21/R28), хеш принадлежность не доказывает. |
| feat/clock-attestation | `e967850` | Принимает ничем не подтверждённый Claim от вызывающего и сам не устанавливает подлинность часов. |
| feat/dual-clock-roles | `0dafc60` | Зона координатора: одновременные входящий follower и исходящий master требуют сквозного relay-теста. |
| feat/epoch-recovery | `bfeb487` | Сверка закрыта: main меняет эпоху только по явным сигналам; эвристика серии пакетов осталась только в отклонённой ветке. |
| feat/hls-segments | `6a6dfce` | Обработка discontinuity намеренно отклоняется до реализации PTS/декодера; ветка реализует запрещённый сейчас путь. |
| feat/long-session-recovery | `cdc98b2` | Смена эпохи по одиночному запоздавшему пакету небезопасна; в main принят вариант без эвристики. |
| feat/nqptp-shm-reader | `bec5de1` | Возвращает NQPTP-специфичный код, который запрещён в ядре; внешний движок времени не встраиваем. |
| feat/raop-timing | `8fa3817` | Заменяет переданный хостом timing-сервис собственными часами; конфликтует с архитектурой D04/D14, нужна отдельная задача с координатором. |
| feat/relay-domain-shift | `5c2a019` | Дублирует clock::domain::translate как отдельный offset; сквозной relay-тест не проводился. |

## 4. Что это значит

- Ни одна из 95 консолидированных веток не добавляет кода, которого нет в дереве: проверка шла по именам функций и по прямому сравнению файлов, а не по одному флагу.
- Формулировка «ветка влита» здесь означает, что её работа уже присутствует в другой форме; это не подтверждение протокола и не аппаратная проверка.
- Отклонённые ветки оставлены в репозитории как есть: удалять чужие ветки без поручения нельзя.
