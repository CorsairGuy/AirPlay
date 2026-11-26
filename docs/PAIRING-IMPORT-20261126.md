# Импорт identity/pairing и маркер авто-парирования (G2)

26 ноября 2026. Ветка `feat/pairing-import` от `origin/main` `b612eb4`. Задача G2 из [STATION-CORE-GAPS-20261126](STATION-CORE-GAPS-20261126.md).

Аппаратной проверки нет: iPhone, Apple TV и AirPort Express недоступны, живое авто-парирование не запускалось.

## Что решено

1. **Маркер авто-парирования переносится в ядро**, но только как факт хранилища, без политики. У Станции (`src/airplay_sender/src/pairing.rs:112`) `mark_auto_pair_tried` — это файл-маркер на получателя, который не даёт повторять автоматическую попытку после перезапуска и при ошибке записи считается «уже пробовали». Само решение «пробовать ли авто-парирование» остаётся в обвязке: в ядре нет цикла авто-парирования, есть только явный `PinPairing`.
2. **`import_new` вне Linux теперь отказывает явно.** Раньше метод существовал только под `cfg(target_os = "linux")`, поэтому вызов из другой платформы просто не компилировался, а сторонний код мог решить, что импорта нет вовсе. Теперь есть заглушка с той же сигнатурой и понятной ошибкой; неатомарного fallback нет, потому что он мог бы перезаписать существующее состояние.

## Изменения — `services/identity/src/sender_store.rs`

- `SenderStore::mark_auto_pair_tried(&self, receiver: &str) -> bool`:
  - маркер — файл `auto-pair-tried/<hex uid>` в приватном каталоге хранилища; каталог `0o700`, файл `0o600` через существующий `create_file` (`create_new`);
  - `true` — попытка уже была, UID некорректен, или маркер не удалось записать (fail-closed, как у Станции);
  - `false` — маркер только что создан этим вызовом;
  - не трогает `identity`/`receivers`, не создаёт identity и не меняет пары.
- `import_new` под `cfg(not(target_os = "linux"))` — явный отказ `atomic sender-store import requires Linux renameat2`.
- Тесты:
  - `auto_pair_attempt_is_recorded_once_and_fail_closed` — повторный вызов `true`, другой UID `false`, после переоткрытия хранилища `true`, пустой UID `true`;
  - `import_is_refused_explicitly_off_linux` (только не-Linux) — ошибка содержит `Linux`, каталог не создан.

## Проверка

```
rustup run stable-x86_64-pc-windows-gnu cargo test --offline -p identity
  8 passed; 0 failed   (в т.ч. оба новых теста)
rustup run stable-x86_64-pc-windows-gnu cargo clippy --offline -p identity --lib -- -D warnings
  ok
rustup run stable-x86_64-pc-windows-gnu cargo fmt -p identity -- --check
  ok
```

Windows-прогон не выполняет Linux-ветку `import_new` и её четыре теста: они под `cfg(target_os = "linux")` и здесь не компилируются. Сам код Linux-импорта не менялся.

## Ограничения

- Реальная миграция состояния Станции (снимок accessory ID → physical UID) не запускалась; `import_new` по-прежнему требует Linux и не создаёт identity.
- Маркер не связан с `PinPairing` автоматически: обвязка должна сама спрашивать его перед авто-попыткой.
- Поведение auto-pair на реальном iPhone/HomePod не проверено — аппаратной проверки нет.
