# Индекс документов

Актуальное состояние12ноября2026: [STATUS](STATUS.md). Единственная текущая матрица реализации; исторические TODO не заменяют её.

Порядок по уточнению пользователя: сначала завершить перевод Станции на ядро и проверки, затем выполнять будущие этапы AP1/AP2 из v11. Указание §0 п.7 плана о параллельном выполнении фаз2–3 уступает этому уточнению. Исходная редакция v10 сохранена в истории Git; в v11 внесено согласованное уточнение D9.

| Документ | Назначение |
|---|---|
| ../README.md | Обзор и существующие команды |
| [AGENT-PLAN-AP12-20261112-v11.md](AGENT-PLAN-AP12-20261112-v11.md) | Пользовательский план будущих этапов AP1/AP2, реестр решений и тест-матрица |
| [GROUP-BUTTON-EXPERIMENTS.md](GROUP-BUTTON-EXPERIMENTS.md) | План эталонных захватов и экспериментов с групповой кнопкой |
| ARCHITECTURE.md | Текущие слои, общие часы и перенос адаптера |
| PROTOCOL.md | Профили и границы достоверности |
| AIRPLAY-WIRE-SPEC.md | Wire reference с закреплёнными источниками и датированными наблюдениями |
| VALIDATION.md | Выполненные проверки и отсутствующие доказательства |
| SESSION_PROFILES.md | Границы AP1/AP2/control/mirroring |
| BUILD_PROFILES.md | Audio-only/default, границы feature и проверки |
| ROUTE_AUTHORIZATION.md | Connection-scoped PIN путь и проверки |
| CARD_UPDATES.md | История artwork/timeline/framing и ограничения UI |
| RELAY_IPC.md | Существующий Unix IPC contract; не native group adoption |
| AUDIO_RATE_ESTIMATOR.md | Измеритель аппаратной частоты; не автоматическая синхронизация |
| STATION_MIGRATION_20261111.md | Хронология переносов, не текущий backlog |
| HANDOFF.md | Продолжение работы и ограничения |
| history/ | Старые обзоры, сохранённые для происхождения решений |
| ../reference/ | Исторические исходники/черновики; не production implementation |

Нормативные лицензии/provenance не заменяются статусными заметками. Modern mirroring research остаётся исследованием; прежние физические подтверждения относятся к конкретным станционным сборкам.

- [README](../README.md)

- [Архитектура](ARCHITECTURE.md)

- [Протокол](PROTOCOL.md)

- [Wire reference](AIRPLAY-WIRE-SPEC.md)

- [Статус](STATUS.md)

- [Проверки](VALIDATION.md)

- [Передача](HANDOFF.md)

## Ранее загруженные вручную черновики

Корневые AIRPLAY-PROTOCOL.md и AIRPLAY-ARCHITECTURE-AND-MIGRATION.md были старым аудитом30октября. Их исходные версии сохранены в history; актуальные тексты — PROTOCOL.md и ARCHITECTURE.md. Прежний HANDOFF также в history; новый — HANDOFF.md. Исследование зеркалирования перенесено в docs без изменения результатов опытов. Даты в истории не заменены на дату обновления. README компонентов и лицензионные/provenance файлы остаются возле компонентов.

- [Continuous relay: перенесённое состояние источника и оставшаяся интеграция](CONTINUOUS_RELAY.md)
