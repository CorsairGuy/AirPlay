# Архитектура ядра и перенос Станции

Актуализировано12ноября2026. [Статус и проверки](STATUS.md).

## Принятое устройство проекта

Собственное единое ядро на Rust; перенос проверенных идей, структуры и wire-фактов в общие слои; новый собственный код пишется с нуля (политика §0.5 v12). Временный внешний backend допустим, но не становится второй независимой реализацией тех же часов/криптографии. Цель — универсальное ядро с явными профилями, а не программа для Mini2. GPL-3.0-or-later и provenance заимствований сохраняются.

| Слой | Ответственность |
|---|---|
| crates/pair, crypto, rtsp, rtp, mrp | Общие кодеки и протоколы; независимое состояние каждой сессии |
| crates/clock, services/timesync | Общий источник времени, role/mapping; явно созданная Service, общие сокеты и union адресатов |
| crates/sender | Экспериментальный AP2 realtime sender, PtpPlan, SessionPool, LocalSink; без Quasar/ALSA paths |
| bin/airplay | Receiver/frontend, MRP card/backend, discovery feed, pairing store, output worker, PtpGroup |
| Репозиторий Станции | Glagol/Quasar, native PCM tap, BT/focus, ALSA маршруты, LED, supervisor, install и rollback |

## Исходящий групповой путь

Source adapter → один paced PCM producer/MediaClock → immutable AudioPacket → SessionPool + optional local renderer. Входящие и исходящие ключи, nonce, RTP counters и session IDs независимы; общего clock/media epoch недостаточно для объявления native group adoption.

PtpGroup получает MasterTimeline существующей общей PTP-службы либо явно опубликованный qualified external timeline через new_external. with_clock_service проверяет идентичность watch-канала, принимает отдельный fresh owner token и явные timing ports. Выбор устройств задаёт полный список физически выбранных адресатов новым connection plans. Работающие сессии обновляют SETPEERS без перезапуска. Реальный running состав обновляет интерес группы в общей службе; освобождение одного интереса сохраняет адресаты других потребителей.

Потеря корректных часов/эпохи или сбой local renderer останавливает передачу группы. Общая PTP-служба продолжает жить. При нормальном EOF применяется drain; при сбое — stop и bounded cleanup. Hard abort не заменяет signal stop + await. Адаптер восстанавливает прямой локальный маршрут существующими механизмами.

## Профили и границы

- AP1 classic, AP2 realtime/buffered, control-only MRP и mirroring не смешиваются неявно.
- AP2 timing выбирается явно: ядро имеет NTP и PTP; групповой профиль использует PTP. Обязательность NTP для всех AP2-устройств по закрытой спецификации Apple не заявляется. In-session fallback отсутствует.
- Общая служба может жить без групп. MasterTimeline TX ограничен qualified local master; cached external timeline отдельно допускает attested sender-master observations. Live owner/attestation/peer-control ещё не подключены; полноценный follower TX не объявлен готовым.
- Audio-only профиль перенесён выборочно из старого черновика: cargo -p airplay --no-default-features исключает видео-зависимости, сохраняя NTP realtime audio. Default включает mirroring. См. BUILD_PROFILES.md.
- Ни Session Running, ни SETUP/SETPEERS ACK, ни MRP-галочка не доказывают звук, синхронизацию или принятие топологии.

## Перенос на Станцию

Сначала закончить continuous/PIN/runtime tests и разделение профилей. Затем pin версии ядра и адаптер native/AirPlay/BT PCM, отдельное локальное приглушение/голос, сохранение guards/autostart и recovery. Не возвращать второе скачивание/декодирование Alice CDN и не переносить аппаратные пути в протокол.

После автономных проверок — только отдельно разрешённая установка с откатом. Последние overlays/tap и smi10/smi11 compatibility проверяются по выбранному backend; собственная PTP-служба не требует NQPTP ABI. Звук/Home app/карточка/групповая подпись проверяются раздельно.

Подробный остаток — [STATUS](STATUS.md); историческая последовательность — [журнал](STATION_MIGRATION_20261111.md). Старый архитектурный аудит сохранён в docs/history, его TODO не является текущим планом.


## Граница v12: единое владение PTP и provenance

На хосте один владелец UDP319/320 и один физический источник времени; RX/TX получают
явные роли/mapping/peer interests через него. Второй NQPTP/PTP listener одновременно
не запускается. До активации core-host требуется проверенный адаптер совместимого SHM
либо согласованная передача владения с откатом. Это открытый интеграционный блокер,
не реализованная автоматическая замена NQPTP. Нельзя relabel RAW timestamps как Instant.
По пользовательской политике §0.5 v12 новые реализации пишутся с нуля: идеи/структура/
wire-факты с provenance-тегом; внешний код и близкий перевод не копируются.
Quad/hub из L-74 — кандидат для проверки, не установленный Apple-прецедент (§9 плана).


## External clock transition contract

`clock::external::Qualifier` pins a daemon instance, clock domain/epoch and external
sender-master evidence. It uses supplied measured RAW/Instant rate and uncertainty,
checks bounded freshness and continuity of successive correlations, and revokes on
any failed observation. It cannot auto-rearm; the owner establishes a new selection.
The offset-step limit is a conservative caller uncertainty policy, not a servo proof.
Still required: ordered SHM reader, actual cross-timestamp sampler/role attestation,
qualified external timeline/lease integration and owner-safe legacy peer control.
`MasterTimeline` remains the explicit local-master API; no synthetic Service status
or hidden second PTP owner was introduced.


NQPTP acquisition now has explicit Linux `nqptp_reader::Reader` and
`raw_correlation::capture/estimate` APIs. Reader identity checks do not establish
a daemon instance, patch support or role. Same-inode restart still needs trusted
external instance/selection evidence. Captures are scheduled by the owner, with
explicit uncertainty/wander policy; no zero-rate correlation from one sample.

`Selection::start_marker` distinguishes stable epoch markers from the Station
patched-master refresh marker, which is rewritten on keepalive. The owner-attested
epoch lower bound remains separate and stable. Do not infer epoch from marker
refresh or infer patch support from ABI. Remaining integration: attestation and
scheduling owner, external timeline/lease, peer-control ownership and frontend.


## External timeline → producer/lease

`external_media::Timeline` держит только cached observations и Qualifier; producer
не читает SHM и не ждёт clock lock. Единственный Publisher обновляет наблюдения;
его Drop/явный revoke или ошибка qualification навсегда отзывает эту selection.
`PtpGroup::new_external` принимает только уже опубликованный timeline; lease следит
за publisher loss, upstream stop и тихим истечением свежести. Отмена одной группы
не отзывает общий publisher и не выключает NQPTP. External group нельзя присоединить
к отдельной Service через with_clock_service. Владение legacy peers остаётся явной
обязанностью owner/preparer; daemon commands не добавлены и готовый host не заявлен.

Runtime-разбиение/названия и устранение существующих повторных MRP-реализаций —
[COMPONENTS-AND-NAMING](COMPONENTS-AND-NAMING.md). Это заменяет старое представление
«три каталога services = три службы»; protocol module не становится daemon автоматически.
