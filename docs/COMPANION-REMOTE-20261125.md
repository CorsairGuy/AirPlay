# Companion Link: пульт и клавиатура — 25 ноября 2026

Ветка `feat/companion-remote` от `main` (`847fe38`). Закрывает пункт очереди
«Companion-пульт» и строку D13 плана (сверка с референсами), опираясь на
D41, D42, L-52, L-61, L-80.

## Что сделано в коде

`crates/companion` (до этого — только doc-комментарий):

| Модуль | Содержимое |
|---|---|
| `opack` | Сборка/разбор OPACK: bool, null, целые (короткие и 1/2/4/8 байт), float32/64, строки и данные с расширенной длиной, UUID, массивы/словари до `0x0F` элементов и «бесконечные» с терминатором `0x03`, ссылки `0xA0..0xC4`. Предел вложенности 64. |
| `frame` | Кадры Companion: 4-байтовый заголовок (вид + длина u24 BE), 17 видов (`PairSetupStart=3` … `FamilyIdentityUpdate=34`), ChaCha20-Poly1305 с 12-байтовым nonce-счётчиком LE и заголовком в AAD, длина включает тег 16 байт; пустые кадры не шифруются. |
| `auth` | Клиентский pair-verify: TLV8 в ключе `_pd`, X25519, проверка подписи приёмника, ChaCha20 `PV-Msg02`/`PV-Msg03`, вывод ключей канала `ClientEncrypt-main` (запись) и `ServerEncrypt-main` (чтение) через HKDF-SHA512 с пустой солью. |
| `commands` | Конверты `{_i,_t,_c,_x}`, `_sessionStart`/`_sessionStop`, `TVRCSessionStart` (L-80), `_hidC` (19 HID-команд), `_mcc` (13 команд), громкость `_vol` как доля 0..1, перемотка `_skpS` (double — отрицательные целые OPACK не кодирует), `_interest`, `_tiStart`/`_tiStop`, `FetchAttentionState` и разбор `SystemStatus`. |
| `link` | Сборка/разбор без транспорта: выдача `_x`, кадр `E_OPACK`, буфер и разбор OPACK из кадра. TCP подключает вызывающий. |

Тестов: **27** (`cargo test -p companion`), включая золотые байты OPACK,
разбор ссылок, шифрованный обмен двумя экземплярами, отказ при подмене
кадра, чужом приёмнике и ошибке `_pd`. Clippy `--all-targets -D warnings` и
`cargo fmt --check` чистые.

## Wire-факты и провенанс

Формат восстановлен по открытым референсам, код не портирован — сверялись
только байты:

- **D42 / apple-opack, pyatv `support/opack.py`** — теги и длины OPACK.
  Замечание: референсный сборщик дедуплицирует повторяющиеся значения в
  ссылки `0xA0..`, а декодер понимает ссылки длиной 1–4 байта (`0xC1..0xC4`),
  хотя сборщик пишет для `0xC3` четыре байта. Мы ссылки не порождаем; при
  разборе следуем декодеру (1/2/3/4 байта).
- **D41 / rust-atv, pyatv `protocols/companion`** — виды кадров, заголовок,
  направление ключей, значения `_auTy: 4`, `_hBtS: 1/2`, `_mcc`, `_vol`,
  `_skpS`, `_sid` = `(remote << 32) | local`.
- **L-61 / AppleTVr**, **L-52 / atv-core** — независимое подтверждение
  «4-байтовый заголовок + OPACK», SRP/Ed25519 и команд `_hidC`.
- **L-80** — `TVRCSessionStart` с `ProtocolVersionKey: "1.2"` до
  `FetchAttentionState` на tvOS 26; `SystemStatus`/`TVSystemStatus` — источник
  состояния питания.

## Что НЕ сделано и почему

- **Companion pair-setup по PIN** (PS-кадры 3/4, SRP-6a) не реализован: в
  проекте уже есть `crypto::srp` и `pair::hap::PairSetup`, но сквозной
  Companion-вариант (TLV `_pd` + `_pwTy`) требует отдельной проверки на
  устройстве. Сейчас поддерживается только pair-verify по уже сохранённым
  HAP-креденшелам — это и есть путь pyatv для настроенного пульта.
- **Touch/HID-события** `_hidT` и текстовый ввод через keyed archiver
  (`_tiC`) не реализованы: нужен архиватор NSKeyedArchiver, это отдельный
  слой, а не wire-команда.
- **Аппаратная проверка невозможна**: Apple TV в наличии нет. Ни один обмен
  с настоящим устройством не выполнялся, поэтому `_sessionStart`,
  `TVRCSessionStart` и `FetchAttentionState` помечены как непроверенные —
  успешная сборка кадра не доказывает совместимость.
- Companion не подключён к `services/remote` и CLI: это следующий шаг после
  появления устройства для проверки.

## Проверки

```
cargo test --offline -p companion          # 27 passed
cargo clippy --offline -p companion --all-targets -- -D warnings
cargo fmt -p companion -- --check
```

Прогон: MSYS2/UCRT64, `stable-x86_64-pc-windows-gnu`, offline.
