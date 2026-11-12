# Текущее состояние — 12 ноября 2026

База последнего полного библиотечного прогона: `dd23261f2b8eebd8cae9b9c211361ba698c57a08`. Более поздние scoped-проверки continuous pipeline перечислены в [VALIDATION.md](VALIDATION.md). Дата документа не означает новую установку на устройство.

## Матрица реализации

| Область | В коде | Ограничение / следующий шаг |
|---|---|---|
| Приём аудио | AP2 realtime96 и buffered103; codec/format выбираются сессией; receiver events и команды источнику | Полная матрица отправителей/кодеков/ошибок не подтверждена; classic AP1 audio остаётся отдельным пробелом |
| Отправка | Экспериментальный realtime96 PCM S16BE/44.1kHz/stereo; HAP, RTSP, RTP/crypto/retransmit | Не buffered TX, не AP1 TX и не готовый универсальный релиз |
| Timing TX | Явные NTP или PTP; PtpPlan/SETPEERS/d7, общий MediaClock | Для группового профиля выбран PTP; явный отказ PTP не вызывает скрытый NTP fallback |
| Общие часы | Одна внешне созданная Service с одним источником времени/парой сокетов; union адресатов разных владельцев | Не отдельный PTP на группу. MasterTimeline сейчас принимает квалифицированный local-master профиль; follower/внешний RAW bridge не объявлены готовыми TX-профилями |
| Группа TX | SessionPool, add/remove, реальные running members, live SETPEERS, общий producer, PtpGroup/local supervision | SETUP/SETPEERS ACK и Running не доказывают native adoption или акустическую синхронность |
| Отмена | Потеря роли/эпохи/свежести, stop и local failure завершают передачу; finish освобождает интерес группы | Общая PTP-служба не выключается. Stop + await обязателен для гарантированного graceful cleanup; hard abort не равнозначен |
| Pairing | Transient, PIN setup, постоянные credentials/pair-verify, store и MRP59/60/61 helper | MRP reader → request context → worker/PTP wrapper → явная InteractivePinForUnpaired policy соединены; сквозной MRP/PIN/persistent RTSP loopback проверен; iPhone interop ещё нужен |
| Discovery | DNS-SD browser/feed, logical/physical catalog, aliases/cluster/leader descriptors | Объявление возможностей не заменяет проверку транспорта; весь runtime не включается автоматически |
| MRP | Карточка, frame boundaries, команды/таймлайн, кеш обложки, outputs backend и отдельная/общая громкости | Общая подпись кнопки и удержание ползунков в реальной группе не подтверждены исправленными |
| Continuous relay | Source Timeline/pump/broadcast, queue/cache guards, единый PtpGroup runtime и opt-in MRP-card SourceControl с playback observer/grace | Default frontend composition, native/BT adapters, receiver/hardware buffers и полный control автомат остаются; [подробности](CONTINUOUS_RELAY.md) |
| Mirroring/video | Отдельный существующий профиль и декодирующие компоненты | Modern mirroring вместе с AP2/PTP/group остаётся исследованием; 60fps не гарантируются для любого железа |
| Минимальная сборка | Существующий Cargo workspace и ffmpeg feature | Локальный audio-only/mirroring feature-патч старого worktree ещё не перенесён; --no-default-features пока не обещает отсутствие video зависимостей |
| Станция | Отдельный адаптер Quasar/Glagol, ALSA tap/output, BT/focus, LED, install/recovery | Не переведён на новое ядро. Готовые guard/recovery Claude сохраняются, не реализуются заново |

## Что подтверждено

- Последний полный автономный прогон: `cargo test --workspace --lib --locked --offline` — **311 passed**, Linux/Pi, контейнер `--network none`. Базовый код + удаление неиспользуемых private methods; диагностические незакоммиченные файлы исключены.
- Объединённый исходный срез Станции4ead45f:137 Rust-тестов (MRP80, sender28, control19, broker10) и C focus-state check. Документ обзора старого MRP-черновика добавлен отдельно; установки не было.
- Это не результат всей CI-матрицы, ffmpeg/cross-clippy или нового физического теста. Ранее физически подтверждённый станционный звук не доказывает правильность новой сборки ядра.
- При одном более раннем прогоне старый PCM loopback test увидел0 совпавших кадров; полный повтор без изменения сборки прошёл. Случай сохранён в журнале, не скрыт как доказанная стабильность.

## Остаток работ

1. Довести continuous runtime до frontend/адаптеров и комплексной проверки нескольких получателей. Opt-in композиция карточки и pipeline уже проверена:164 теста (application86 + sender78), включая MRP Pause/Play → PCM в памяти; реальное оборудование не проверялось.
2. iPhone interop connection-scoped PIN пути (автономный сквозной обмен уже проверен); комплексные add/remove/failure проверки PTP-группы.
3. Native group topology/подпись кнопки/ползунки: сверять с захватами, не менять gid и capability flags наугад.
4. Перенести audio-only профиль и полезные локальные Glagol deadline/Retry-After правки в соответствующие слои.
5. Подключить frontend/адаптер Станции к общему ядру с pin версии и сохранить capture/routes/recovery/BT/LED.
6. Отдельно разрешённые домашние проверки всех источников, голоса Алисы, синхронизации и сбоев. Проверку buffered-only Яндекс Музыки делать на отдельном тихом приёмнике.

Голос Алисы остаётся локально. Контент HomePod во время ответа не приглушать по решению пользователя. Согласие на push/main не является согласием на установку или воспроизведение.

## Навигация

- [Архитектура и перенос](ARCHITECTURE.md)
- [Протокол и границы достоверности](PROTOCOL.md), [wire reference](AIRPLAY-WIRE-SPEC.md)
- [Журнал переносов](STATION_MIGRATION_20261111.md) — хронология, не текущий TODO.
- [Проверки](VALIDATION.md), [индекс документов](README.md), [передача работы](HANDOFF.md).
