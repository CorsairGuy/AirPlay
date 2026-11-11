# AirPlay 1/2: протокол и состояние реализации

Актуализировано12ноября2026. Полной официальной публичной спецификации Apple здесь нет. Документ различает известный wire profile, наблюдение, политику реализации и неизвестное.

## Где находятся сведения

- [Консолидированный wire reference](AIRPLAY-WIRE-SPEC.md): последовательности, framing/байтовые форматы, pairing, RTP, timing, MRP, screen mirroring и закреплённые первичные источники. Его исходные замеры/схема помечены своим срезом; они не являются автоматически актуальным отчётом о реализации.
- [Текущая матрица реализации](STATUS.md) и [проверки](VALIDATION.md).
- [Разделение профилей](SESSION_PROFILES.md), [sender API](../crates/sender/README.md).
- Текущие typed field mappings: crates/mrp/src/schema.rs и proto provenance. Исторический AppendixA wire reference относится к указанному там snapshot, не к нынешнему числу полей.

## Профили, которые нельзя подменять друг другом

| Профиль | Аудио / каналы | Timing / управление |
|---|---|---|
| Classic AP1 | Собственный RAOP/RTSP профиль | NTP-подобный обмен; DACP в соответствующем профиле |
| AP2 realtime | type96, согласованные codec/format/ключи; PCM TX в ядре экспериментален | NTP или PTP согласно выбранному профилю; нельзя менять незаметно после SETUP |
| AP2 buffered | type103, согласованные codec/format и buffering | Поддержка RX не означает наличие buffered TX или принудительный выбор этого режима приложением iPhone |
| AP2 MRP control-only | Зашифрованный data/control tunnel; отдельная media ownership | MRP — управление/состояние, не RTP payload и не доказательство играющей media-сессии |
| Mirroring | Отдельный согласованный video/audio/key profile | Современный AP2/PTP/group вариант не доказывается наличием legacy mirroring |

Для нашего группового TX выбран PTP, NTP не является зависимостью этой PTP-сессии. Закрытая спецификация не позволяет объявлять здесь NTP обязательным или запрещённым для всех AP2-устройств. Нельзя определять версию только по _raop._tcp, кодек — по type96, или native playback HomePod — только по приложению на iPhone.

## Синхронизация и группы

Одна общая clock service и один физический источник времени обслуживают группы. Каждая группа имеет независимые session/auth state и lifetime interest в union адресатов. PtpPlan связывает SETUP/SETPEERS/d7 с clock ID/epoch; producer штампует общий PCM один раз. Receiver render minimum не учитывается повторно. LocalSink получает ту же immutable PCM и presentation deadline, а не отдельное немедленное проигрывание.

SETUP/SETPEERS ACK означает принятие запроса транспортом. Running означает состояние задачи. Ни то ни другое не является native adoption, аудиоподтверждением или измерением синхронности. Эти наблюдения и attempt/epoch нельзя подменять друг другом.

## Что остаётся неполным

Continuous relay lifecycle, интерактивный PIN в активном output-selection, native topology/UI, общий frontend/adaptor и физическая матрица совместимости. Buffered TX/AP1 TX/полная BMCA/servo и современное mirroring+AP2 не объявлены завершёнными.

Старый аудит30октября сохранён в [истории](history/AIRPLAY-PROTOCOL.md). Его утверждения «MRP/отправителя нет» больше не описывают main.
