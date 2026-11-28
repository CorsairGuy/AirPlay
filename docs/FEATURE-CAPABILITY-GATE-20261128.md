# Capability gate: бит Features → обработчик (D14, R05)

Проверка `crates/device` и `runtime` от 28 ноября 2026. Таблица перечисляет
только те биты, для которых в коде есть обработчик или явный отказ. Бит,
названный в профиле, сам по себе не доказывает поддержку.

## Таблица

| Бит | Константа | Обработчик / условие | Если обработчика нет |
|---|---|---|---|
| 0 | `VIDEO` | зеркалирование: `mirror` sink + `--mirror` | запуск отклоняется новым gate |
| 1 | `PHOTO` | `sessions::photo::Service` (опция `photo`) | пока не гейтится; `/photo` отвечает 501 |
| 7 | `SCREEN` | `mirror` sink + `--mirror` | запуск отклоняется новым gate |
| 9 | `AUDIO` | `Player` + realtime/buffered потоки | всегда есть |
| 11 | `AUDIO_REDUNDANT` | redundancy в RTP-приёмнике | тесты rtp |
| 14 | `FAIRPLAY_AUTH` | `crates/fairplay` `/fp-setup` | без `--mirror` ключ не нужен |
| 15–17 | `METADATA_*` | `Events` (карточка, MRP) | `NoEvents` |
| 18–22 | `AUDIO_FORMATS_*` | `codec::matrix::Advertisement` | неподдержанная комбинация → 500 SETUP |
| 30 | `UNIFIED_ADVERTISER_INFO` | `GET /info`, TXT | — |
| 38 | `UNIFIED_MEDIA_CONTROL` | канал 130 + MRP | логическая регистрация без `seed` |
| 40 | `BUFFERED_AUDIO` | SETUP тип 103 | без `shk` → отказ |
| 41 | `PTP` | `timesync::TimeSync::start` при `time: None` | если PTP не запущен, бит в профиле всё равно объявляется — известное расхождение |
| 42 | `SCREEN_MULTI_CODEC` | H.265 в mirror | только при mirror backend |
| 46–48 | pairing bits | `services/sessions` HAP | — |

## Что изменено

`runtime::lifecycle::receiver::start` теперь отказывается запускаться, если
профиль объявляет `SCREEN` или `VIDEO`, а backend зеркалирования не
сконфигурирован. Биты профиля не переписываются: OtherTV остаётся ровно
`0x8300527FFEE6`. Тест —
`runtime/tests/feature_gate.rs` (отказ и неизменные аудиобиты в `/info`).

`services/sessions` проверяет, что OtherTV-биты совпадают в `/info`,
AirPlay TXT и RAOP TXT и что `deviceID`/`psi`/`pk` — собственные
(`other_tv_bits_stay_identical_in_info_plist_and_both_txt_records`).

## AppleTV14,1 и HomePodMini

`tests/interop/fixtures/appletv14-1.reference.plist` хранит только
наблюдённые поля: модель `AppleTV14,1`, `sourceVersion 960.13.1`,
`features 0x3C177FDE4A7FDFD5`, `statusFlags 0x10644`,
`hasUDPMirroringSupport`, `featuresEx`. Нет ID, ключей, имени. Это не
`Profile`: маска записана строкой, `display` отсутствует, поэтому
`Profile::from_value` возвращает ошибку — сырой захват не запускается.

`homepodmini.reference.plist` содержит только подтверждённые снимком Станции
`model AudioAccessory5,1` и `sourceVersion 550.10`; полная маска помечена
`unknown` и не подменяется маской другого устройства.

Неподдержанные/отсутствующие поля AppleTV: `display` (ширина/высота/цвет),
`accessory_type`, `enclosure_color`, `audio_only_ui`, `raop`; `featuresEx`
не разобран по битам. Профиль для объявления не создан.

## Ограничения

- Бит `PTP`: профиль с битом 41 при `time: Some(TimeSync::idle)` объявляет
  PTP, не имея работающего PTP; это осознанно не гейтится, чтобы не ломать
  loopback-тесты, и требует решения координатора.
- `PHOTO` не гейтится: `/photo` отвечает 501, но бит может быть объявлен.
- Проверка с iPhone/Apple TV не выполнялась.
