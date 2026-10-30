# AirPlay 1/2: рабочая спецификация и состояние реализации

Дата: 30 октября 2026. Целевой репозиторий: [CorsairGuy/AirPlay](https://github.com/CorsairGuy/AirPlay). Проверенный commit: [767b1156b7d6a7176aae54bfe03a5e883f6a0320](https://github.com/CorsairGuy/AirPlay/tree/767b1156b7d6a7176aae54bfe03a5e883f6a0320).

Документ подготовлен для размещения в общем репозитории как `docs/PROTOCOL.md`. Это консолидированная спецификация известных публичных частей с границами достоверности, источниками и пробелами. Полной официальной публичной wire-level спецификации AirPlay 2 среди проверенных источников нет; закрытые или непроверенные части здесь не реконструируются догадками. Неофициальная airplay-spec и protobuf-схемы не являются нормативными документами Apple.

## Краткий итог

**Реализованы конкретные пути:** discovery, RTSP/control skeleton, pairing/verification, AP2 audio receive, timing, mirroring/video и мост MRP. У каждого есть ограничения профиля.

**Частично:** полная session lifecycle/flush, матрица кодеков, синхронизация и ограничения ресурсов. Поддержка отдельного успешного потока не означает полное AirPlay 2.

**Ещё предстоят в общем ядре:** полноценный MRP server/card, аудиоотправитель и группы HomePod, classic AP1 audio receive, законченные control CLI и IPC. На Станции есть отдельные наработки MRP/TX, но их наличие не означает завершённый перенос сюда.

Ни одно крупное семейство функций не обозначена «полностью реализована»: для этого нужны фиксированный профиль, независимый interop и проверки lifecycle/ошибок. В этом аудите новые испытания на физических устройствах не проводились.

## Роли и последовательности

- **Receiver:** объявляет возможности, принимает соединение, аутентифицирует peer, согласует transport/codec/timing, получает и воспроизводит media, отдаёт метаданные/состояние и обрабатывает остановку.
- **Sender:** обнаруживает получателей, устанавливает доверие и media sessions, кодирует/передаёт кадры с временными метками, управляет presentation time, завершает их. Sender и controller могут быть разными участниками.
- **Controller/MRP:** подписывается на состояния players/output contexts, отправляет команды и принимает изменения. Само control-соединение не доказывает передачу звука.
- **Group coordinator:** согласует участников, их идентичности, состояние и clock mapping. Общий текст на кнопке и галочки — наблюдаемое представление; они не заменяют transport и timing.
- **Platform adapter:** соединяет протокол с PCM, устройством вывода и источниками; Алиса, BlueZ и подсветка не относятся к AirPlay wire protocol.

Классический audio path: DNS-SD → RTSP `ANNOUNCE`/SDP → `SETUP` → `RECORD` → RTP media/timing/retransmission → `FLUSH`/`TEARDOWN`. Это семейство сообщений, а не обязательный универсальный порядок для AP2. Источники: [Shairport rtsp.c](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/rtsp.c), [RTP](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/rtp.c).

AP2: discovery и `/info` → согласованный pairing/verify профиль → plist `SETUP` для session и stream → соответствующие event/data/audio каналы → timing/anchors и media → rate/flush/teardown. Realtime, buffered и control-only sessions имеют разные требования. Не назначать UDP/TCP или PTP всем сессиям по одному имени AirPlay 2. Источники: [pyatv AP2 session](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/ap2_session.py), [Shairport AP2](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/AIRPLAY2.md).

MRP tunnel: согласовать канал данных, обработать его framing/encryption и protobuf messages, выполнить bootstrap/subscriptions, затем передавать snapshot/delta player/queue/artwork/output state и команды. TCP bridge type 130 покрывает транспорт к отдельному серверу, но не реализует этот сервер. Источник: [pyatv channels](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/channels.py).

Mirroring и URL/HLS playback — отдельные режимы, их нельзя вывести из наличия audio RX. Transport encryption/pairing не означает поддержку DRM-контента или сертификацию MFi.

## Матрица фактического состояния общего ядра

Статусы: **частично** — существенный путь есть, перечислены пробелы; **нет в проверенном entry point** — заглушка/явный отказ; **не подтверждено** — недостаточно свидетельств. Эта таблица оценивает целевой код; следующая карта отдельно оценивает публичную документацию протокола.

Все относительные пути ниже относятся к закреплённому commit общего ядра.

| Возможность | Статус | Свидетельство и граница |
|---|---|---|
| Discovery, capabilities, `/info` | Частично | `crates/device`, `bin/airplay/src/serve.rs`, `session.rs`; нужны единый capability registry и согласованность build/runtime/MRP |
| RTSP/session lifecycle | Частично | `session.rs`, `media.rs`; `SETUP`, `RECORD`, anchors, buffered flush и teardown handlers существуют, но classic `FLUSH` только отвечает 200 без действия |
| Legacy/HAP transient/PIN/verify | Частично | Реализованы pairing пути и interop tests; не доказательство полного HomeKit accessory/MFi |
| Classic AP1 audio receive | Нет полного пути | `session.rs`: тест `unknown_request_is_not_implemented` проверяет `ANNOUNCE` → 501; `media.rs` отмечает отсутствие AP1 non-plist SETUP |
| AP2 realtime/buffered audio receive | Частично | `media.rs`, `crates/audio_stream`; есть decode/playout, но полная codec/rate/channel/source matrix не проверена |
| NTP/PTP-derived clock mapping | Частично | `crates/clock/src/ptp.rs`, audio scheduling; полноценная групповая TX-синхронизация не подтверждена |
| Metadata и commands к источнику | Частично | `bin/airplay/src/remote.rs` и события приёмника; initial artwork/card в целевом сценарии MRP-карточки ещё требует MRP server |
| MRP encrypted data tunnel/bridge | Реализован путь; профиль ограничен | `media.rs`, `--mrp-bridge`; это транспорт, а не готовый mediaremote server |
| MRP server: DeviceInfo/player/queue/artwork/volume | Нет в entry point | `crates/mrp/src/lib.rs`, `services/mediaremote/src/lib.rs` — заглушки. Перенос из Станции намечен Claude |
| AP1/AP2 audio TX | Нет готового CLI/engine | `send` сообщает «ещё не реализовано»; Station `airplay_sender` — отдельный источник переноса |
| Group membership, independent volume, group UI | Не завершено | Нет интегрированного sender + authoritative coordinator. Проверки интерфейса Станции нельзя приписать новому ядру |
| Screen mirroring/video | Частично | `mirror`, `video`, опциональный FFmpeg path; actual device/profile coverage требует отдельной матрицы |
| HLS, Companion, capture, IPC | Нет в проверенных entry points | Соответствующие `src/lib.rs` — заглушки, существование crates не доказывает функцию |
| CLI list/status/send/pair/set/log | Нет в entry point | `bin/airplay/src/main.rs` явно возвращает not implemented |
| Независимая сборка audio без video | Не завершено | `bin/airplay/Cargo.toml`: ряд video/mirror/HLS зависимостей без optional; один ffmpeg feature недостаточен |
| Home enrollment/Handoff/DRM parity | Не подтверждено | Нельзя вывести из pairing, relay или названия FairPlay-модуля |

Проверяемые исходники: [main.rs](https://github.com/CorsairGuy/AirPlay/blob/767b1156b7d6a7176aae54bfe03a5e883f6a0320/bin/airplay/src/main.rs), [session.rs](https://github.com/CorsairGuy/AirPlay/blob/767b1156b7d6a7176aae54bfe03a5e883f6a0320/bin/airplay/src/session.rs), [media.rs](https://github.com/CorsairGuy/AirPlay/blob/767b1156b7d6a7176aae54bfe03a5e883f6a0320/bin/airplay/src/media.rs), [manifest](https://github.com/CorsairGuy/AirPlay/blob/767b1156b7d6a7176aae54bfe03a5e883f6a0320/bin/airplay/Cargo.toml), [MRP](https://github.com/CorsairGuy/AirPlay/blob/767b1156b7d6a7176aae54bfe03a5e883f6a0320/crates/mrp/src/lib.rs).

## Проверки и правила обновления

[CI](https://github.com/CorsairGuy/AirPlay/actions/runs/40932488604) включает Linux/Windows/macOS, отдельную FFmpeg проверку, ARMv7/aarch64 cargo check и pyatv interop pairing. Cross-check не равен запуску на ARMv7 со штатной glibc; fake/null sink не равен корректному ALSA и микрофону; зелёный CI не подтверждает акустическую синхронность HomePod.

Для каждого сообщения постепенно фиксировать wire types, framing, состояние до/после, ack/error/retry, clock domain, лимиты, pinned source и обезличенный fixture. Не выдумывать точные поля/константы там, где имеется только пользовательское наблюдение. При переносе схем сохранять неизвестные поля и numeric flags. Данные pairing и временные подписанные media URL в fixtures не публиковать.

Следующий раздел содержит объединённый source ledger, карту известных частей и критерии полноты. Его `partial` в колонке покрытия относится к публичной спецификации, а не к реализации общего ядра.

## Что можно считать спецификацией

Публичной исчерпывающей официальной wire-level спецификации AirPlay 2 среди проверенных источников нет. [AIRPLAY2.md](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/AIRPLAY2.md) прямо обозначает это ограничение. Название openairplay/airplay-spec означает **неофициальную** реконструкцию, а не документ Apple.

Нужны четыре независимых класса свидетельств:

- **APPLE-PUBLIC** — документы Apple об API, пользовательском поведении, безопасности и лицензируемых программах. Они задают продуктовую семантику, но не раскрывают все сетевые сообщения.
- **RE-CODE** — конкретный исходник OSS на зафиксированном commit; доказывает реализованный путь этого проекта, а не обязательность для всех устройств.
- **EXPERIMENT** — сохранённая обезличенная трасса с версиями sender/receiver, направлением и шагами воспроизведения. Не превращать частный результат в универсальное правило.
- **UNKNOWN** — смысл поля, переход или совместимость не установлены. Существование protobuf-поля не доказывает полноту поведения сервера.

## Зафиксированные первичные источники

SHA получены через GitHub REST GET commits?per_page=1 для default branch, имена веток — через GET repository. Файлы читались с ref=SHA; ниже ссылки закреплены. Это срез проверки, не обещание неизменного HEAD.

| Источник | Ветка и commit | Практическая область |
|---|---|---|
| [postlund/pyatv](https://github.com/postlund/pyatv) | master; [b277a4c8222ecdcbaab8a24e3e713ca44765adb4](https://github.com/postlund/pyatv/commit/b277a4c8222ecdcbaab8a24e3e713ca44765adb4) | Sender/client, pairing, metadata/control, MRP schemas |
| [mikebrady/shairport-sync](https://github.com/mikebrady/shairport-sync) | master; [0a34fbda83724cc06266aadcf7c88acc8fef4e7d](https://github.com/mikebrady/shairport-sync/commit/0a34fbda83724cc06266aadcf7c88acc8fef4e7d) | Receiver audio AP1/AP2, RTSP/RTP, timing integration |
| [mikebrady/nqptp](https://github.com/mikebrady/nqptp) | main; [ab8fb67483411640fd7f1e193889482f2421c37c](https://github.com/mikebrady/nqptp/commit/ab8fb67483411640fd7f1e193889482f2421c37c) | PTP-derived clock tracking; shared-memory timing API |
| [FDH2/UxPlay](https://github.com/FDH2/UxPlay) | master; [534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc](https://github.com/FDH2/UxPlay/commit/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc) | Mirroring/video receiver, rendering, legacy audio |
| [openairplay/airplay-spec](https://github.com/openairplay/airplay-spec) | master; [00063da0e7f16c20fbfb5d29331334da1a89590a](https://github.com/openairplay/airplay-spec/commit/00063da0e7f16c20fbfb5d29331334da1a89590a) | Unofficial readable wire notes; last checked commit July 2020 |
| [openairplay/airplay2-receiver](https://github.com/openairplay/airplay2-receiver) | master; [6c343d3679ddb561c61566985acaaf587d0a3bd3](https://github.com/openairplay/airplay2-receiver/commit/6c343d3679ddb561c61566985acaaf587d0a3bd3) | Experimental Python AP2 receiver; broad negotiation and codec examples |

Дополнительные Apple-public источники:

- [AirPlay deployment: discovery, identification, security](https://support.apple.com/en-in/guide/deployment/dep9151c4ace/1/web/1.0). В том числе изменение идентификации Apple TV с tvOS 18: MAC/deviceID больше не гарантирован в объявлении.
- [Apple: playback destinations, multiroom, stereo pair and Handoff](https://support.apple.com/en-euro/guide/iphone/iph315e0d58d/ios). Пользовательская модель; не wire protocol.
- [Apple: accessories and public noncommercial HAP specification entry](https://developer.apple.com/accessories/).
- [Apple: HomeKit communication security](https://support.apple.com/en-mide/guide/security/sec3a881ccb1/web).
- [Apple MFi FAQ](https://mfi.apple.com/faqs): AirPlay audio входит в лицензируемую экосистему. Наличие публичного HAP документа не делает AirPlay wire protocol публичным.
- [Apple media streaming overview](https://developer.apple.com/documentation/technologyoverviews/streaming): API/application-level вход, не полная спецификация приёмника.

## Карта требований и доступности источников

Здесь **partial** означает неполноту публичной wire-спецификации соответствующей области. Это не оценка CorsairGuy/AirPlay и не утверждение, что конкретная OSS функция не работает.

| Область | Что специфицировать в общем ядре | Первичное доказательство | Покрытие публичной спецификации |
|---|---|---|---|
| Discovery / DNS-SD | Browse/register _airplay._tcp и legacy _raop._tcp; parsing двух 32-bit частей feature mask; стабильные идентичности; TXT updates при смене роли | [Discovery](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/service_discovery.md); [pyatv flags/parser](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/utils.py) | partial: таблицы RE, изменения Apple по версиям |
| Features / negotiation | Разделять advertised capabilities и реально согласованный session profile; неизвестные биты сохранять; не включать buffered/PTP без реализации | [Features](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/features.md); [receiver bitflags](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2/bitflags.py) | partial: есть расхождения имён битов |
| Session RTSP | State machine, content types, request IDs/CSeq, errors, SETUP negotiation, RECORD/FLUSH/TEARDOWN, authenticated reconnect | [RTSP receiver](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/rtsp.c); [AP2 control session](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/ap2_session.py) | partial: несколько разных session families |
| Pairing / crypto | Раздельные legacy, transient HAP, persistent HAP paths; TLV, SRP, verification, key derivation, framed control/event/data encryption, access control | [HAP crypto](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/auth/hap_srp.py); [transient](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/auth/hap_transient.py); [receiver HAP](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/pair_ap/pair_homekit.c) | partial: public HAP foundation, AirPlay-specific adaptation RE |
| Classic audio | ANNOUNCE/SDP + SETUP, RTP sequence/timestamps, loss recovery, encryption profile, ALAC/PCM/AAC capabilities negotiated explicitly | [RTP](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/rtp_streams.md); [receiver RTP](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/rtp.c); [legacy crypto/audio support](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2/airplay1.py) | partial: strong legacy coverage, не все codec/profile combinations |
| AP2 realtime / buffered | Раздельные transport/queue/flush/scheduler paths; codec, bit depth, sample rate, channels; buffer limits; backpressure | [SETUP stream cases](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/rtsp.c); [stream sockets/format selection](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2/connections/stream.py); [current supported formats](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/AIRPLAY2.md) | partial; работающие профили не означают все AP2 варианты |
| Timing / sync | NTP-like legacy clock mapping, PTP-derived AP2 mapping, anchors, drift correction, timestamp rollover, clock loss/master changes | [anchor conversion](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/rtp.c); [PTP handling](https://github.com/mikebrady/nqptp/blob/ab8fb67483411640fd7f1e193889482f2421c37c/nqptp-message-handlers.c); [shared memory ABI](https://github.com/mikebrady/nqptp/blob/ab8fb67483411640fd7f1e193889482f2421c37c/nqptp-shm-structures.h) | partial: NQPTP использует часть PTP, не полноценный PTP clock |
| Video URL/HLS/photos | Отдельный URL playback API, transport, play-state/events, content loading, supported app scope | [video notes](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/video/README.md); [photo notes](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/photos/README.md); [HLS/video receiver](https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/airplay_video.c) | partial; URL video не равно screen mirroring |
| Screen mirroring | Setup/key negotiation; TCP framing; codec configuration; H.264/HEVC NAL units; timestamps/A-V sync; renderer capabilities | [mirroring receive](https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/raop_rtp_mirror.c); [pairing](https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/pairing.c); [supported scope](https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/README.md) | partial; supported codec/resolution зависит от negotiated profile и renderer |
| Metadata / DMAP / DACP | Track/art/progress through SET_PARAMETER; source control tokens; state updates и lifecycle; отдельный экспорт приложениям | [legacy metadata](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/metadata.md); [metadata events](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/METADATA%20CODES.md); [DMAP parsing](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/dmap/parser.py) | partial; legacy stream metadata и DMAP remote-control service различны |
| MRP | Protobuf envelope/extensions, bootstrap/subscription, client/player paths, queues/artwork, events/acks; AP2 tunnel | [data/event framing](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/channels.py); [MRP builders](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/mrp/messages.py); [DeviceInfo schema](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/mrp/protobuf/DeviceInfoMessage.proto) | partial: schema ≠ полный автомат Apple server |
| Volume | Legacy gain dB/mute vs MRP endpoint/member normalized values; clear conversion; availability/capabilities; no echo loops | [legacy volume](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/volume_control.md); [conversion helpers](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/utils.py); [MRP volume messages](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/mrp/messages.py) | partial: диапазон/смысл привязан к API, не единый scalar |
| Groups / multiroom | Real group/session identity, leader/follower, member add/remove/set, discovery updates, per-member control, shared clock; snapshot vs delta | [groupUUID, SETPEERS and session roles](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/rtsp.c); [modify context](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/mrp/protobuf/ModifyOutputContextRequestMessage.proto); [remove output notification](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/mrp/protobuf/RemoveOutputDevicesMessage.proto) | partial: incomplete state semantics and renderer/UI rules |
| Handoff | State/queue/source-ownership transfer; distinguish proximity UX, URL playback, relay and real ownership transfer | [Apple documented requirements](https://support.apple.com/en-euro/guide/iphone/iph315e0d58d/ios) | unknown wire completeness; не считать обычный relay полноценным Handoff |
| HomeKit / Home | Enrollment/access-control/persistent pairing versus becoming a complete HAP accessory; household scope and lifecycle | [Shairport Home enrollment](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/ADDINGTOHOME.md); [Apple HAP entry](https://developer.apple.com/accessories/) | partial; added-to-Home ≠ HomePod feature parity |
| Companion | Отдельный control protocol, pairing, framing, apps/power/HID; optional module | [Companion protocol](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/companion/protocol.py); [limits](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/docs/documentation/supported_features.md) | partial; не требование для любого AirPlay audio receiver |
| DRM / MFi boundary | Отделить transport encryption/authentication от защищённого movie playback и accessory certification | [video DRM limitation](https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/README.md); [FairPlay/MFi limits](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/README.md); [Apple MFi](https://mfi.apple.com/faqs) | missing для универсальной публичной end-to-end DRM/certification specification |

## Уточнения, которые защищают от неверных выводов

1. **AirPlay 2 не означает только AAC.** Зафиксированный Shairport 5.6.2 описывает ALAC24/48 buffered и AAC48 multichannel наряду с basic realtime ALAC16/44.1 и buffered AAC44.1. HD96/192 и Atmos в этом проекте не заявлены поддержанными. Это scope конкретного receiver, не полный перечень форматов Apple. [Source](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/AIRPLAY2.md)

2. **Старые README не годятся для текущей матрицы.** У Shairport 5.6 описаны remote control через MPRIS/D-Bus/MQTT и несколько независимых AP2 instances на host; старые обзоры 4.x могли утверждать обратное. Нужны pinned version, build flags и dependency ABI. [Source](https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/AIRPLAY2.md)

3. **UDP/TCP и timing — отдельные оси.** Legacy SETUP в неофициальных notes описывает три UDP канала; AP2 receiver code отдельно выбирает TCP для buffered media. pyatv control-only AP2 session запрашивает timingProtocol=None: удачный MRP tunnel не доказывает audio/timing implementation. [Legacy SETUP](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/rtsp_requests/setup.md); [AP2 sockets](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/ap2/connections/stream.py); [Control-only](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/ap2_session.py)

4. **Имена feature bits противоречат друг другу.** Неофициальная таблица называет bit38 SupportsCoreUtilsPairingAndEncryption, bit48 SupportsTransientPairing; pyatv называет bit38 SupportsUnifiedMediaControl, bit48 SupportsCoreUtilsPairingAndEncryption. Bit32 тоже имеет двойное/неуверенное имя. Хранить numeric mask + source alias + проверенное поведение. Не объявлять одно название каноническим. [Table](https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/features.md); [Code](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/utils.py)

5. **Экспериментальный AP2 receiver полезен как readable reference, но не как гарантия синхронизации.** Его README заявляет realtime/buffered, несколько codec paths и pairing, но явно исключает accurate NTP/PTP sync, FairPlay v2 и MFi authentication. Собственный claim «all codecs» требует проверки по согласованному профилю. [Source](https://github.com/openairplay/airplay2-receiver/blob/6c343d3679ddb561c61566985acaaf587d0a3bd3/README.md)

6. **UxPlay — специализированный mirroring/video источник.** Он не реализует AP2 multiroom audio. HLS поддержка в README ограничена проверенным YouTube iOS path; Apple video DRM не поддержан. Перенос mirroring кода не покрывает HomePod groups или protected video. [Source](https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/README.md)

7. **MRP schema не является спецификацией UI и не подтверждает серверную реализацию.** ModifyOutputContext имеет type SharedAudioPresentation=1, add/remove/set списки в полях2..7; отдельный RemoveOutputDevices имеет outputDeviceUIDs=1 и optional endpointUID=2, extension70. Но порядок конфликтующих операций, snapshot/delta contract и точное использование endpointUID нужно подтверждать trace+tests. [Request schema](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/mrp/protobuf/ModifyOutputContextRequestMessage.proto); [Notification schema](https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/mrp/protobuf/RemoveOutputDevicesMessage.proto)

8. **Группа комнат, stereo cluster и Handoff — разные сущности.** Apple описывает stereo pair как один playback destination, а multiroom как несколько выбираемых destinations. Нельзя автоматически приравнивать physical speaker count, logicalDeviceCount и configuredClusterSize. Предыдущие локальные обезличенные эксперименты могут дополнять evidence ledger только с пометкой конкретной версии/сценария; здесь они не заменяют source coverage. [Apple behavior](https://support.apple.com/en-euro/guide/iphone/iph315e0d58d/ios)

## Как ставить fully / partial / missing в CorsairGuy/AirPlay

Оценка применяется к **одной фиксированной функции/роли/профилю**, а не ко всему слову AirPlay.

- **fully**: реализованы обязательные переходы оговорённого профиля; есть позитивные, негативные, reconnect/teardown и ресурсные тесты; capability advertising соответствует поведению; известна проверенная sender/OS/receiver matrix. Наличие файла или compilation alone недостаточно.
- **partial**: рабочий subset, stub/assumed semantics, непроверенные transitions, ограничения транспорта/формата/платформы, либо нет подтверждения interoperability. Явно перечислить оставшееся.
- **missing**: проверкой кода установлено отсутствие нужного пути, есть явный unsupported/stub или тестом подтверждено отсутствие поведения.
- **unknown**: код/тесты ещё не проверены или evidence не позволяет вывод. Не переименовывать unknown в missing; до аудита целевого репозитория применять именно unknown.

Шаблон одной строки evidence ledger:

| ID | Role/profile + requirement | Status | Source class + permalink | Target commit/file/symbol | Test/trace | Limit/gap |
|---|---|---|---|---|---|---|
| DISC-001 | Receiver: _airplay feature-mask low/high parsing | unknown | RE-CODE, pinned source | pending actual audit | golden TXT + malformed mask + unknown bits | sender matrix pending |
| GROUP-001 | Sender/controller: removing last member terminates relay and updates observers | unknown | RE-CODE schema + EXPERIMENT needed | pending | add/remove/set, repeated/conflicting UIDs, independent observers | order/scope semantics unproven |

## Структура единой достоверной спецификации

Для каждой subsystem хранить рядом: terminology/roles; message schema + wire types; state transition table; success/error/retry semantics; timing and resource bounds; identity/privacy rules; source permalink; known disagreements; reproducible golden fixtures; implementation status. Никогда не хранить pairing secrets или signed media URLs в публичных fixtures.

Предлагаемое разделение модулей общего проекта:

- discovery/identity/capability registry;
- RTSP and session orchestration;
- auth/crypto profiles;
- media transport and codec negotiation;
- clocks/synchronization;
- playback/rendering abstractions;
- metadata and MRP/DMAP/DACP adapters;
- group coordinator with authoritative membership state;
- optional mirroring/HLS, HomeKit and Companion modules;
- platform/device adapters вне протокольного ядра.

Это архитектурный вывод из разделения ответственности источников, а не доказательство готовности их комбинации. Перед переносом кода требуется сохранить authorship/license provenance каждого файла; licenses разных деревьев не следует выводить из одного repository badge.



