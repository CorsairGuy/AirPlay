# Зеркалирование вместе с AirPlay 2: исследование

30 октября 2026. Задание: последняя секция `CODEX-BRIEF-2026-10-30.md`.

## Результат

Найдена конкретная альтернативная реализация современного зеркалирования: **doubletake умеет выбирать HAP-encrypted control + PTP и отправлять video110 вместе с realtime audio96**. В этом пути ключи и шифрование отличаются от старого UxPlay/AES/NTP пути, который сейчас реализован для зеркалирования в нашем ядре.

Поэтому задача шире подбора feature mask. Наиболее обоснованное направление — изучить и реализовать соответствующий modern receiver path, а не требовать `ekey` старого формата от любого iPhone.

Это вывод по исходникам отправителя, **не результат запуска нашего приёмника с iPhone**. Комбинация video110 + buffered audio103 + PTP + multiroom не подтверждена. Не доказана и необходимость закрытого Apple-only FairPlay для этого сочетания.

## Границы выполненной работы

- Прочитан brief. Три независимых исследования: receiver, sender, Apple UX/эксперимент.
- Через Pi выполнены только Bonjour browse и один `GET /info` Apple TV. На Pi скрипт подавался через stdin; файлов и сервисов не создавалось.
- Прямой TCP-доступ ПК к Apple TV завершился timeout; успешное чтение выполнено через Pi в той же LAN.
- На Станции ничего не запускалось и не изменялось. Репозитории не редактировались, git fetch/commit/push не выполнялись. Браузер не использовался.
- Музыка, pairing, SETUP/RECORD и изменение громкости не запускались. Диагностические профили пока **не запускались**.
- На iPhone отправлен запрос пользовательской проверки. Результат ещё не получен; отсутствие ответа не считается проверкой или согласием.

К таблице «Все открытые линии работы» не переходили: для этого ожидается сообщение Кирилла, что Claude остановился. Перед каждой такой линией требуется fetch обоих репозиториев и повторная проверка веток/PR. Установка и публикация требуют отдельного согласия согласно brief.

## 1. Что действительно снято у Apple TV

Наблюдение `GET /info`: 30.10.2026, 10:57:30 по Екатеринбургу (05:57:30 UTC). Response `RTSP/1.0 200 OK`, binary plist, 1396 байт. Bonjour получен непосредственно перед ним.

| Поле | Bonjour | `/info` |
|---|---|---|
| model | `AppleTV14,1` | `AppleTV14,1` |
| source version | `960.13.1` | `960.13.1` |
| protocol version | `1.1` | `1.1` |
| features | `0x4A7FDFD5,0x3C177FDE` | `0x3C177FDE4A7FDFD5` |
| flags/statusFlags | `0x10644` | `67140` = `0x10644` |
| igl / gcgl | `1` / `1` | В этом ответе верхнеуровневых полей нет |
| vv | `1` | `1` |
| OS build | — | `23L773` |
| hasUDPMirroringSupport | — | `true` |
| canRecordScreenStream | — | `false` |
| supportedFormats | — | Отдельные `screenStream`, `audioStream`, `bufferStream`, `lowLatencyAudioStream` |
| featuresEx | — | Base64 `1d9/St5/Fzw4oY58` |

У `featuresEx` 12 декодированных байт: первые 8 при little-endian чтении совпадают с обычным `features`, смысл оставшихся четырёх не установлен. Не назначать им названия битов без источника. `hasUDPMirroringSupport=true` — объявленная возможность; фактический транспорт этого iPhone ещё не наблюдался.

`supportedAudioFormatsExtended.bufferStream` присутствует. Это перечень capabilities, не доказательство активного buffered audio при зеркалировании. Значения форматов сохранены в локальном обезличенном JSON; их соответствие кодекам здесь не угадывается.

Измеренная Apple TV одновременно объявляет bit7 (screen), bit40 (buffered audio), bit41 (PTP), bit46 и bit48. Это подтверждает **сосуществование объявлений**. Одинаковое объявление ещё не доказывает одновременное использование возможностей в одной сессии.

Полные TXT/plist с постоянными идентификаторами и публичными ключами оставлены только в приватных локальных evidence-файлах; в этот документ они не включены.

## 2. Почему исходная гипотеза о маске слишком узкая

Сравниваем конкретные версии:

| Реализация | Mask | Наблюдаемая граница |
|---|---|---|
| Наше ядро, AP2 audio | `0x1C340405D4A00` | AP2 audio profile |
| Наше ядро, mirror | `0x527FFEE6` | High 32 bits нулевые; нет40/41/46/48 |
| Текущий UxPlay | `0x5A7FFEE6` | Тоже high=0, но bit27 включён |
| Apple TV Кирилла | `0x3C177FDE4A7FDFD5` | Объявлены screen + AP2 audio/timing capabilities |

В UxPlay `0x527FFEE6` оставлена в комментарии, а активно используется `0x5A7FFEE6`. Разница — bit27 legacy pairing. Его обработчик умеет смешивать расшифрованный AES seed с legacy pair-verify ECDH secret. Значит, правило «bit27 всегда нужно выключить» не универсально. [UxPlay discovery](https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/dnssdint.h#L32), [key derivation](https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/raop_handlers.h#L850).

В shairplay-rust выбор video/AP2 частично закреплён собственными build profiles. Исследовательский статус проекта не доказывает запрет со стороны любого iPhone. [Feature selection](https://github.com/metaneutrons/shairplay-rust/blob/2fb72b30b658b33f106165a5b6fbd941919bdbe5/src/net/features.rs#L152), [AP2 status](https://github.com/metaneutrons/shairplay-rust/blob/2fb72b30b658b33f106165a5b6fbd941919bdbe5/AP2-STATUS.md).

Копировать mask Apple TV целиком нельзя как инженерный shortcut: она содержит возможности, которые наш приёмник ещё не реализует. Успешная видимость с такой маской не будет означать корректную поддержку этих функций.

## 3. Главное найденное различие: modern mirroring

Первичный источник: [omarroth/doubletake](https://github.com/omarroth/doubletake/tree/5e2f1cbe3e1070bcefec8e549c7baf884727bc00), commit `5e2f1cbe3e1070bcefec8e549c7baf884727bc00`. Это reverse-engineered sender, не нормативная спецификация Apple.

### Выбор пути

В `compatibility.go` encrypted HAP control выбирает descriptor-only placement FairPlay material. PTP выбирается при encrypted control, feature41 и подходящей `sourceVersion` (порог `354.54.6`, с отдельным исключением). Это правило конкретного sender. `960.13.1` наблюдаемой Apple TV проходит его version gate; HAP-соединение с ней в этом исследовании не устанавливалось. [Policy](https://github.com/omarroth/doubletake/blob/5e2f1cbe3e1070bcefec8e549c7baf884727bc00/internal/airplay/compatibility.go#L58).

### Ключи и потоки

| Участок | Старый путь | Modern path в doubletake |
|---|---|---|
| Корень session SETUP | Wrapped `ekey` + `eiv` | Descriptor-only policy, старый root `ekey` не обязателен |
| Video descriptor | type110, streamConnectionID | type110 с `shk`/`shiv` в descriptor |
| Шифрование видео | AES-CTR, SHA-512 derivation | ChaCha20-Poly1305; HKDF из HAP pair-verify shared secret, с connection-specific salt |
| Аудио экрана | Realtime96 с legacy AES | Realtime96 с отдельным 32-byte stream key; в соответствующей ветке streamConnections |
| Время | NTP | PTP при выполнении policy gate |
| Buffered103 + multiroom | Не следует из mirror96 | Этим кодом/наблюдением не подтверждено |

Для video HKDF используются `DataStream-Salt<videoID>` и `DataStream-Output-Encryption-Key`. **Нельзя считать video `shk` непосредственно готовым ChaCha key:** в современном пути основой служит shared secret, с предусмотренным sender fallback. Аудио `shk` обрабатывается иначе. [Video SETUP](https://github.com/omarroth/doubletake/blob/5e2f1cbe3e1070bcefec8e549c7baf884727bc00/internal/airplay/mirror.go#L1047), [video crypto selection](https://github.com/omarroth/doubletake/blob/5e2f1cbe3e1070bcefec8e549c7baf884727bc00/internal/airplay/mirror.go#L1167), [audio descriptors](https://github.com/omarroth/doubletake/blob/5e2f1cbe3e1070bcefec8e549c7baf884727bc00/internal/airplay/mirror.go#L1240).

`parsakhaz/open-airplay` использует порт этой логики; это дополнительный пример интеграции, а не второе независимое подтверждение протокола. Заявление README о работающем Apple TV sender не заменяет наш опыт с iPhone→новый receiver. [open-airplay](https://github.com/parsakhaz/open-airplay/tree/1e879a982df78b630a005fcee3ce278a56b3181d).

### Граница собственного ядра

Проверен свежий public main `327bd2236241b1234ede20394da91bce3dce6e9b` после PR11; репозиторий читался через GitHub, локальные ветки не менялись.

- `setup_session` уже распознаёт PTP и NTP.
- Но `setup_video` требует ранее расшифрованный 72-byte root `ekey`, затем использует legacy AES derivation.
- `time_source` для зеркалирования возвращает только NTP clock либо `NoTime`, а не PTP mapping.
- Type96 без `shk` при включённом mirror уходит в AAC-ELD/AES-CBC путь. Наличие AP2 audio ветки96/103 само по себе не соединяет её с modern video.

Таким образом, рекламировать PTP и screen недостаточно: даже после успешного выбора профиля отсутствуют нужные mirroring crypto/timeline paths. [Current core media.rs](https://github.com/CorsairGuy/AirPlay/blob/327bd2236241b1234ede20394da91bce3dce6e9b/bin/airplay/src/media.rs#L285), [device masks](https://github.com/CorsairGuy/AirPlay/blob/327bd2236241b1234ede20394da91bce3dce6e9b/crates/device/src/lib.rs#L33).

## 4. Что означает «как у Apple TV»

Нужно разделять четыре результата:

1. Устройство **объявляет** audio AP2 и screen одновременно — у Apple TV Кирилла подтверждено.
2. Зеркалирование работает с современными pairing/control/crypto и PTP, audio96 — найден исполняемый sender path; у нашего receiver не проверено.
3. В одной presentation/session одновременно идут video110 и **buffered103** с PTP — пока неизвестно.
4. Apple TV принимает зеркалирование и отдельно отправляет звук на HomePod — отдельная downstream роль; она не доказывает пункт3.

Apple manuals описывают screen mirroring и выбор HomePod/AirPlay speakers для звука Apple TV, но не раскрывают тип upstream stream103. [Apple TV audio destinations](https://support.apple.com/guide/tv/homepod-airplay-enabled-speakers-home-theater-atvb66f63f15/27/tvos/27), [Screen Mirroring](https://support.apple.com/en-us/102661). Home Theater/default HomePod output и обычная группа AirPlay speakers — разные условия опыта, их нельзя подменять друг другом.

Задержка realtime screen audio и буферизованной музыки имеет разные требования. Отсутствие103 при зеркалировании не следует автоматически считать ошибкой, пока не установлен реальный целевой сценарий.

## 5. Что можно наблюдать без подмены

- Bonjour и доступный `/info` дают capabilities, но не активный codec/key/timing mode.
- После HAP pair-verify control может быть зашифрован. Обычный pcap не раскрывает SETUP/fp-setup только потому, что известен порт7000. Длины пакетов не заменяют чтение сообщений.
- Pi на обычном switch port может не видеть unicast iPhone↔Apple TV; promiscuous mode не создаёт SPAN. Peer-to-peer/AWDL может проходить вне LAN capture.
- Для полноценного passive flow capture нужна подходящая существующая AP/SPAN точка. Она всё равно не расшифровывает HAP control.
- Диагностический receiver может журналировать поля **своей** сессии до/после шифрования. Это не наблюдение внутренней сессии Apple TV.

В текущем исследовании decoded iPhone→Apple TV SETUP, `/fp-setup`, type/timing и media не сняты. Причина не заменяется утверждением «Apple-only».

## 6. Матрица следующих проверок

Все строки ниже имеют статус **NOT RUN**. Это план, не журнал успешных опытов.

### Сначала настоящий Apple TV

| ID | Изменение | Записать |
|---|---|---|
| A0 | Текущий idle baseline, без смены выходов | Версии iOS/tvOS, текущая audio route; Bonjour и `/info` уже получены |
| A1 | Пользователь включает только Screen Mirroring | Есть ли изображение; доступен ли выбор audio output; объявления до/во время/после |
| A2 | От A1 пользователь выбирает HomePod, только после согласования | Продолжается ли видео, откуда идёт звук; отдельный AppleTV→HomePod leg при доступном наблюдении |
| A3 | Остановить только mirroring | Какие media/routes прекращаются или остаются |

Не включать музыку автоматически. HomePod не повышать выше пользовательских33%; в исследовании громкость не менялась. Для wire-level вывода о103 нужны decoded session evidence, одного UI недостаточно.

### Затем отдельный диагностический receiver на Pi

Предварительное условие: endpoint умеет нужные HAP/crypto/timing handlers либо честно отмечает их отсутствие. В текущем ядре modern mirror path отсутствует; ложное объявление поддержки не считается положительным опытом. В этом исследовании endpoint не создавался.

| ID | База / единственный исследуемый delta | Цель |
|---|---|---|
| R0 | Подтверждённый legacy mirror baseline | Контроль работы старого пути и iPhone |
| R1 | От R0 только bit27, с корректным paired/unpaired derivation | Проверить влияние legacy pairing, не путать с ошибкой ключа |
| R2 | От R0 только bit41 | Наблюдать запрос времени и ошибки; не утверждать PTP работает без clock path |
| R3 | От R0 только bit40 | Наблюдать влияние buffered capability |
| R4 | От R2 добавить только bit40 | Проверить interaction40/41 |
| R5 | От заранее работающего HAP baseline добавить только screen bit7 | Наблюдать современный mirror negotiation и расположение ключевых полей |
| R6 | От соответствующей базы менять по одному `srcvers`, затем model | Проверить policy gates; не менять их одновременно с mask |
| R7 | От modern mirror baseline исследовать extended descriptors | `featuresEx`, `hasUDPMirroringSupport`, streamConnections — отдельные вопросы, не единый пакет предположений |

У каждого профиля свои `pi`/`deviceid`/`gid`, как требует brief; не копировать identity Apple TV и не занимать порты активных служб. Ограничить lifetime, включить null sinks и лимиты RAM/log/connection count. Отдельная identity меняет pairing/cache — это дополнительная переменная. Поэтому каждый профиль повторяет свою baseline-процедуру, auth state фиксируется, а выводы по одному отрицательному запуску не считаются причинно доказанными. Для более строгого A/B потребуется отдельно согласовать метод контроля identity/cache.

Журналировать только названия/типы/длины key fields, не значения ключей, shared secret или pairing credentials. Отмечать requested, authenticated, negotiated, decoded и timing-validated раздельно. Приём video110 без ошибок ещё не доказывает звук103/PTP/multiroom.

## Рекомендация треду ядра

1. Не менять текущие рабочие audio/mirror profiles в production.
2. Использовать doubletake modern sender как протокольную подсказку для отдельного receiver experiment: HAP secret lifetime, descriptor keys, stream-specific derivation, frame framing, PTP TimeSource и audio96 encryption.
3. Проверять iPhone policy только на диагностическом endpoint; sender→receiver loopback полезен для code correctness, но не воспроизводит выбор режима iOS.
4. Только после работающего modern110+96+PTP исследовать buffered103 и group routing как отдельные возможности.
5. Если определённая комбинация не заработает, фиксировать конкретную стадию/ошибку. Не выводить необходимость Apple-only сертификата из отсутствия legacy `ekey` или нескольких неудачных masks.

**Рабочая объединённая маска не найдена; невозможность не доказана. Найден более конкретный технический пробел и источник реализации для следующего контролируемого опыта.**
