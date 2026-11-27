# Чек-лист координатора: `review/today-20261127`

Только порядок действий, числа и границы. Состав прогона и шаги W01–W10 не
дублируются — они в `scripts/verify-today.sh` и
[TEST-PLAN-IPHONE-WINDOWS-20261127.md](TEST-PLAN-IPHONE-WINDOWS-20261127.md).

Ветка `review/today-20261127` собрана от `integration/all-new-20261127`
(`ba584e7`); сверх базы: `3407b4e`, `671de98`, `2481fcf`, `4326dca` и этот
файл. Верхний коммит печатает `git log --oneline -1`. `main` с момента
ответвления ушёл на один коммит вперёд (`9369fc9`, «Restore strict workspace
clippy checks»), но файлы не пересекаются: у него `runtime/tests`,
`identity/state.rs`, `media/raop.rs`, `media/tx/*`.

## 1. Команды

```bash
cargo test --offline -p timesync --lib    # точечно: новый тест маршрута relay
./scripts/verify-today.sh quick           # fmt + clippy + 4 крейта + workspace + runtime
./scripts/verify-today.sh                 # то же + interop (нужны pyatv/srptools)
```

`OFFLINE=1` перед скриптом — если нет сети и кэш прогрет. Скрипт печатает
`!!! FAILED` на упавшем шаге, итог в конце и возвращает ненулевой код.

## 2. Шесть имён, которых нет в `main`

| Тест | Крейт |
|---|---|
| `relay_round_trip_converts_the_inbound_stamp_exactly_once` | `timesync` |
| `classic_flush_drops_a_pending_candidate` | `audio_stream` |
| `declared_boundary_flushes_without_waiting_for_a_series` | `audio_stream` |
| `packets_from_before_the_declared_boundary_do_not_return` | `audio_stream` |
| `alternating_scales_never_confirm_a_new_epoch` | `audio_stream` |
| `long_session_keeps_one_scale_through_losses_and_reordering` | `audio_stream` |

## 3. Ожидаемое число тестов

Фиксированной константы нет: ветка складывает несколько слитых ветвей, и
счётчик зависит от базы. Ориентиры — последние зафиксированные прогоны в
`docs/` на своих базах:

| Крейт | Ориентир | Добавлено этой веткой |
|---|---|---|
| `clock` | 73 (`NQPTP-SHM-20261126.md`) | — |
| `timesync` | 42–52 (`DUAL-CLOCK-ROLES`, NTP-задача) | +1 |
| `audio_stream` | 27 (`EPOCH-RECOVERY-20261126.md`) | +5 |
| `media` | 149 (`AGENT-TASKS.md`) | — |

Оговорка по `AGENT-TASKS.md`: он описывает `Playout::rebase` применительно к
ветке `feat/epoch-recovery`. В этой ветке такого метода нет — объявленная
граница приходит в `flush_classic`, тесты выше это и закрепляют.

Признак успеха — `test result: ok` без `FAILED` и наличие всех шести имён выше.
Отклонение счётчика на несколько тестов из-за слитых ветвей — не поломка;
падение, пропавшее имя или `FAILED` — поломка.

## 4. Что сознательно не входит

- HomePod/AirPlay 2-группа, Apple TV, зеркалирование и видео как стенд:
  зеркалирование проверяется только smoke-тестом `--features mirroring`.
- Живые PTP/NQPTP-часы: нет чтения SHM с железа и нет grandmaster-эмуляции.
- Bit-exact сверка PCM, задержки lead, длительное прослушивание: нужен
  измерительный стенд и захват с устройства.
- Шаг `pyatv_client.py` — независимый клиент, а не iPhone; `SKIP` из-за
  отсутствия pyatv/srptools не считается пройденной проверкой.
- Аппаратный прогон W01–W10 — за человеком, iPhone → Windows.

## 5. Что автором не проверено

Ни компиляция, ни `cargo test`, ни clippy/fmt, ни interop, ни живой прогон:
`cargo` и `rustc` в окружении автора отсутствуют, установка тулчейна
запрещена. Тесты написаны против сигнатур, прочитанных в исходниках
(`InstantSource::new`, `ClockMap::new`, `MasterTimeline::new_dual`,
`reading_at_now`, `clock::domain::translate`, `Playout::flush_classic`), но не
собирались. Ожидаемый класс ошибок при сборке — порядок/типы аргументов, а не
логика.

## 6. После приёмки

Fast-forward невозможен: в `main` есть `9369fc9`, которого нет в ветке.
Нужен обычный merge (файлы не пересекаются, конфликтов быть не должно) или
rebase ветки на `main` — решает координатор. Ветки `feat/c01…c87`,
`wip/dual-clock-peer-roles-20261126`, `feat/long-session-recovery` не вливать:
их работа уже в `main` в переписанном виде либо не завершена.
