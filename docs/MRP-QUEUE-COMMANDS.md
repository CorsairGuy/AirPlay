# MRP: команды очереди и их подтверждённые поля

Источники: pinned `crates/mrp/proto` (снят из pyatv), накопленные wire-факты в
[MRP-CARD-PHASE6](MRP-CARD-PHASE6-20261125.md) и [COMPANION-REMOTE](COMPANION-REMOTE-20261125.md).
Таблица отделяет «имя enum существует» от «семантика поля подтверждена».

## Команды

| Команда | Номер | Где объявлена | Что подтверждено | Состояние в проекте |
| --- | --- | --- | --- | --- |
| `SetPlaybackQueue` | 48 | `CommandInfo.proto:47` | только имя и номер | не обрабатывается, отказ `NotSupported` |
| `InsertIntoPlaybackQueue` | 52 | `CommandInfo.proto:51` | только имя и номер | отказ `NotSupported`, очередь не меняется |
| `ReorderPlaybackQueue` | 55 | `CommandInfo.proto:52` | только имя и номер | отказ `NotSupported` |
| `RemoveFromPlaybackQueue` | 56 | `CommandInfo.proto:53` | только имя и номер | отказ `NotSupported` |
| `PlayItemInPlaybackQueue` | 57 | `CommandInfo.proto:54` | только имя и номер | отказ `NotSupported` |

Номера 1–6 (`Play`, `Pause`, `TogglePlayPause`, `Stop`, `NextTrack`, `PreviousTrack`) и 45
(`SeekToPlaybackPosition`) реализованы в `services/remote/src/mrp_service`. Всё остальное
получает `SendCommandResultMessage.sendError = NotSupported` (10) и не касается состояния.

## Почему команды очереди не включены

В pinned protobuf у `SendCommandMessage` есть только `command`, `options`, `playerPath`
(`SendCommandMessage.proto:12-16`). Сами команды очереди несут параметры в
`CommandOptions`, но захваченных кадров с этими командами нет: неизвестно, приходит ли
идентификатор элемента в `contentItemIdentifier` (поле 1 `CommandOptions`), в
`systemAppPlaybackQueueData` (поле 15) или в отдельном сообщении. Без захвата нельзя
отличить «переставить элемент» от «перейти к элементу», а угадывание по имени enum
запрещено. Поэтому поддерживается безопасный отказ.

`supportsSharedQueue` остаётся `false` в `CommandInfo` и `DeviceInfoMessage`
(`services/remote/src/model.rs`): контроллер не должен считать, что очередь общая.

## Запрос содержимого очереди

`PlaybackQueueRequestMessage` (extension 37) реализован на чтение:
`location` (1), `length` (2), `artworkWidth` (4), `artworkHeight` (5),
`contentItemIdentifiers` (12); ответ ограничен `MAX_ARTWORK_BYTES`. Поля
`queueIdentifier`, `requestId`, `context` в ответе не используются — их семантика
не подтверждена.

## Тесты

`cargo test --offline -p remote --lib queue_mutation` — команды 52/55/56/57 получают
`NotSupported`, идентификатор запроса возвращается, `content_id`/`title`/`playing`
не меняются.
