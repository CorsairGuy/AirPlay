# Биты возможностей: объявление против реализации (C56)

Сверка 2026-11-27 по актуальному `main` (`02dc811`). Все профили устройств
остаются вне универсального ядра; таблица показывает только, какой путь
существует в коде и каким тестом он покрыт. Наличие бита в профиле **не**
доказывает поддержку на устройстве и не подтверждено на реальном Apple TV,
HomePod или iPhone.

## Маска

| Бит | Константа `device::Features` | Объявляется в профиле | Фактический путь в ядре | Тест / статус |
| --- | --- | --- | --- | --- |
| 0 | `VIDEO` | `tv.plist`, `mac.plist` | приём видео (H.264/H.265) в `crates/mirror`, декодер за фичей `ffmpeg` | `mirror` фрейминг/fanout; декодер C07/C28 заблокирован (FFmpeg) |
| 1 | `PHOTO` | `mac.plist` | HTTP-фото в `sessions::photo`, границы в C22 | тесты C22, устройство не проверялось |
| 7 | `SCREEN` | `tv.plist` | `Display` + `displays` в `/info`, обязателен при бите | `device` тесты, `runtime` MIRROR_RECEIVER |
| 9 | `AUDIO` | все аудио-профили | realtime (96) и buffered (103) приём | `audio_stream` тесты |
| 11 | `AUDIO_REDUNDANT` | `speaker.plist` | повторные пакеты/резервный поток в `audio_stream::net` | тесты `net`, устройство открыто |
| 14 | `FAIRPLAY_AUTH` | `mac.plist` | FairPlay v2 — только каркас `crates/fairplay`, ключи не выпускаются | не подтверждено |
| 15 | `METADATA_PROGRESS` | аудио-профили | `media::events::Progress` → карточка | `remote` тесты |
| 16 | `METADATA_ARTWORK` | аудио-профили | обложка в `remote::card`, границы 2 МиБ | `remote` тесты; stale artwork — задача C29 |
| 17 | `METADATA_TEXT` | аудио-профили | title/artist/album в `remote::model` | `remote` тесты |
| 18–22 | `AUDIO_FORMATS_1..4` (18, 19, 20, 22) | профили | ALAC/AAC/PCM в `crates/codec`, SDP-матрица C03 | `media` тесты C03 |
| 30 | `UNIFIED_ADVERTISER_INFO` | все профили | `_airplay._tcp` TXT с `features`/`pi`/`psi` | `mdns`, `discovery` тесты |
| 38 | `UNIFIED_MEDIA_CONTROL` | все профили | MRP-канал 130 (`services/remote`) | `remote` тесты; выбор трека — C26/C31 |
| 40 | `BUFFERED_AUDIO` | аудио-профили | buffered TX/RX (103), nonce C16 | `media`, `audio_stream` тесты |
| 41 | `PTP` | аудио-профили | `services/timesync`, `crates/clock` | loopback-тесты; реальный PTP — открыто |
| 42 | `SCREEN_MULTI_CODEC` | `MIRROR_RECEIVER_HEVC` | H.265 в mirror-приёме | `mirror` тесты; устройство не проверялось |
| 46 | `HK_PAIRING_AND_ACCESS_CONTROL` | `tv.plist` | HAP pair-setup/verify (`crates/pair`) | `pair` тесты C38/C43 |
| 47 | `SYSTEM_PAIRING` | `tv.plist` | системное сопряжение (pair-setup по PIN) | `pair` тесты |
| 48 | `COREUTILS_PAIRING_AND_ENCRYPTION` | `tv.plist` | шифрованный control-канал (`crypto::channel`) | тесты C39 |

## Правила

- `MIRROR_RECEIVER` намеренно не выставляет биты 27, 40, 41, 48: у профиля
  зеркалирования нет buffered-аудио, PTP и legacy-сопряжения. Это проверяет
  `crates/device/tests/capability_bits.rs`.
- `AUDIO_RECEIVER` не выставляет `SCREEN`: аудио-профиль не объявляет экран.
- Несогласованный профиль (`SCREEN` без `display` и наоборот, нулевая маска,
  `status_flags` больше `u32`, лишние ключи) отклоняется при разборе
  (`crates/device/src/profile.rs`), а не исправляется молча.
- `features` в TXT (`0xНИЗ,0xВЕРХ`) и `featuresEx` в `/info` кодируют одну и ту
  же маску; расхождение считается ошибкой профиля.
- Бит, для которого нет проверенного пути или теста, не рекламируется как
  подтверждённый: строки «не подтверждено» выше остаются открытыми до
  аппаратной проверки.

## Ограничения

- Таблица описывает ядро, а не конкретную модель: `docs/profiles/*.plist` —
  примеры представления, а не доказательство совместимости.
- FairPlay, PTP-роли и выбор трека в системной карточке требуют отдельных
  задач (C26/C31/C32) и реальных устройств.
- Аппаратная приёмка не проводилась.
