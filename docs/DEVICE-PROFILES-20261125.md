# Внешние профили устройства

Ветка `feat/device-profiles` от `main` (`847fe38`), не влита. Убран
единственный зашитый набор имени/модели/флагов: приёмник может взять их из
plist-файла, не пересобирая программу. Windows-интерфейс не менялся.

## Формат

`airplay --profile ПУТЬ` читает XML- или binary-plist со словарём:

| Ключ | Тип | Обязателен | Проверка |
|---|---|---|---|
| `name` | string | нет | непустая, ≤ 64 байт |
| `model` | string | да | ASCII, ≤ 64 байт |
| `features` | integer или `"0x..."` | да | не 0; бит SCREEN требует `display` и наоборот |
| `status_flags` | integer | нет (0x404) | ≤ u32 |
| `source_version` | string | нет (366.0) | цифры и точки, ≤ 32 |
| `accessory_type` | integer | нет | идёт в TXT `at` |
| `display` | dict `width`/`height`/`fps` | при SCREEN | 1..7680, fps 1..120 |
| `audio_only_ui` | boolean | нет | карточка MRP всё равно требует `--mrp` |
| `enclosure_color` | string | нет | ≤ 32 байт |
| `raop` | boolean | нет (true) | `false` — объявлять только `_airplay._tcp` |

Неизвестные ключи и ключи неизвестного типа отклоняются: опечатка в профиле
не должна молча превращаться в другое устройство. Значения профиля заменяют
соответствующие параметры командной строки (`--model`, `--features` и т. п.);
это указано в `--help`.

Примеры: `docs/profiles/speaker.plist` (колонка без экрана),
`docs/profiles/tv.plist` (экран 1920×1080@60) и `docs/profiles/mac.plist`.
Это описания того, чем приёмник представляется, а не заявление о
реализованных возможностях: маска не делает поддержанным то, чего нет в коде.

## Изменения

- `crates/device/src/profile.rs` — загрузка, строгий разбор и валидация.
- `DeviceInfo.raop` — профиль может выключить `_raop._tcp`;
  `sessions::Context::services` учитывает это.
- `serve::Options.profile` и `--profile` в разборе аргументов.

## Тесты

`cargo test --offline -p device -p sessions -p airplay`:

- device: 8 тестов, включая разбор трёх примеров, hex-маску, отказ для
  неизвестного ключа, неверной версии, `display` без бита SCREEN, нулевой
  маски и неверного типа `raop`.
- sessions: 23 теста, включая `profile_can_leave_only_the_airplay_service`.
- airplay: 4 теста, включая загрузку `tv.plist` через `--profile`.

`cargo clippy --offline -p device -p sessions -p airplay --all-targets` — без
новых предупреждений.

## Границы

Профили не проверялись с реальным iPhone/Apple TV: какие маски и модели
действительно принимает iOS, по-прежнему требует аппаратного захвата.
