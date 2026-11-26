# Документация AirPlay

Документация разделяет текущий код, задания и исторические исследования. При противоречии датированной заметки и текущего состояния проверять код, [статус](STATUS.md) и [задачи](AGENT-TASKS.md); старые результаты тестов не переносятся на новые коммиты.

## Начать здесь

- [Состояние реализации и ограничения](STATUS.md) — что существует и что ещё не подтверждено.
- [Задачи агентов](AGENT-TASKS.md) — приоритеты, исполнители, критерии и команды тестирования.
- [Архитектура](ARCHITECTURE.md) и [компоненты и имена](COMPONENTS-AND-NAMING.md) — границы кода и история реорганизации.
- [План v12](AGENT-PLAN-AP12-20261112-v12.md) — требования, референсы и открытые вопросы, а не свидетельство реализации.
- [Протокол](PROTOCOL.md), [wire reference](AIRPLAY-WIRE-SPEC.md), [аудит референсов](REFERENCE-AUDIT-20261125.md) — факты и источники.
- [Тестирование](TESTING.md) и [результаты проверок](VALIDATION.md) — методика и датированные свидетельства.

## Возможности

- Аудио: [AP2 RX](AUDIO-RECEIVE-20261125.md), [AP1 RX](RAOP-RX-20261125.md), [AP1 TX](AIRPLAY1-TX-20261125.md), [buffered TX](BUFFERED-TX.md), [кодеки](AUDIO-CODEC-MATRIX.md), [multichannel](MULTICHANNEL-20261125.md).
- Видео: [mirroring TX](MIRROR-TX-20261125.md), [fanout](MIRROR-FANOUT-20261125.md), [исследование](MIRRORING-AIRPLAY2-RESEARCH.md), [фото](PHOTO-PROFILE-PHASE8-20261125.md), [Windows TV](WINDOWS-TV.md).
- Управление: [MRP](MRP-CARD-PHASE6-20261125.md), [Companion](COMPANION-REMOTE-20261125.md), [touch](COMPANION-TOUCH-20261125.md), [доверие](ROUTE_AUTHORIZATION.md).
- Устройства и группы: [профили](DEVICE-PROFILES-20261125.md), [discovery](DISCOVERY-PROFILES-20261125.md), [continuous relay](CONTINUOUS_RELAY.md), [эксперименты групп](GROUP-BUTTON-EXPERIMENTS.md).
- Разработка: [сборочные профили](BUILD_PROFILES.md), [безопасность](SECURITY.md), [сеансы](SESSION_PROFILES.md), [передача работы](HANDOFF.md).

Датированные документы об отдельных ветках сохраняют исходные наблюдения и команды, а не актуальный статус веток. История переносов и ранние документы сохранены для происхождения решений; файлы лицензий и референсы находятся рядом с соответствующим кодом.
