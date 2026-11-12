# Передача работы — 12 ноября 2026

Актуальная база — [STATUS](STATUS.md), [архитектура](ARCHITECTURE.md), [протокол](PROTOCOL.md). Старые указания «sender/MRP не сделаны» заменены текущей матрицей; предыдущий handoff сохранён в docs/history.

## Следующая работа

1. Комплексно проверить PTP group selection/peer reconciliation/cancellation; opt-in continuous/Card pipeline уже есть, frontend/Station integration остаётся.
2. Проверить PIN на iPhone (сквозной loopback уже проходит); довести native topology/button/slider на основании захватов.
3. Audio-only profile перенесён и проверен. Локальные Glagol deadline/Retry-After перенести в адаптер Станции; обновить differential harness под нынешний API.
4. Подключить адаптер Станции к pin ядра, сохранив native PCM tap, AirPlay/BT, локальный голос/duck и готовое recovery/autostart.

## Ограничения

- Пользователь разрешил push и объединение в main. Установка, перезапуск и звук на домашних устройствах требуют отдельного согласия. Разрешены автономные Pi/null/loopback тесты.
- Браузер не трогать. Писать кратко по-русски.
- Одна общая PTP-служба; группа её не создаёт и не выключает. Stop + await, не hard abort для штатного cleanup.
- Голос Алисы только локально; HomePod-контент не приглушать при ответе. Не возвращать второй CDN/decode.
- Не терять отдельные незакоммиченные диагностические events.rs/audio_stream/net.rs, clock integration test и перестановку импортов card.rs. Они исключены из последнего опубликованного/tested source.
- В другом worktree airplay-core остались старый исходный audio-only черновик (уже портирован выборочно), mediaremote/MRP/sender и документы. Сверять полезные отличия, не накатывать целиком.
- Station source: поздняя ld5dy4 a538a30; интеграционная ветка сохраняет её runtime и опубликованные архивы/документацию. Локальные station-* часто являются контекстными портами/альтернативами, не списком готовых независимых функций.

Последние проверки:311 core library tests;137 Station Rust tests + C focus-state. Это не полный cross-platform CI и не новая физическая проверка.
