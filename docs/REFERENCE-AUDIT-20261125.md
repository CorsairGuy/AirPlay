# Сверка эталонов — 25 ноября 2026

Проверены GitHub metadata, релизы и указанные ниже первоисточники. Это обзор
источников, а не доказательство совместимости нашего ядра. Коммиты — срезы
default branch на момент проверки. Лицензия репозитория не заменяет проверки
отдельных файлов и provenance; чужой код не переносится.

## Дополнительный поиск: новые проекты и свежие коммиты

Два отдельных запроса GitHub: проекты с `airplay`, созданные с 1 августа
2026; коммиты с `airplay` за 18–25 ноября и история активных эталонов.
Результаты ранжируются по применимости; README и заявления авторов сами по себе
не являются нашей аппаратной проверкой. Поиск не гарантирует полноту.

### Сверка дополнительного обзора

- [OpenAir](https://github.com/Antoinenz/OpenAir) заявляет RX-проверку на
  Apple TV и Shairport Sync, но HomePod в его README отмечен как непроверенный.
  Лицензия текущего репозитория — GPL-3.0, не заявленная в обзоре MIT.
- [iMirror](https://github.com/prat3ik/iMirror/blob/main/NOTICE.md) описывает
  проблему двух одновременных операций регистрации Android `NsdManager` и
  сериализацию как своё исправление. Это наблюдение об API Android, не wire-факт
  AirPlay и не доказанная проблема всех версий Android.
- [Shairport 5.5.1](https://github.com/mikebrady/shairport-sync/releases/tag/5.5.1)
  перечисляет некорректный `timingPeerInfo.Addresses` и короткий `shk` среди
  security fixes; в обзоре они ошибочно приписаны 5.6. Наш RX в `media/rx.rs`
  пропускает нестроковые адреса через `filter_map`, а ключ `shk` проверяет
  преобразованием в `[u8; 32]`. Это статическое наблюдение, не fuzz-аудит.
- Группа `SRP` в `crates/pair/src/hap.rs` выбирается через `rfc5054_3072()`
  в обеих ролях. Это проверка выбора группы в коде, не подтверждение interop.
- [airplay2-rs](https://github.com/lmcgartland/airplay2-rs) описывает yield
  PTP (3 Sync и 2 Announce) и sync-пакеты PT=87 как собственный сценарий TX.
  Не распространять конкретные значения на все устройства без захвата.
  Документ об именах флагов содержит неоднозначности вокруг bit 26/30, а правило
  `51 без 26` не считать доказанным для всех комбинаций и классов устройств.
  В проверенном TV `0x527FFEE6,0x8300` оба бита 26 и 51 выключены.
- [esp32-airplay2-surround](https://github.com/Kristian8606/esp32-airplay2-surround)
  описывает один AirPlay gateway и собственную доставку к динамикам. Это не
  доказательство AirPlay-группы с числом PTP peers выше восьми.
- [AirBridge](https://github.com/alcxandros/AirBridge) — Windows-оболочка над
  UxPlay/GStreamer; автор описывает отключение анонса аудио для сохранения
  звука на iPhone и отдельный WebRTC обратный канал. Не считать этот канал
  AirPlay TX; поведение профиля проверить отдельно на собственной технике.
- `0x527FFEE6` как младшее слово встречается в [shairplay-rust](https://docs.rs/crate/shairplay/latest/source/AP2-STATUS.md),
  но общий playfair/UxPlay provenance не делает реализации независимыми
  подтверждениями wire-семантики или совместимости AP2 audio + video.

### Сверка следующих обзоров

- [CVE-2026-14324](https://bugzilla.redhat.com/show_bug.cgi?id=CVE-2026-14324):
  в PipeWire RAOP недоверенный `Content-Length` в ответе приводил к ошибке
  выделения памяти и падению. Наш sender использует `rtsp::decode_response`,
  общий `MAX_BODY` и ограничение входного буфера. Добавлен регрессионный тест
  ответов с `MAX_BODY + 1` и `usize::MAX` без присланного тела; это не полный
  fuzz-аудит и не доказательство наличия CVE в нашем коде.
- [AirCiller](https://github.com/carlosciller/AirCiller) заявляет отправку
  локального видео с Mac на Apple TV, без перекодирования совместимого видео;
  лицензия по README GPL-3.0. Степень самостоятельности протокольного стека и
  совместимость с нашим приёмником отдельно не проверены.
- [CastBay](https://github.com/weenas/castbay) использует UxPlay, ExoPlayer и
  MediaCodec; заявляет проверку iPhone на TCL, Sony и BYD. Его замечание про
  Mac Music относится к используемому им срезу. [UxPlay PR #639](https://github.com/FDH2/UxPlay/pull/639)
  позднее добавил поддержку FairPlay v2 в master; нельзя распространять старое
  ограничение на всю текущую линию UxPlay. Наш v2 всё ещё не реализован.
- [Wormhole Display](https://github.com/pgodlews/wormhole-display) заявляет
  выбор extended display на Mac с приёмником Meta Portal. Это кандидат для
  проверки геометрии/объявлений; результат на нашем ядре не подтверждён и не
  доказывает, что одних дескрипторов всегда достаточно.
- [obs-airplay-plugin](https://github.com/dgraid/obs-airplay-plugin) заявляет
  два источника и сосуществование с системным приёмником macOS; долгие циклы
  и soak авторы оставляют непроверенными. Это не подтверждение реализации
  нескольких профилей внутри нашего процесса.
- [esp32-iphone-agent](https://github.com/onehumanbeing/esp32-iphone-agent)
  передаёт H.264 с ESP32 в iPhone-приложение по **Wi-Fi TCP**, не по BLE;
  BLE используется для команд и статуса. Это отдельная граница приёмника и
  декодера, не доказательство работы нашего TX.
- [Shairport Sync 5.6](https://github.com/mikebrady/shairport-sync/releases/tag/5.6)
  сам указывает, что при работающих AP2-командах два поля доступности пока
  остаются false. Это не измерение задержки нашего ТВ-профиля.

**Высокий приоритет, обновление существующего эталона:** UxPlay 17 ноября
слил [поддержку FairPlay v2](https://github.com/FDH2/UxPlay/pull/639),
которую авторы связывают с macOS Music. Наш приёмник сейчас явно отклоняет
`request[4] != 0x03` в `crates/fairplay/src/sap.rs`: v2 не поддержан.
Это конкретный кандидат для проверки входящего macOS Music и протокольных
векторов, а не доказательство работоспособности Apple Music DRM. Версия
протокола v2 не равна mode 2 внутри v3. UxPlay — GPL-3.0; код не заимствовать.

**Свежее по синхронизации:** [airplay-cli #123](https://github.com/music-assistant/airplay-cli/pull/123)
описывает срыв синхронизации группы на хосте с сильным отклонением частоты
часов; корректировку применяют между сессиями. Это сценарий для испытания
нашего собственного clock engine на drift и перестройку эпох, не доказательство
его текущего дефекта. [Коммит #127](https://github.com/music-assistant/airplay-cli/commit/bb43374db7084ce3afc7ac7863601d4ac573b1f6)
касается расчёта старта AP2 с NTP; сравнить его контракт в тестах релея.
Чужой код и daemon NQPTP в ядро не переносить.

**Новые кандидаты:**

- [tvpipe](https://github.com/mitsuhiko/tvpipe), создан 18 ноября: Rust TX
  зеркалирования, общий encode на несколько получателей, документация
  архитектуры и измерений. README заявляет Apache-2.0 в целом, но FairPlay
  crate — LGPL-3.0 и происходит от doubletake; это не независимый референс
  FairPlay. Наш RX и TX им ещё не испытаны.
- [AudioSync](https://github.com/yassersutojoyo/audiosync), создан 16 октября:
  beta с заявленным собственным Rust AirPlay sender и локальной аудиозадержкой;
  GPL-3.0. Исследовать подход к изменению запаса буфера и синхронизации без
  предположения, что он пригоден для экранного видео или проверен на нашей сети.
- [airplay-tv](https://github.com/R3G1ST/airplay-tv), создан 25 ноября:
  Kotlin Android TV RX с заявленными mirroring, transient pairing и аудио;
  GPL-3.0, проект очень новый. Заявления о возможностях, происхождение
  реализации и устойчивость не проверены на устройствах.
- [Android TV AirPlay Receiver](https://github.com/exgota/android-tv-airplay-receiver):
  Java RX для старых Android/Fire TV, README отдельно перечисляет проверенные
  устройства и ограничения. GPL-3.0; может помочь составить матрицу профилей,
  независимость реализации и результаты требуют проверки.
- [esp32-airplay2-surround](https://github.com/Kristian8606/esp32-airplay2-surround)
  уже присутствовал в плане, но недавно обновлялся. Авторы описывают gateway,
  PTP и относительные Wi-Fi TSF-измерения и отдельно оговаривают, что они не
  измеряли акустическое смещение относительно HomePod. Референс методики,
  не подтверждение совместимости нашего PTP.

| Источник | Проверенный срез / релиз | Значение для проекта |
|---|---|---|
| [Shairport Sync](https://github.com/mikebrady/shairport-sync/releases/tag/5.6.3) | `7ce90d8`, 5.6.3 | AP2 remote с 5.6; 5.6.3 — исправления mDNS, метаданных, старта и гонки MPRIS. Реальный reverse-remote сверять по протоколу и классу устройства. |
| [UxPlay](https://github.com/FDH2/UxPlay) | `fbd040e`, стабильный релиз 1.73.7 | HLS и BLE-discovery; `master` указывает experimental 1.74. Не смешивать релиз и рабочую ветку; GPL-3.0. |
| [pyatv](https://github.com/postlund/pyatv/releases/tag/v0.18.0) | `b277a4c`, 0.18.0 | Новейший отмеченный GitHub релиз датирован 19 июня; наличие более новых обсуждений или коммитов не означает новый релиз. |
| [airplay-cli](https://github.com/music-assistant/airplay-cli/releases/tag/v0.5.17) | `ca23976`, 0.5.17, 25 ноября (Екатеринбург) | Исправлен старт AP2-колонок с NTP и начальные метаданные Bose. Приоритетная проверка тайминга и сценариев релея; GPL-3.0. |
| [OwnTone](https://github.com/owntone/owntone-server/releases/tag/29.4) | `e2a3915`, 29.4, 22 ноября | Исправления HomePod OS 27, MediaRemote и pause/resume; утверждение, что новее 29.3 изменений нет, устарело. GPL-2.0. |
| [doubletake](https://github.com/omarroth/doubletake/releases/tag/v0.6.0) | `be27776`, 0.6.0 | TX зеркалирования, адаптация bitrate/FPS и shared capture; поведение на наших устройствах не проверено. LGPL-3.0. |
| [shairplay-rust](https://github.com/metaneutrons/shairplay-rust/releases/tag/v0.13.0) | `34ff8ad`, 0.13.0 | Версия в присланном обзоре устарела; источник заявляет AP1/AP2 и mirroring. Провенанс отдельных файлов/тестов проверить отдельно. LGPL-3.0. |
| [Android AirPlay Server](https://github.com/jqssun/android-airplay-server) | `a46cca0`, 0.0.36 | Android TV, MediaCodec и HLS поверх UxPlay; не независимый протокольный стек. GPL-3.0. |
| [OpenDisplay](https://github.com/peetzweg/opendisplay/releases/tag/v1.30.3) | `7f61aeb`, 1.30.3 | Sender и receiver как два приложения; идеи виртуального дисплея и видеотракта, не подтверждение нативного AirPlay-режима. GPL-3.0. |
| [goplay2](https://github.com/openairplay/goplay2) | `eb779df`, последний push 21 апреля 2023 | Старый RX-референс; GitHub `archived=false`. Нельзя называть архивированным только из-за отсутствия активности. Apache-2.0. |

Дополнительно сверены metadata GitHub: [airplay2-receiver](https://github.com/openairplay/airplay2-receiver),
[doubletake-m2-compat](https://github.com/Edowndotdown/doubletake-m2-compat),
[fairplay-sap-core](https://github.com/objevovat/fairplay-sap-core-airplay2-sender-authentication-handshake),
[airplay2-rs](https://github.com/lmcgartland/airplay2-rs),
[berryaudio](https://github.com/berry-audio/berryaudio),
[zenlink](https://github.com/zenvyzenshiro/zenlink),
[iPhoneMirror](https://github.com/RayrenSX/iPhoneMirror),
[MoonCast](https://github.com/meta-tabchen/MoonCast),
[AirPlayServer](https://github.com/xenos1337/AirPlayServer),
[atv-core](https://github.com/corvofeng/atv-core),
[airplay-hub](https://github.com/napthemax/airplay-hub),
[airfry](https://github.com/nored/airfry),
[airplay2-sender-cpp](https://github.com/akustikrausch/airplay2-sender-cpp),
[pyatv-rs](https://github.com/SkrOYC/pyatv-rs),
[iDescriptor/shairplay-rust](https://github.com/iDescriptor/shairplay-rust). Их глубина
поддержки и все изменения с прошлого пина отдельно не подтверждались.
GitHub не определил SPDX-лицензию atv-core и airfry автоматически; это не
доказывает ни наличие, ни отсутствие лицензии. `openairplay/airplay2-receiver-go`
не удалось подтвердить по предполагаемому адресу; требуется найти первоисточник.

### Новые пины 25 ноября 2026 (отдельный поиск GitHub API)

Срезы сняты `git ls-remote` с default branch; лицензия — metadata GitHub. Заявления
README не являются нашей проверкой; код не переносится. Статусы задач эти записи не меняют.

| Источник | Срез | Лицензия | Значение и ограничения |
|---|---|---|---|
| [serezhka/java-airplay](https://github.com/serezhka/java-airplay) | `61f6be7` | MIT | AP2-RX для десктопа: mirroring, звук, HLS через GStreamer/FFmpeg/Chromium-MSE; соседний `java-airplay-lib` (MIT, последний push 2022) позиционируется как сервер «в роли Apple TV». Дополнительный AP2-RX для сравнения: сверка stream-SETUP и HLS-ветки там, где прежние эталоны только GPL (UxPlay, shairport-sync). Заявления README о поддержке устройств не проверены. |
| [Mihonarium/StreamToSpeaker](https://github.com/Mihonarium/StreamToSpeaker) | `3edd815` | MPL-2.0 | Rust, Windows: виртуальное аудиоустройство → AirPlay/Sonos/UPnP. Единственный найденный открытый Windows-sender; полезен для темпа звука в TX ; к приёму зеркалирования напрямую не относится. Совместимость с нашим TX не проверена. |
| [kushuh/airpipe](https://github.com/kushuh/airpipe) | `0a2f521` | Apache-2.0 | Rust, PipeWire → AP2, sender-only, терминальный. Малый разрешительный кросс-чек AP2-TX рядом с OwnTone (GPL-2.0, C): выбор профиля и старт сессии. Создан в октябре 2026, поведение на наших приёмниках не проверено. |
| [nickustinov/itsytv-core](https://github.com/nickustinov/itsytv-core) | `0a09f06` | не определена GitHub | Swift: Companion + MRP + AirPlay. Третья независимая реализация MRP/Companion для §6 п.4 (подписи pair-setup/pair-verify) и п.13 (схема) после pyatv, `pyatv-rs` (D41) и `station-relay`. Создан 14 ноября 2026; лицензия неизвестна — только чтение wire-фактов. |

Не покрыто этим проходом: DACP/DAAP-remote, fuzz-корпуса сетевых парсеров и Rust-реализации
gPTP — GitHub Search API ответил `rate limit exceeded` после примерно 15 запросов с одного IP.

## Исправления и задачи

1. [pyatv #2866](https://github.com/postlund/pyatv/issues/2866): на конкретном
   Apple TV с tvOS 27 beta отсутствие PIN связывали с отсутствующим заголовком
   `X-Apple-Client-Name`; комментаторы сообщили об успешном pairing после его
   добавления. Наш `services/media/src/tx` пока не содержит такого заголовка.
   Проверить контракт и добавить тест с реальным устройством; не объявлять
   Companion единственным возможным путём и не обобщать бета-наблюдение.
2. [fairplay-sap-core](https://github.com/objevovat/fairplay-sap-core-airplay2-sender-authentication-handshake)
   описывает реализацию SAP **стороны отправителя** с заявленной поддержкой mode 3.
   Это отдельная задача от приёмника и DRM контента; hardware coverage и
   лицензии компонентов требуют дополнительной проверки.
3. Проверить в собственных сценариях приёмника/релея: NTP start deadline,
   pause/resume, peer loss, многокомнатный дрейф, стартовые метаданные,
   актуальные форматы MRP и обработку malformed input. Из релизов не следует,
   что эти возможности реализованы или сломаны у нас.
4. В следующих аппаратных прогонах фиксировать версию iOS/tvOS, приложение,
   модель, commit ядра, профиль и измерение от выбора до первого кадра/звука.
   Пересматривать закреплённый эталон при изменении его активного для нас пути;
   security advisories и issues проверять по первоисточнику.
5. Список в плане частично использует неполные ссылки `github (...)` и внешние
   описания. Идентификация и ревизия всех таких источников ещё не завершена;
   не переносить неподтверждённые утверждения в статус проверенных.
