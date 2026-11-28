# Результаты D01–D30 (28 ноября 2026)

Сводка по второй половине серии: что сделано кодом, что проверено
референсами, что осталось. Уровни: **выполнено**, **частично**,
**заблокировано**. Подробности R-аудита — в
[REFERENCE-AUDIT-RESULTS](REFERENCE-AUDIT-RESULTS-20261128.md).

| № | Уровень | Результат |
|---|---|---|
| D01 | частично | таблица SETUP и синтетические fixtures; строгий отказ shk != 32; причина iPhone не доказана |
| D02 | выполнено | атомарная проверка всего списка streams, откат задач/владельца, тест обработчика |
| D03 | выполнено: точная сверка с shairport 5.5.2 player.c::do_flush; flush_sequence.rs покрывает границу/поздний пакет/rollover/повторный поток | FLUSH-граница и поздние пакеты покрыты `source_change_flushes_*`, `stale_retransmit_*`, `classic_flush_*`; построчная сверка с shairport 5.5.2 не зафиксирована |
| D04 | выполнено: диагностика timing без ключей, отказ на отсутствующий/0/>u16 порт, закрытие сокета; PTP-движок не менялся | NTP без порта не запускает обмен, TEARDOWN закрывает сокет; PTP-движок не менялся |
| D05 | заблокировано | OwnTone auth-setup/et=4 не подтверждён; реализацию не включали |
| D06 | выполнено | IPv4/IPv6 o= и c=, открытый/шифрованный, parser; loopback Transport/ANNOUNCE |
| D07 | выполнено | шифрованный ANNOUNCE 501 и очистка формата; защищённый RX не заявляется | |
| D08 | выполнено | `reject_policy_is_atomic_for_simultaneous_senders`, `parallel_teardowns_never_free_the_new_owner`, sessions reject-policy |
| D09 | выполнено | повтор/недесятичный Content-Length отвергаются без сдвига буфера; склейка/разрыв покрыты |
| D10 | выполнено | обрыв после OPTIONS/pairing/SETUP; UDP-порты освобождаются, следующий клиент обслуживается |
| D11 | выполнено | AppleTV14,1 reference fixture без ID/ключей, тест «не Profile» |
| D12 | выполнено | HomePodMini: model/версия/маска/status из снимка Станции; совместимость runtime не проверялась |
| D13 | выполнено | OtherTV-биты совпадают в `/info`, `_airplay` и `_raop` TXT |
| D14 | выполнено для gate: SCREEN/VIDEO отказ; PTP-idle и PHOTO — отдельные задачи, OtherTV не изменён | gate SCREEN/VIDEO; PTP-idle и PHOTO не гейтятся |
| D15 | проверено | `profile_isolation.rs`, `multi_profile.rs`: свои ID/ключи, restart; pgid на запуск |
| D16 | частично: withdrawn по типам проверен; TTL-истечение — поведение mdns-sd, локального кэша TTL нет | `discovery/tests/interface_flap.rs`, snapshot-тесты; `_raop` expiry отдельно не собрана |
| D17 | частично | C75: mdns-sd не даёт `ScopedIpV6`; parse-only не выдаётся за подключение |
| D18 | выполнено | DeviceInfo первым, SetState после подписки, идентификаторы клиентов раздельны |
| D19 | заблокировано | тип 37 не реализован (C31/C26); поля очереди не проверить |
| D20 | выполнено как research | `docs/MRP-QUEUE-COMMANDS.md`; `NotSupported` сохранён |
| D21 | частично | обложка заменяется/очищается, соседние треки делят; поколения источника нет |
| D22 | выполнено: TrackLoader с ограничением фоновых запросов и revision guard; app-адаптер конкретного провайдера не подключён | `track_queue_async.rs`: поздний результат, abort, revision; лимита фоновых запросов нет |
| D23 | заблокировано | публичный pause/seek runtime не согласован; контракты в docs/STATION-PIPELINE-CONTRACTS |
| D24 | выполнена: два mock-получателя, уход одного безопасен, последний освобождает источник; EOF/отмена покрыты | `pcm_source_lifecycle.rs`, `continuous_source_generation.rs`: EOF, отмена, нет потребителей |
| D25 | выполнено: subprocess abort до/после rename, перезапуск видит целый старый или новый файл; stale tmp не применяется | атомарная замена/fsync/stale tmp; настоящий subprocess crash не добавлен |
| D26 | частично | revoke в FileStore и PIN-политика покрыты; семантика уже начатого verify не выбрана |
| D27 | выполнено | перехват логов: PIN и байты ключа не логируются |
| D28 | заблокировано | нет рабочей FFmpeg-среды для воспроизводимых HLS-сегментов |
| D29 | выполнено: сервер видит EOF после abort загрузки, permit возвращается; production-код не менялся — прежний провал был гонкой теста | permit/лимиты/редиректы покрыты; немедленный EOF на сервере после abort не гарантирован |
| D30 | заблокировано | PTP-векторы и dual engine — зона координатора; NQPTP не внедряли |

## Код, добавленный в этой серии

- `services/media/src/rx.rs`: `validate_streams`, откат задач и владельца,
  NTP без фиктивного порта; тесты обработчика.
- `crates/rtsp/src/lib.rs`: отказ повторного/недесятичного Content-Length.
- `services/sessions/src/session.rs`: тест OtherTV-битов и журнала без секретов.
- `runtime/src/lifecycle/receiver.rs`: gate SCREEN/VIDEO без mirror backend.
- Новые тесты: `runtime/tests/rtsp_cleanup.rs`, `runtime/tests/feature_gate.rs`,
  `services/media/tests/audio_setup_fixture.rs`,
  `crates/device/tests/reference_profiles.rs`.

## Ограничения серии

- iPhone/Apple TV/HomePod, звук, синхронизация, 60 FPS не проверялись.
- pyatv-клиент, CI job, полный workspace-прогон не выполнялись.
- PTP/timesync, `playout.rs`, relay и профили координатора не менялись.
