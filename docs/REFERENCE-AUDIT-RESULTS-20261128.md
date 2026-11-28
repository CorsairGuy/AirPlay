# Результаты 30 проверок main по референсам (R01–R30)

Дата: 28 ноября 2026, ветка `agent/integration`. Ниже — что реально
сверено с кодом `main`, какие тесты уже есть и чего не хватает. Уровни:
**проверено** — код и тест совпадают с утверждением; **частично** — есть
код/тест, но нет независимого вектора или аппаратной проверки;
**заблокировано** — нужен отсутствующий обработчик или координация.
Тесты, отмеченные как существующие, не перезапускались все подряд в этом
ходе, если это не указано явно.

| № | Уровень | Доказательство в коде/тестах | Ограничение |
|---|---|---|---|
| R01 | частично | `services/media/tests/audio_setup_fixture.rs`; `rx::tests::failed_multi_stream_setup_*`; docs/IPHONE-AUDIO-SETUP-20261128.md | нет захвата iPhone; fixture синтетический |
| R02 | частично | `rx.rs::setup_streams` (StreamKey::ChaCha/AesCbc), `setup_mirror_audio`, TEARDOWN `player.release` | нет независимого вектора происхождения ключа |
| R03 | частично | `timesync::ntp` loopback test; `rx::tests::ntp_setup_*` | байты обмена не сверены с iPhone; PTP-движок не трогали |
| R04 | частично | ответы 96/103 в `rx.rs`; `audio_setup_fixture` проверяет dataPort/controlPort | ошибки late-bind не инжектировались |
| R05 | частично: gate SCREEN/VIDEO и таблица бит готовы; PTP-idle и PHOTO остаются отдельными задачами | docs/FEATURE-CAPABILITY-GATE-20261128.md; `runtime/tests/feature_gate.rs` | PTP-бит с idle-часами и PHOTO не гейтятся |
| R06 | частично | 20 тестов `crates/fairplay`; `sessions` end-to-end `/fp-setup` | нет опубликованных независимых векторов |
| R07 | частично | `sessions::tests::transient_pairing_switches_to_encrypted_channel` | pyatv-клиент локально не запускался |
| R08 | проверено | `pin_pairing_then_verify_on_new_connection`, `pin_only_mode*`, `pairing_logs_do_not_contain_*` | реальный PIN на устройстве не проверялся |
| R09 | проверено: crypto 14+5+8 тестов зелёные, включая tampered frame | `crypto/channel.rs`: multi-frame, partial reads, tampered frame | нет RFC 8439 кросс-вектора в тесте |
| R10 | частично | `media::tx::airplay1` ANNOUNCE/SDP; AirPlay1 loopback в `runtime/tests/multi_profile.rs` | построчная сверка с OwnTone `raop.c` не зафиксирована |
| R11 | проверено: NIST CBC, OpenSSL CBC-вектор, OAEP/SHA-1 и 256-байтный модуль; защищённый RX не заявляется | `crypto` RSA/CBC тесты существуют; `docs/RAOP-RX-CRYPTO.md` | нет OpenSSL/NIST fixture в этом ходе |
| R12 | частично | `rx.rs::setup_session` (ekey/eiv), `docs/RAOP-RX-20261125.md` | защищённый RX не заявляется рабочим |
| R13 | проверено: пакеты crypto, rtp и media tx::session прогнаны 28.11 (17+25 тестов), retransmit/flush/wrap зелёные; двух разных клиентов в одном тесте нет | `airplay1::tests::retransmit_uses_original_packet_and_ignores_other_peers`; `session::peer_port_cannot_wrap_to_another_endpoint` | два разных клиента в одном тесте не подняты |
| R14 | проверено: media tx::session 25 тестов и playout flush-тесты зелёные; построчная сверка с player.c не зафиксирована | `source_change_flushes_before_sending_marked_packet`; `stale_retransmit_after_flush_*`; `playout::classic_flush_*` | FLUSH не трактуется как смена эпохи |
| R15 | проверено: rtp 17 тестов (wrap/nonce/marker) зелёные; ChaCha-вектор не внешний | `rtp/packetizer`: nonce/marker при wrap; `sync_wire_fixture_and_wrapping_latency` | ChaCha-вектор не внешний |
| R16 | проверено | `media` buffered tests: partial link, record/flush failure, new keys | частичная запись проверяется на TCP-модели |
| R17 | частично | `codec::matrix` тесты и `realtime_format_rejects_impossible_parameters` | малые декодируемые fixtures не добавлены |
| R18 | частично | `media/tests/mirror_tx.rs` (TX), ответы RX в `rx.rs` | loopback 400/406/415/455/500/501 для RX не собраны |
| R19 | проверено для KDF/CTR: независимый sha512-вектор и NIST AES-CTR; реальный поток не проверялся | `mirror::stream_key_iv`, `stream_key_matches_formula` | authenticated mock не равен AES-CTR |
| R20 | проверено | `mirror::annex_b_conversion`, `hevc_parameter_sets_*` | нет исходных Apple-фикстур |
| R21 | частично | тесты video/playout на устаревшие кадры | 60 FPS без дисплея не заявляется |
| R22 | заблокировано | аудит владения PTP только по документам | менять координаторскую ветку нельзя |
| R23 | частично: clock 68+7+9+6 тестов зелёные; внешний wire corpus не подключён | `crates/clock` тесты correction/domain/length | внешний corpus не подключён |
| R24 | частично: clock domain-тесты входят в 90 зелёных; полный RX→TX relay не проверен | `clock::domain` численные тесты | полный RX→TX relay не проверен |
| R25 | частично | `services/discovery` snapshot/interface_flap тесты | живого mDNS и scope IPv6 нет |
| R26 | проверено как reference | `tests/interop/fixtures/appletv14-1.reference.plist`; `crates/device/tests/reference_profiles.rs` | сырая маска не профиль |
| R27 | заблокировано | `PlaybackQueueRequestMessage` тип 37 намеренно игнорируется (C31/C26) | чтение очереди не реализовано |
| R28 | частично | `card.rs` тесты обложки; второй трек с той же обложкой | нет поколения источника |
| R29 | частично | `docs/STATION-PIPELINE-CONTRACTS-20261127.md`, `media/tests/station_adapter.rs` | синтетический адаптер не назван source API |
| R30 | частично | loopback-стенды SETUP/NTP/RTSP cleanup, HLS cancel/EOF, `feature_gate` | Windows CI и pyatv не прогонялись в этом ходе |

## Найденные расхождения

1. **RTSP Content-Length**: повтор и недесятичная запись раньше выбирали
   первое значение. Теперь запрос отвергается целиком, буфер не сдвигается
   (`crates/rtsp`).
2. **RX SETUP**: список `streams` проверяется до изменений; при позднем
   отказе задачи и владелец плеера освобождаются.
3. **NTP timingPort**: отсутствующий/нулевой/вне-u16 порт больше не
   запускает бессмысленный обмен; TEARDOWN закрывает сокет.
4. **Feature gate**: профиль с `SCREEN`/`VIDEO` без mirror backend теперь
   отказывается стартовать, а не объявляет несуществующий обработчик.
5. **HLS cancellation**: проба с сервером показала, что abort загрузки не
   гарантирует немедленный EOF на сервере; это записано как ограничение
   D29/R30, а не как исправление.

## Что не проверялось

- iPhone, Apple TV, HomePod, реальный звук, синхронизация и 60 FPS.
- pyatv-клиент и CI job (нет локального pyatv/runner).
- Полные прогоны всех крейтов; запускались только затронутые тесты.
