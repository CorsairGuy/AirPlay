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
