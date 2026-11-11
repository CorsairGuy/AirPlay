# Архитектура ядра и перенос Станции

Актуализировано12ноября2026. [Статус и проверки](STATUS.md).

## Принятое устройство проекта

Собственное единое ядро на Rust; перенос проверенных решений и совместимого по лицензии кода в общие слои. Временный внешний backend допустим, но не становится второй независимой реализацией тех же часов/криптографии. Цель — универсальное ядро с явными профилями, а не программа для Mini2. GPL-3.0-or-later и provenance заимствований сохраняются.

| Слой | Ответственность |
|---|---|
| crates/pair, crypto, rtsp, rtp, mrp | Общие кодеки и протоколы; независимое состояние каждой сессии |
| crates/clock, services/timesync | Общий источник времени, role/mapping; явно созданная Service, общие сокеты и union адресатов |
| crates/sender | Экспериментальный AP2 realtime sender, PtpPlan, SessionPool, LocalSink; без Quasar/ALSA paths |
| bin/airplay | Receiver/frontend, MRP card/backend, discovery feed, pairing store, output worker, PtpGroup |
| Репозиторий Станции | Glagol/Quasar, native PCM tap, BT/focus, ALSA маршруты, LED, supervisor, install и rollback |

## Исходящий групповой путь

Source adapter → один paced PCM producer/MediaClock → immutable AudioPacket → SessionPool + optional local renderer. Входящие и исходящие ключи, nonce, RTP counters и session IDs независимы; общего clock/media epoch недостаточно для объявления native group adoption.

PtpGroup получает MasterTimeline существующей общей PTP-службы. with_clock_service проверяет идентичность watch-канала, принимает отдельный fresh owner token и явные timing ports. Выбор устройств задаёт полный список физически выбранных адресатов новым connection plans. Работающие сессии обновляют SETPEERS без перезапуска. Реальный running состав обновляет интерес группы в общей службе; освобождение одного интереса сохраняет адресаты других потребителей.

Потеря корректных часов/эпохи или сбой local renderer останавливает передачу группы. Общая PTP-служба продолжает жить. При нормальном EOF применяется drain; при сбое — stop и bounded cleanup. Hard abort не заменяет signal stop + await. Адаптер восстанавливает прямой локальный маршрут существующими механизмами.

## Профили и границы

- AP1 classic, AP2 realtime/buffered, control-only MRP и mirroring не смешиваются неявно.
- AP2 timing выбирается явно: ядро имеет NTP и PTP; групповой профиль использует PTP. Обязательность NTP для всех AP2-устройств по закрытой спецификации Apple не заявляется. In-session fallback отсутствует.
- Общая служба может жить без групп. MasterTimeline TX пока ограничен qualified local master; полноценный follower TX/RAW bridge не объявлены готовыми.
- Универсальный audio-only Cargo profile ещё не перенесён из старого локального черновика. Выключенный показ видео и исключённые зависимости — разные вещи.
- Ни Session Running, ни SETUP/SETPEERS ACK, ни MRP-галочка не доказывают звук, синхронизацию или принятие топологии.

## Перенос на Станцию

Сначала закончить continuous/PIN/runtime tests и разделение профилей. Затем pin версии ядра и адаптер native/AirPlay/BT PCM, отдельное локальное приглушение/голос, сохранение guards/autostart и recovery. Не возвращать второе скачивание/декодирование Alice CDN и не переносить аппаратные пути в протокол.

После автономных проверок — только отдельно разрешённая установка с откатом. Последние overlays/tap и smi10/smi11 compatibility проверяются по выбранному backend; собственная PTP-служба не требует NQPTP ABI. Звук/Home app/карточка/групповая подпись проверяются раздельно.

Подробный остаток — [STATUS](STATUS.md); историческая последовательность — [журнал](STATION_MIGRATION_20261111.md). Старый архитектурный аудит сохранён в docs/history, его TODO не является текущим планом.
