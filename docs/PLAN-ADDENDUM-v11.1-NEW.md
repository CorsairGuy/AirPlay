# PLAN-ADDENDUM-v11.1-NEW.md

Маркер: current-status-20261113
Новый накопитель (чистый лист), ведётся параллельно плану AGENT-PLAN-AP12-20261112 v11.1.
Правило внесения: по §0.5 v11.1 — provenance-тег одной строкой; код с нуля; Apple-код — никогда.
Ссылки на директивы плана: L-xx → соответствующие D/S/OBS плана v11.1.

ВАЖНО ДЛЯ АГЕНТА: все накопленные патч-напоминания применяются при сборке НОВОЙ версии плана — AGENT-PLAN-AP12-20261112 **v12**.
Не выпускать очередную редакцию 11.x. Целевая версия сборки: v12 (см. патч-напоминание 17).

---

## Реестр (сводно)

| Запись | Тема | Статус переноса |
|---|---|---|
| L-52/D95 | atv-core: серверный MRP/Companion-референс | NEW |
| L-53 | shairplay v0.6.2: type96/103, <50 мс, BS.775, 0x527FFEE6 | NEW (обновление D23) |
| L-54/D96 | DeetsAirplay: protocol.md, floor 250 мс, входящий UDP | NEW |
| L-55/D97 | BeamCast: AP1 TX, свой mDNS без Bonjour → D35 | NEW |
| L-56/D98 | BLE-discovery AirPlay: формат рекламы, IPv4-only, AWDL | NEW |
| L-57 | rust-atv fork (PapeThePope) — действующая ветка D41 | NEW (обновление D41) |
| L-58/D99 | esp32-airplay2-surround: Gateway-модель на ESP32 | NEW |
| L-59 | napthemax/airplay-hub: кросс-протокольный мультирум | NEW |
| L-60 | airplay-esp32: лицензия Non-Commercial (наследуется форками) | NEW (обновление D39) |
| L-61 | AppleTVr: полный Companion Link с нуля (SRP/Ed25519/OPACK) | NEW |
| L-62 | airplay2-sender-cpp: полный wire-рецепт AP2-realtime | NEW (обновление D19) |
| L-63 | openairplay airplay-spec: дерево решений пикера | NEW |
| L-64 | SRP-6a: group 3072, SHA-512, PIN 3939; MFi при битах 26/51 | NEW (обновление D30) |
| L-65 | streamsemble: 3 подтверждения (96/103 bit-clean, PTP 248/247) | NEW (обновление D29) |
| L-66 | doubletake: reverse handshake (3 UDP + 1 TCP); LLM-генерация | NEW (обновление D10) |
| L-67 | pyatv #2908: pair-pin-start таймаут на tvOS/audioOS 26.6 | NEW |
| L-68 | pyatv #2893: /feedback heartbeat ломает RTSP-сессию (tvOS 26.5) | NEW |
| L-69 | owntone #1413: полные mDNS-TXT дампы стереопар (4 состояния) | NEW |
| L-70 | UxPlay: отключение feature bit 27 → коннект без задержки | NEW |
| L-71 | pyatv-rs RISKS.md: R7 подтверждает фазы 3–5; MRP volume idle | NEW |
| L-72 | owntone SPEAKER_GROUPING: не слита с 2022 — потолок группировки | NEW |
| L-73 | HomePod 27.2: квад-система как один аксессуар (факты) | NEW |
| L-74 | Сигналы 27.2 для плана; квад-захват deferred (нет железа) | NEW |
| L-75 | shairport-sync 5.6.2: живой AP2-референс; RTP-timestamp overflow | NEW (обновление §1.1) |
| L-76 | nqptp: SHM-спецификация; эксклюзив UDP 319/320 — риск ARCHITECTURE | NEW |
| L-77 | doubletake-m2-compat: подтверждение reverse handshake; SAP snapshot | NEW |
| L-79 | Music Assistant airplay-cli: AP2 RAOP-compat профиль, −30..0 дБ | NEW |
| L-80 | TVRCSessionStart (Companion, tvOS 26) + SystemStatus патч | NEW |
| L-81 | shairplay: issue ~10 с задержки старта; OBS «нет команд в iPhone» | NEW (обновление D23) |
| L-82 | airplay-cli: --protocol auto по TXT; конвенция громкости | NEW (обновление L-79/D60) |
| L-83 | Инструменты приёмки: soundcheck, signal-estimator, bit-exact паттерны | NEW |
| L-84 | Полная таблица TXT openairplay: rsf/hgid/hmid/pgcgl; два 32-бит features | NEW |
| L-85 | owntone #1774: owntone только NTP; shairport-sync AP2 только PTP | NEW |
| L-86 | owntone #1413: семантика pgid/gid, дерево высоты 2, транзиентные группы | NEW |
| L-87 | Официальные SRP-вектора Apple (AIS §5.5.2, 3072-bit) | NEW |
| L-88 | Полные HKP-деривации (openairplay §9.1): control/tv/MFi | NEW |
| L-89 | iakacer AIRPLAY_2_SPEC: transient-флоу, X-Apple-HKP: 4, ChaCha20 24B trailer | NEW |
| L-90 | UxPlay BLE-спецификация: 14-байт beacon, генератор, AWDL у ATV4K | NEW |
| L-91 | PoPETs 2020: BLE AirPlay приватность (публичный MAC, IPv4 cleartext) | NEW |
| L-92 | pyatv-rs RISKS M7: AirPlay pair-setup без PIN на современных tvOS | NEW |
| L-93 | airplay-esp32: DACP/MRP-разделение; BT → AirPlay автоприостанавлива | NEW |
| L-94 | AWDL-развилка + hotspot-профиль (уличный сценарий; USB-gadget исключён решением пользователя) | NEW |
| L-95 | AWDL-наработки: OWL, nexmon, OpenDrop, замеры; AirPlay-over-AWDL отсутствует | NEW |
| L-96 | Wi-Fi Aware (NAN) в iOS 26: стандартизированный P2P — будущий путь | NEW |
| L-97 | DMA-открытие: обязательства Apple перед ЕК, AWDL→NAN-конвергенция, NAN-железо приёмника | NEW (обновление L-96) |
| L-98 | Airdrop-on-Windows как OBS «протокол ≠ транспорт» (AirDrop поверх Wi-Fi Direct) | NEW |
| L-100 | AWDL AirPlay: статус мира + план HomePod-имитации (UxPlay -p2p, EZCast, nexmon RPi 4B) | NEW (обновление L-95) |

---

## L-52 / D95 — atv-core: серверный MRP/Companion-референс

corvofeng/atv-core (MIT): устройство, притворяющееся Apple TV, принимает нативный пульт iPhone из Пункта управления без джейлбрейка. Рекламирует _mediaremotetv._tcp + _companion-link._tcp; MRP 49152, Companion 49153; SRP-пейринг с фиксированным PIN 1111; Companion стримит дельта-координаты тачпада (аналог опционального трека D13/D41/D42). Web-инспектор показывает живой поток SRP-хендшейков и MRP-каналов. Второй независимый «серверный» референс MRP после rust-atv (D41), работающий с нативным пультом iPhone (pyatv так не умеет).
Provenance: MIT заявлен в README. Привязка: фаза 6, Companion opt.

## L-53 — shairplay v0.6.2 (обновление D23)

shairplay-rust стал крейсом на crates.io (0.6.2) с форками. Новые wire-факты:
- раздельная реализация type 103 (buffered AAC по TCP, музыка) и type 96 (realtime ALAC по UDP, Siri/системные звуки) — опора для D87;
- старт подключений <50 мс за счёт детерминированного персистентного ключа и обработки eventPort на isRemoteControlOnly-каналах — ответ на сценарии быстрого реконнекта §5;
- мультиканал 5.1/7.1 с даунмиксом ITU-R BS.775;
- video-фича переключает приёмник на legacy features 0x527FFEE6 (UxPlay-совместимый, iOS 18+), AP2-buffered осознанно выключен — значение битовой маски для D80/D86;
- конфигурируемая AP1-реклама (cn=0,1; et=0; RSA/FairPlay-опции) — прецедент честной рекламы возможностей.
Форк iDescriptor/shairplay-rust заявляет LGPL-3.0-or-later. Кросс-чек с L-81 (задержка старта ~10 с — другой путь).

## L-54 / D96 — DeetsAirplay

deets-137/DeetsAirplay: самописный AP2-отправитель на Rust (Windows → HomePod). Главное — docs/protocol.md: байтовый рецепт пейринга, деривации ключей, SETUP-плайстов, layout'ов пакетов и таймингов с gotchas. Факты: минимальная задержка realtime-потока 250 мс (выбор отправителя), приёмник шлёт timing-запросы ОБРАТНО отправителю → отправителю нужен входящий UDP (в Windows добавляли firewall-правило) — ловушка фазы 3.
Provenance: лицензия неизвестна (?).

## L-55 / D97 — BeamCast

priidiu/BeamCast (MIT): RAOP-трансмиттер для Windows 11: WASAPI loopback → ALAC (порт энкодера Apple, Apache-2.0) → RTP. Свой mDNS/DNS-SD без Bonjour — прецедент для D35 на стороне отправителя. Проверен на WiiM Mini и HomePod как приёмниках. Кросс-чек фазы 3 рядом с D19/D33.

## L-56 / D98 — BLE-discovery AirPlay

UxPlay wiki: формат Apple BLE-рекламы для discovery AirPlay — manufacturer code 0x004C, continuity type 0x09, флаги, seed, IPv4-адрес и порт в 14-байтной рекламе; Apple TV 4K дополнительно рекламирует AWDL peer-to-peer. В плане обнаружение описано только через mDNS (D35) — BLE это отдельный непокрытый dimension. Для Pi-релея необязательно; OBS в WIRE-SPEC (как iPhone находит приёмники «близости», почему только IPv4) для D84.

## L-57 — rust-atv fork (обновление D41)

PapeThePope/rust-atv — поддерживаемый форк pyatv-rs (Rust 2024, tvOS 27 в примерах), разбитый на крейсы pyatv-core/mdns/pairing/opack/proto-mrp/proto-companion. Актуальная альтернатива замороженному оригиналу.

## L-58 / D99 — esp32-airplay2-surround: Gateway-модель

Kristian8606/esp32-airplay2-surround (Apache-2.0, с оговоркой «FairPlay tables excepted»): Apple TV видит ОДИН приёмник, Gateway форвардит сырой multichannel-поток до 8 спикерам по Wi-Fi; спикеры в пикере не видны. Ровно hub-модель §1.4 в эмбеддеде, с рабочей 7.1-синхронизацией. Усиление §1.4/D12/D60.

## L-59 — napthemax/airplay-hub

napthemax/airplay-hub (MIT): кросс-протокольный мультирум на Linux: AP1 через PipeWire loopback+delay, AP2 через OwnTone, контроль «hold back the faster path». Методика согласования двух протоколов решена практически — референс для будущего мультирума релея.

## L-60 — airplay-esp32: лицензия (обновление D39)

Лицензия airplay-esp32 — Non-Commercial, наследуется форками («производная не может дать права, которых не дал исходник»). Идеи и OBS свободны, код не портировать.

## L-61 — AppleTVr: Companion Link с нуля

LaurentVoillot/AppleTVr (MIT, Swift): полный Companion Link — SRP-6a 3072-bit + Ed25519, ChaCha20-Poly1305 с 12-байтным LE nonce, framing 4-byte header + OPACK, команды _hidC/_mcc. Третий независимый OPACK-референс рядом с D42.

## L-62 — airplay2-sender-cpp: полный wire-рецепт AP2-realtime (обновление D19)

Репо из D19 (Apache-2.0 C++20 sender) опубликовал полный рецепт AP2-realtime:
- ChaCha20-Poly1305 framing: [2-byte LE len][cipher][16-byte tag], chunk ≤ 1024 B, AAD = length prefix, nonce = [4 zero][8-byte LE counter], раздельные send/recv счётчики ключей;
- SETUP — это RTSP-метод rtsp://host/sessionId, НЕ POST /setup (404); перед ним обязательный GET /info;
- event channel — это keep-alive: без него приёмник рвёт сессию ~25–30 с после RECORD; event-ключи = HKDF Events-Salt / Events-Write|Read-Encryption-Key поверх pair-verify-секрета, свапнутые (reverse connection);
- audio key = первые 32 байта pairing shared secret, raw, без HKDF (подтверждает D19: key_len 64, шифрование аудио 32);
- ALAC обязателен на realtime-потоке, приёмник hardcode'ит и игнорирует ct/audioFormat;
- POST /feedback — только RTSP/1.0, не HTTP/1.1; 200 OK минимальный.
Автор ведёт ежедневную работу против Apple TV 4K (AppleTV14,1) — живой «серебряный» источник фазы 3.

## L-63 — Дерево решений пикера

openairplay/airplay-spec (service_discovery):
- AppleTV = model has prefix AppleTV; HomePod = prefix AudioAccessory; ThirdPartySpeaker = биты 26 (HasUnifiedAdvertiserInfo) ИЛИ 51 (SupportsUnifiedPairSetupAndMFi); иначе Unknown;
- CanBeRemoteControlled = SupportsBufferedAudio и НЕ PINRequired;
- md-биты метаданных: 0 text, 1 artwork, 2 progress, 50 bplist.
Прямая опора для D80/D84 и захода «выглядеть телевизором в пикере» (L-45). Дополнительные опоры: L-69, L-84, L-90.

## L-64 — SRP-6a параметры (обновление D30)

SRP-6a: group 3072-bit (не 2048!), SHA-512, PIN 3939 в эталоне; MFi /auth-setup обязателен при битах 26/51; FairPlay v3 подтверждён в openairplay Python.

## L-65 — streamsemble: три подтверждения (обновление D29)

realtime 96 bit-clean на 176.4 KB/s; buffered 103 loopback 111.8 с bit-clean; PtpReceiverClock 248/247.

## L-66 — doubletake: reverse handshake (обновление D10)

Отправитель открывает 3 последовательных UDP (timing/control/data) + 1 TCP и объявляет их в SETUP; без reverse handshake приёмник молчит. Provenance-факт: автор заявляет, что код в основном сгенерирован LLM — репо использовать только для идей, сверяя с D19 и UxPlay.

## L-67 — pair-pin-start таймаут на современном стеке

pyatv #2908 (08.2026): POST /pair-pin-start (HAP transient) таймаутит на tvOS 26.6 / audioOS 26.6 → «Failed to set up remote control channel». User-Agent стека: AirTunes/960.13.1. Пейринг-поверхность современного стека меняется — S-сценарий для §5/§6.

## L-68 — /feedback heartbeat ломает сессию

pyatv #2893: RTSP-сессия рассинхронизируется во время /feedback heartbeat на tvOS 26.5 — keep-alive не только держит сессию, но и может ломать её. В эталонный захват: «feedback под нагрузкой». Рядом: play_url → HTTP 500 на /playback-info (tvOS 26); падение multicast-discovery при живом unicast.

## L-69 — mDNS-TXT дампы стереопар (золото для D58/D80)

owntone issue #1413: реальные дампы TXT-записей стереопар HomePod (audioOS 15.4, AudioAccessory1,1) в четырёх состояниях (idle/пара/группа/лидер):
- составной gid у лидера группы: gid = <UUID лидера>+0+<UUID сессии>; у участников — простой UUID (подтверждает «формат не предполагать» в D58);
- fex содержит канальный десигнатор: AMp/StBLNbw (левый) vs AMp/StBrNbw (правый) — fex не монолит;
- tsid/tsm — поля стерео-механики, только у Apple-приёмников; tsid лидера совпадает с UUID в составном gid;
- разница features-слов: AudioAccessory1,1 0xBC354BD0 vs AudioAccessory5,1 0xBC356BD0 — ровно 0x2000 (бит 45): старшая модель поддерживает то, чего нет у младшей (кандидат stereo-pair-фича);
- Sonos Bookshelf как «вендор без Apple-стека»: pi в MAC-форме вместо UUID, manufacturer/serialNumber/fv.
Сверка 0xBC354BD0 с живым HomePod 1 возможна прямо у пользователя (два HomePod 1 в хозяйстве).

## L-70 — UxPlay: быстрый коннект без pair-verify

UxPlay: отключение feature bit 27 убирает pairing-шаг → подключение без 5-секундной задержки. Компромисс «быстрый коннект ↔ shared secret» для D80.

## L-71 — pyatv-rs RISKS: подтверждение стратегии

pyatv-rs RISKS.md: AirPlay 2 audio-stream SETUP таймаутит на tvOS 27 и в pyatv, и в pyatv-rs — их вывод совпадает с нашим: «исправлять по свидетельству успешного Apple-отправителя, а не ещё одного byte-for-byte порта» → прямое обоснование фаз 3–5 (собственный sender + эталонный захват). MRP volume недоступен в idle, появляется только при групповом воспроизведении — уточнение к D60/ползункам.

## L-72 — owntone SPEAKER_GROUPING: стеклянный потолок

owntone жив (v29.3, 07.2026), но ветка SPEAKER_GROUPING не слита в main с 2022. Третье независимое подтверждение, что sender-side группировка приёмников — потолок; hub-модель §1.4 остаётся правильной. Отправитель ведёт N независимых сессий; приёмники не объединяются со стороны приёмника.

## L-73 — HomePod Software 27.2: квад-система (факты)

HomePod Software 27.2 + tvOS 27.2 (обе требуются) добавили домашний кинотеатр из четырёх HomePod — только с Apple TV 4K gen4: две стереопары, спереди и сзади:
- каждая пара однородна (две mini 2 или два HomePod 2); допустимы: 4 mini 2, 4 HomePod 2, пара полноразмерных спереди + пара mini 2 сзади; mini 2 с HomePod 2 — только тылы; оригинальный HomePod mini исключён;
- настройка на самой Apple TV; после группировки система из четырёх колонок и приставки — ОДИН аксессуар: один ползунок, один выход в пикере;
- два режима: Surround Sound (Spatialize Stereo, регулятор Surround Level −8…+8 дБ, шаг 0,5 дБ; выкл. → обычное стерео на фронт, тылы получают пустой микс) и Whole Room Audio (diffuse-рендеринг; каналы по диагонали: фронт-лево+тыл-право играют левый, фронт-право+тыл-лево — правый);
- находки кода (pdfu): тюнинги quad_frontleft/frontright/rearleft/rearright; оценщик временных задержек и карта 4×4 позиций; семейство Forklift = 2×AID12001 + 2×AID12007; AID12007 = HomePod mini 2 (неизвестное железо октябрьской беты); удалён surround DSP-граф HomePod 2 (AID12001/HAL/speaker_surround.dspg); helpers подрезания импульсных откликов для калибровки;
- Siri proxy-activation path: HomePod в Home Theater-группе передаёт Siri-запросы на Apple TV отдельным путём (старый микрофонный релей остаётся);
- Sound Glow для старых моделей (проверка LightShowCapability, не поддержан только HomePod 1);
- ограничения: без центра/сабвуфера (диалоги только из фронтальной пары), без passthrough и DTS.
Статус: продуктовые факты + находки кода; wire-уровень НЕ подтверждён — не переносить в WIRE-SPEC до захвата.

## L-74 — Сигналы 27.2 для плана

1. Hub-модель §1.4 получила нативный Apple-прецедент: Apple TV gen4 ведёт четыре колонки как один аксессуар с одним ползунком — паттерн «мастер-хаб + N динамиков» теперь официальная поверхность Apple (усиливает D12/D60).
2. Квад-система — следующий уровень иерархии группировки: группа из двух стереопар с ролями (front/rear) поверх. Wire-уровень (новые поля? новый gid-формат? флаги?) неизвестен — кейс эталонного захвата GROUP-BUTTON-EXPERIMENTS.
3. КВАД-ЗАХВАТ НЕ ИСПОЛНИМ НА НАШЕМ ПАРКЕ (решение пользователя 2026-11-13): только два HomePod 1 (AudioAccessory1,1); фича требует HomePod 2/mini 2 + Apple TV 4K gen4; оригинальный HomePod mini исключён самим Apple — предел железа, не ПО. Статус: deferred, «наблюдаемо только у владельцев», пересмотр при появлении совместимого железа.
4. Исполнимый эталон группировки — стереопарный захват из L-69 (дампы сняты с AudioAccessory1,1 — совпадает с нашим парком; сверка 0xBC354BD0 возможна).
5. Siri proxy path — OBS в карточке MRP (фаза 6): у «группы с мастером» появляется канал мастер↔участник.
6. Surround Level как ползунок −8…+8 дБ с шагом 0,5 дБ — уточнение D60: групповые регуляторы бывают dB-шкальными с фиксированным квантованием.
7. «Mac audio via AirPlay — responsive and persistent» в 27.2 — Apple усиливает persistent-сессии sender-side → подтверждение персистентного stdin в D57-архитектуре.

## L-75 — shairport-sync 5.6.2: живой AP2-референс (обновление §1.1)

Релиз 10.2026 «Security Update and Long-Playing Enhancement». Матрица AIRPLAY2.md шире, чем «AP1-база» в §1.1: работает realtime ALAC/S16/44100/2, buffered AAC/F24/44100 и 48000, lossless ALAC/S24/48000 buffered, surround 5.1/7.1 buffered, транскодирование/миксдаун, добавление в Home app; не работает 96/192 кГц, Atmos, Windows iTunes. Свежие коммиты: 32-битный RTP-таймстемп на PTP-коннекциях переполняется на длинных сессиях (расширение до 64 бит — тест-кейс фазы 4); hardening DACP-парсера против малформированных TLV (прецедент D34/D38-fuzz). Предлагается скорректировать §1.1: shairport-sync — база AP1 И живой AP2-референс.

## L-76 — nqptp: SHM-спецификация и риск UDP 319/320

README nqptp: полная спецификация SHM-интерфейса — двойная запись с __sync_synchronize(), обязательное сравнение main/secondary копий при чтении, команды по порту 9000. Референс паттерна OBS-29 «демон на хосте + shm». РИСК: nqptp требует эксклюзивный доступ к UDP 319/320 и не уживается с другим PTP-демоном или полным PTP-сервисом. Запрет ARCHITECTURE «одна общая PTP-служба» и стыковка со shairport-sync-приёмниками сталкиваются на этих портах — вопрос владения 319/320 явно решить в ARCHITECTURE/SECURITY до фаз 2–4.

## L-77 — doubletake-m2-compat

Форк doubletake (07.2026, релицензирован LGPL-3.0): независимо подтверждает reverse handshake («Apple TV подключается обратно к трём UDP и одному TCP; до завершения reverse handshake приёмник молчит, SETUP не возвращает»); флаг -port-range для confinement под firewall. Тестируемые приёмники включают HomePod 1st gen — реальный кейс нашего релея как цели. FairPlay SAP реализован как «snapshot-backed Go ARM64 execution» — эмуляционный приём: по L-50 только OBS-наблюдение, воспроизведение в репозиторий не попадает.

## L-79 — Music Assistant airplay-cli

music-assistant/airplay-cli — единый бинарь RAOP+AP2 (нативный HAP-флоу) с persistent PCM-входом (подтверждение D57). Их собственная AP2-реализация не умеет password pairing и PTP timing — gap даже у популярного проекта.

## L-80 — Companion на tvOS 26

Indigo-плагин: PR «Send TVRCSessionStart during Companion protocol connect» — новый шаг хендшейка Companion, без которого power state и push ломаются на tvOS 26; runtime-патч ha-pyatv-tvos26-patch от поломки Companion SystemStatus. Вывод для фазы 6: в эталонный захват добавить проверку TVRCSessionStart и SystemStatus.

## L-81 — shairplay: задержка старта и границы (обновление D23)

Крейс shairplay (metaneutrons = Fabian Schmieder): известный issue ~10 с задержки между выбором цели и стартом аудио (контрапункт к «<50 мс» из L-53 — реконнект и стартовая настройка AP2 это разные пути; важно для D61); OBS «сторонние AP2-приёмники не могут слать playback-команды в iPhone».

## L-82 — airplay-cli: профили и громкость (обновление L-79/D60)

--protocol auto выбирает маршрут по mDNS TXT (алгоритм для D80); новый профиль «AP2 RAOP-compat» (auth-setup + RAOP-флоу для приёмников без pair-verify) → SESSION_PROFILES; конвенция громкости linear-in-dB −30…0 дБ → дополнение к D60.

## L-83 — Инструменты приёмки фазы 4

- soundcheck (gak4u): sample-accurate latency через FFT-кросс-корреляцию лог-синус-свипа (20,8 мкс на 48 кГц); дрейф через линейную регрессию по пингам ~0,1 ppm при 30+ пингах за 60 с — разрешение, требуемое приёмкой «≥2 часа» (D9).
- signal-estimator (gavv): latency_corr / latency_step / losses / io_jitter / io_delay — измеряет джиттер операций планирования ALSA; отдельный срез приёмки D9.
- spdif-bit-exactness-tools (jwhitham): бит-точные паттерны — 24 сэмпла с бегущим битом + маркер 0x654321; методика «инвертировать паттерн и суммировать с записью → полная тишина»; паттерн не выживает при ресемплинге/скейлинге/дизеринге — маркер нетронутости пути «приём → вывод».
Все три — внешние инструменты: по §0.5 код не копируем, методики забираем в VALIDATION.

## L-84 — Полная таблица TXT openairplay (обновление D80/D84)

Дополнения к таблице TXT-полей: rsf (required sender features), hgid (home group UUID), hmid (household ID), pgcgl. Важно: features кодируется ДВУМЯ 32-битными значениями через запятую (второй опционален) — правило парсинга для D80.

## L-85 — owntone #1774: ловушка совместимости timing

OwnTone не может использовать AP2 с shairport-sync: OwnTone поддерживает только NTP timing, а shairport-sync в AP2-режиме — только PTP. Следствия: наш shairport-наследованный приёмник будет PTP-only → собственный отправитель обязан реализовывать PTP; owntone — референс только AP1 fan-out; тест «owntone → наш AP2-приёмник» невозможен в принципе — не тратить время.

## L-86 — owntone #1413: семантика группировки

Алгоритм от автора ветки: pgid имеет приоритет над gid; дерево групп максимальной высоты 2 (стереопара со своим gid → родительская группа, pgid = тот же UUID); пикерные группы транзиентны, стереопары — персистентны и именованы. При воспроизведении в участников пикерная группа «магически растворяется» — живое подтверждение «стеклянного потолка» из L-72. Ветка собирается с -DPREFER_AIRPLAY2 -DSPEAKER_GROUPING; реализует PIN auth + volume control для групп — референс D58/D59.

## L-87 — Официальные SRP-вектора Apple

Accessory Interface Specification (Apple, 2019), §5.5.2 «SRP Test Vectors»: полная таблица тест-векторов SRP-6a для 3072-битной группы RFC 5054 — modulus N, generator g = 0x05, все промежуточные значения (a, A, b, B, salt, verifier v, scrambling u, premaster S, session key K) для тестовой пары. Канонический набор для проверки SRP-реализации в pair-setup — сверка против публикации Apple, а не против чужих реализаций (закрывает типовой источник ошибок: нестандартные HomeKit-деривации x/M1/M2). Статус: числа из публичной спецификации для производителей аксессуаров; вектора в репозиторий допустимы.

## L-88 — Полные HKP-деривации

openairplay airplay-spec §9.1 (pairing/hkp): точные HKDF-параметры всех ключей:
- control-канал: Salt Control-Salt; Info Control-Write-Encryption-Key / Control-Read-Encryption-Key; nonce = 64-битный счётчик в младших разрядах 96-битного (старшие 32 бита = 0);
- transient pairing: EncryptionKey = SRP shared secret напрямую, pair-verify пропускается;
- MFi при бите 51: в M4 добавляется TLV 0x05 с ключом Pair-Setup-Encrypt-Salt / nonce PS-Msg04; внутри MFi-сигнатура (RSA-1024 + SHA-1) под ключом MFi-Pair-Setup-Salt.
Все деривации теперь специфицированы построчно (дополнение к D80).

## L-89 — iakacer AIRPLAY_2_SPEC: transient-флоу

Детализация: заголовок X-Apple-HKP: 4; метод 4 = Transient HomeKit; AP2-audio RTP = ChaCha20-Poly1305 с 24-байтным trailer; TLV-типы 0x00/0x01/0x02. Кросс-чек WIRE-SPEC.

## L-90 — UxPlay BLE-спецификация

Полный разбор BLE-рекламы AirPlay: 14-байтовый beacon 0D FF 4C 00 09 08 13 30 …; два варианта (ATV3 — фиксированный порт 7000; ATV4K — порт в payload + второй ADU для AWDL); поведение seed (инкремент при каждом обновлении); ADV_NONCONN_IND с интервалом 100 мс; ограничение «только IPv4»; рабочий генератор uxplay-beacon.py. Bluetooth LE beacon page UxPlay wiki.

## L-91 — BLE AirPlay и приватность

PoPETs 2020 (privacy analysis): AirPlay-BLE использует публичный MAC и передаёт IPv4 цели в cleartext — каждое касание пикера раскрывает факт стриминга. Если добавляем BLE-обнаружение (D98) — осознанно выбираем, что раскрываем. SECURITY.md-материал.

## L-92 — AirPlay pair-setup без PIN на современных tvOS

pyatv-rs RISKS.md (M7): на современных tvOS AirPlay pair-setup не показывает PIN — Companion единственный путь пейринга; AirPlay-туннель затем аутентифицируется теми же HAP-креденшелами. Для PIN-пути из §1.3: проверку на iPhone вести с оглядкой — на современных прошивках AirPlay-HKP-PIN может быть недоступен в принципе. Интероп-деталь: pyatv-rs zero-pads DMAP pairing GUID, pyatv — нет (кросс-чек D62).

## L-93 — airplay-esp32: DACP/MRP и автоприостанавлива

airplay-esp32 независимо подтверждает DACP/MRP-разницу (DACP только в AP1, в AP2 — MRP) и даёт прецедент поведения «BT подключился → AirPlay автоприостанавливается» — полезно для роутера источников в фазе 7.

## L-94 — Уличный сценарий: AWDL-развилка + hotspot-профиль

(Решение пользователя 2026-11-13: внести AWDL-развилку и hotspot-профиль; USB-gadget НЕ вносить.)

Транспортная развилка «AirPlay без Wi-Fi»:
- AirPlay не требует интернета, но требует IP-сети; «нулевого» транспорта не бывает (TCP/UDP поверх IP + mDNS + PTP в AP2).
- Peer-to-Peer AirPlay («без Wi-Fi» в рекламе Apple) работает только внутри экосистемы Apple: iPhone находит Apple TV/HomePod по BLE-маячку и поднимает прямую связь по AWDL. AWDL закрыт для сторонних устройств; колонка не от Apple участвовать не может. Открытая Linux-реализация AWDL существует (OWL, см. L-95) — исследовательская, но чтобы iPhone выбрал её как приёмник AirPlay, нужно ещё притворяться Apple-устройством в пикере (незакрытый кейс). Практически для улицы — не путь.
- Вывод: уличный сценарий = свой приёмник (Pi) + своя IP-сеть; домашние HomePod не участвуют.

Hotspot-профиль (основной путь):
- Pi в режиме точки доступа (hostapd/NetworkManager); iPhone подключается к сети Pi — видит приёмник через mDNS внутри одного L2-сегмента; интернет iPhone берёт с сотовой сети.
- Паттерн autohotspot: Pi подключается к домашнему Wi-Fi, когда он есть, и переключается в AP, когда нет (raspberryconnect / raspberrypi.stackexchange — рефересы).
- Для улицы: 2,4 ГГц (дальность), стационарное питание или powerbank.
- Deployment-кейс для D16: «Pi без сети → автоподъём AP → приёмник виден» — исполнимый сценарий с критерием приёмки.
- Связь с L-90: у Apple-приёмников есть BLE-канал с IPv4 в payload именно для P2P-сценариев без IP-сети; у стороннего приёмника такого канала нет — релей в «уличном» профиле всегда опирается на mDNS внутри созданной им же сети.

## L-95 — AWDL-наработки на GitHub и замеры

Готового AirPlay-over-AWDL приёмника НЕ существует. Наработки и опыты:

- OWL / Open Wireless Link (seemoo-lab/owl, C, GPL-3): открытая реализация AWDL для Linux/macOS. Userspace, Netlink API, создаёт виртуальный интерфейс awdl0 (link-local IPv6), пиров добавляет в neighbor table — существующие IPv6-программы работают без модификаций. Требует Wi-Fi-карту с active monitor mode + frame injection (рекомендация: Atheros AR9280, 802.11n); без ACK к принимаемым кадрам отправитель ретранслирует каждый кадр до 7 раз → деградация пропускной способности. Статичная election-метрика; channel sequence фиксирован при старте; НЕ допускает одновременного подключения к AP — радиоинтерфейс монопольно занят AWDL. Каналы: 6, 44, 149. Проект НЕ сопровождается (последние правки ~2019); экспериментальный. Референс-доки: owlink.org; Wireshark-диссектор встроен в Wireshark 3.0+.
- nexmon (seemoo-lab): пропатченная прошивка Broadcom/Cypress — monitor mode + frame injection на встроенном Wi-Fi Raspberry Pi (brcmfmac, CYW43430/43455): Pi 3, 3+, 4, 5, Zero W, Zero 2 W; injection зависит от чипа. Kali 2025.3 вернул пакеты brcmfmac-nexmon-dkms + firmware-nexmon (DKMS, выживает обновления ядра). Т.е. OWL на Pi возможен без внешнего донгла, но с оговорками по injection.
- OpenDrop (seemoo-lab/opendrop, Python, GPL-3): открытая реализация AirDrop поверх OWL — доказательство, что Apple-устройство принимает Linux-AWDL-пира (AirDrop работает end-to-end: свежий форк на MediaTek MT7921 отправляет И принимает с iPhone iOS 26 без Apple ID). Практическая цена: установка подключения ~10 с против ~2 с у нативного AWDL (по опыту habr-эксперимента: OWL на Pi Zero + Kali + BLE-реклама py-bluetooth-utils/apple bleee).
- Научная база: MobiCom '18 «One Billion Apples' Secret Sauce: Recipe of the Apple Wireless Direct Link Ad hoc Protocol» — реверс протокола (формат кадров, Availability Windows, master election, синхронизация accuracy, channel hopping dynamics, achievable throughput, security assessment); Demo-статья «Linux Goes Apple Picking».
- Замеры производительности AWDL: Yggdrasil-мэшинг поверх AWDL (Mac-to-Mac): iperf3 >100 Mbps; замечание — при активном AWDL снижается скорость инфраструктурного Wi-Fi ( hopping).
- Замеры латентности на macOS-стороне (важно как анти-прецедент): AWDL вырывает общую радиокарту на чужой канал каждые 1–2 с: domos.ai — avg 118 мс с AWDL против 36 мс без (10% worst 264 vs 38 мс); независимый тест — avg 25,4→4,3 мс, max 93→12 мс, jitter 41,6→1,8 мс, спайки >15 мс: 29%→0%. AWDL предпочитает каналы 6 (2,4) и 44/149 (5 ГГц); Wi-Fi-сеть на канале 149 минимизирует hops. Обход: sudo ifconfig awdl0 down (macOS переактивирует после сна). Существенный риск для нашего sender: если iPhone держит AWDL-окна, инфраструктурный Wi-Fi под ним деградирует — OBS для фазы 4.
- AirportItlwm-Tahoe (kgp-macPro, hackintosh, Intel AX210): активный awdl0 + AirLink/wifip2pd, двунаправленный AirPlay и Screen Mirroring на macOS Tahoe. Диагностический факт из gist: чужой Apple TV в Пункте управления виден ТОЛЬКО на awdl0 (mDNS _airplay-p2p, link-local fe80::, отсутствует в arp -a); dns-sd CLI не броусит AWDL по умолчанию. OBS: P2P-AirPlay-сервис живёт на awdl0, а не в LAN.
- Вердикт для плана: эксперимент «nexmon на Pi + OWL + shairport-sync на awdl0» технически мыслим (OWL поднимает awdl0, mDNS над ним работает), но: OWL мёртв с 2019, монополизирует радио (нужен второй интерфейс/донгел, если хотим hotspot одновременно), требует injection, и даже при поднятом awdl0 iPhone в пикере должен увидеть и выбрать наш приёмник — это упирается в незакрытый кейс пикера (L-63). Записываем как OBS/future, НЕ как фазу. Если когда-нибудь дойдёт — минимальный эксперимент: OWL на AR9280-донгеле, shairport-sync (AP1) на awdl0, iPhone на той же AWDL-канальной паре; критерий успеха — приёмник виден в пикере.

## L-96 — Wi-Fi Aware (NAN): стандартизированный P2P — будущий путь

- По обязательству перед ЕС (DMA) Apple открыла Wi-Fi Aware (NAN, Neighbor Awareness Networking) для сторонних разработчиков в iOS 26 — не только в ЕС. NAN — открытый стандарт (та же технология на Android); устройства iPhone 12+, iPad 10-го поколения и новее.
- Apple позиционирует NAN для media streaming, file transfer, accessory control, screen sharing — всё P2P, без интернета и роутера; опциональный шаг код-подтверждения «как в AirDrop».
- Прецедент приложения: Close-Call (salnav/Close-Call) — видеозвонки поверх Wi-Fi Aware на iOS 26: Camera/Mic → VideoToolbox H.264/AudioToolbox AAC → RTP (FU-A) → AES-256-GCM → UDP over Wi-Fi Aware. Т.е. аудио/видео P2P без сети на iOS — уже реальность, но только внутри стороннего приложения (нативный пикер AirPlay NAN не использует).
- Для плана: если требование «AirPlay без какой-либо сети» станет критичным, реалистичный санкционированный путь — не AWDL (закрыт), а companion-приложение на iOS поверх Wi-Fi Aware (меняет форму продукта: нужно приложение, не нативный пикер). Linux/Pi-сторона NAN пока незрелая (экспериментальная поддержка wpa_supplicant). Статус: future; не в фазах; пересмотреть при появлении зрелой Linux-реализации NAN.
- Отметить связь с L-94: hotspot-профиль остаётся главным уличным путём; NAN — альтернатива на горизонте, не замена.

---

## L-97 — DMA-открытие (обновление L-96)

- Обязательства Apple перед ЕК по DMA: опубликованные спецификации interop-протоколов; публикация Ditto цитирует обязательство формулировкой «Apple shall not prevent AWDL from becoming part of the Wi-Fi Aware standard» — регулятор целит в конвергенцию AWDL и NAN, не только в NAN API.
- Прецедент Google: Quick Share получил AirDrop-совместимость по тем же обязательствам — третий-сторонний вендор допущен к Apple-P2P.
- Wi-Fi Aware 4.0/5.0 в iOS 26: стандартизированный P2P для сторонних.
- ОГРАНИЧЕНИЕ: NAN-железо нужно ОБЕИМ сторонам — приёмнику тоже. Wi-Fi-чипы Raspberry Pi (Broadcom/CYW) NAN не умеют; для релея NAN = смена платформы или внешний чип.

## L-98 — Airdrop-on-Windows как OBS «протокол ≠ транспорт»

Прецедент: AirDrop-совместимая реализация на Windows работает поверх Wi-Fi Direct + BLE-discovery — то есть протокол отделён от транспорта Apple. Подтверждает принцип «протокол AirDrop/AirPlay ≠ AWDL»: тот же протокол может жить на другом L2-транспорте. Для нашего уличного трека — теоретическое обоснование «AirPlay без AWDL», но практического пути AirPlay-over-WiFiDirect нет: iPhone не клиент Wi-Fi Direct (ограничение Apple, см. L-94/L-95).

## L-100 — AWDL AirPlay: статус мира + план «HomePod-имитации» (обновление L-95)

Статус мира (замеры 2026-11-13):
- Peer-to-peer AirPlay (AWDL) существует ТОЛЬКО у Apple-железа: Apple TV (A1469+) и HomePod. Sonos — MFi-лицензиат AirPlay 2 — AWDL не получает (официальный ответ Sonos: «без роутера связи не будет, с нашей текущей интеграцией»); в iakacer AIRPLAY_2_SPEC (L-89) peer-to-peer AirPlay записан как non-goal/out of scope.
- ReAWDL (hackintosh, AX210) — НЕ прецедент стороннего стека: Apple-стек (wifip2pd, IO80211) на не-Apple радио. Подтверждает состав сервисов на awdl0: _airplay + _airplay-p2p + _raop.
- EZCast Pro D15 — вендорский прецедент режима «AirPlay P2P (AWDL)» на не-Apple железе (уровень подтверждения: мануал, не wire-реверс). Apple-стек в принципе принимает AWDL-приёмник от не-Apple железа.
- Сторонних Linux-OWL реализаций AirPlay-приёмника НЕ существует: RPiPlay issue #9 («Peer-to-Peer AirPlay — is it possible?», 2019) и shairport-sync issue #933 — два независимых вопроса без единого ответа годами. Прецедента нет — мы первые.
- UxPlay -p2p (macOS only, только с Bonjour-бэкендом — НЕ работает при сборке с -DUSE_MDNS=1; обязательно с -pin, т.е. legacy-pairing путь): прецедент приёмника на awdl0 ВНУТРИ Apple-стека macOS (использует SO_RECV_ANYIF). Тот факт, что пикер принимает его — фильтр пикера в AWDL-секции не криптографический, как минимум для legacy-пути. РЕФЕРЕНС-ЗАХВАТ: дамп mDNS-анонсов с UxPlay -p2p на нашем Mac = эталон для воспроизведения (шаг 0 эксперимента).
- Факты MobiCom '18: Apple на macOS дропает пакеты от TCP/UDP-серверов, привязанных напрямую к awdl0 (нужен SO_RECV_ANYIF) — на Linux с OWL этого ограничения НЕТ, приёмник слушает напрямую; AirPlay-пир держит связь до RSSI −78 (обычные AWDL-пиры — до −65): запас по дальности для улицы больше, чем у AirDrop.

План «HomePod-имитации» (платформа по решению пользователя: nexmon на RPi 4B, донгел не нужен для первого шага):
1. Контроль на LAN: мимикрия model=AudioAccessory5,1 в пикере — базовая линия.
2. OWL/opendrop-rs поднимает awdl0, синхронизация с iPhone в AWDL-кластере.
3. BLE-маячок 0x09-only (uxplay-beacon.py, фиктивный IPv4): гипотеза — iPhone поднимает AWDL-окна от одного 0x09, адрес приёмника берёт из mDNS по awdl0; 0x16-токен (7-байтный AWDL-токен ATV4K, нереверснут) — только если пикер молчит.
4. mDNS на awdl0: анонс ВСЕГО состава сервисов (_airplay + _raop + _airplay-p2p), SRV на link-local fe80::, deviceid в TXT, model=AudioAccessory5,1. Мимикрия — ПЕРВИЧНЫЙ механизм входа в пикер: обходит MFi-ловушку битов 26/51 (L-64).
5. shairport-sync (AP1) на awdl0, слушает link-local IPv6.

Матрица исходов: (а) появился и стримится → P2P-путь открыт; (б) появился, коннект не доходит → фильтр на модель проходит, ломается ниже (пейринг/RTSP); (в) молчит → либо нужен 0x16-токен, либо фильтр криптографический — вопрос закрывается честно.

Риски nexmon: крэши на быстрых мелких инъекциях, монополизация радио (одновременно hotspot невозможен), нет ACK/ретрансмиссии. Для шага «видимость в пикере» — терпимо; для throughput — замер. Донгел MT7921AU/AR9280 — только если тест пройдёт и нужна скорость.

---

## Сводная секция «Пикер» (патч-напоминание 16; опоры L-63, L-69, L-84, L-90)

### Слой 1 — три канала, которыми приёмник попадает в пикер
1. mDNS (LAN): _airplay._tcp + _raop._tcp — штатный путь, Apple-железо не требуется.
2. BLE-beacon: manufacturer 0x004C, continuity type 0x09; несёт IPv4+порт; iPhone читает маячок, пробует IP на достижимость и тянет имя по сети. Прецеденты у третьих сторон (Cisco, Barco ClickShare) — по лицензионной цепочке. Вывод для улицы: beacon сам по себе НЕ делает приёмник видимым без IP-пути — это канал «подсказки адреса».
3. AWDL/P2P-секция: сервисы на awdl0 (link-local fe80::). Сегодня только Apple-стек (macOS UxPlay -p2p, ReAWDL). Наш открытый кейс.

### Слой 2 — классификация
Дерево из спеки openairplay (audio-логика): model префикс AppleTV → Apple TV; AudioAccessory → HomePod; биты 26/51 → ThirdPartySpeaker; иначе Unknown. CanBeRemoteControlled = SupportsBufferedAudio и НЕ PINRequired. Мимикрия model=AudioAccessory5,1 попадает в HomePod без активации MFi — самый дешёвый вход в пикер.
Ограничение спеки: описывает 4 audio-подтипа, а иконок в пикере больше (Mac с Monterey 2021, iPhone-приёмник iOS 18.1+ ограниченно, AirPort Express, smart-TV) — у видео-слоя пикера своя, публично не описанная логика. Механизм иконки smart-TV при Unknown-подтипе не реверснут. Наш релей — аудио-приёмник, остаётся в задокументированной ветке. Полный справочник классов и иконок — отдельный документ PICKER-DEVICE-CLASSES.md (рядом с накопителем, пополняется дампами).
iPad приёмником не является никогда — факт-ограничение.

### Слой 3 — TXT-словарь
Ключевые поля (L-84): deviceid, features (ДВА 32-битных слова через запятую — правило парсинга!), model, srcvers, pi, gid/pgid (L-86: дерево высоты 2), fex (не монолит, L-69), flags/md-биты (0 text, 1 artwork, 2 progress, 50 bplist), rsf/hgid/hmid/pgcgl.
Эталон сравнения — дампы L-69: AudioAccessory1,1 features=0xBC354BD0 vs AudioAccessory5,1 0xBC356BD0 (разница — бит 45). Sonos: pi в MAC-форме, свои manufacturer/serialNumber/fv — пикер терпит «не-Apple» значения в LAN-секции.

### Слой 4 — BLE-механика
Полная legacy-реклама ~46 байт, manufacturer data Apple ~14 байт (0D FF 4C 00 09 08 13 30…); seed инкрементится при каждом обновлении; ADV_NONCONN_IND ~100 мс; ATV4K добавляет второй ADU (type 0x16, 8 байт — 7-байтный AWDL-токен, нереверснут). Приватность (L-91): публичный MAC + IPv4 в cleartext → SECURITY.md.

### Слой 5 — профили релея
| Профиль | Канал видимости | Классификация | Статус |
|---|---|---|---|
| Дом (LAN) | mDNS | mimicry AudioAccessory5,1 | рабочий |
| Улица (hotspot, L-94) | mDNS внутри своей AP | тот же | рабочий, главный |
| Улица (AWDL, L-95/L-100) | AWDL-секция | неизвестно — фильтр не проверен | эксперимент; эталон — захват с UxPlay -p2p |

### Открытые вопросы (закрываются только тестами)
1. Фильтр AWDL-секции — криптографический или нет.
2. 0x16-токен — обязателен ли для поднятия AWDL-окон iPhone (гипотеза: 0x09 хватает, адрес из mDNS).
3. HomeKit-ожидания при мимикрии HomePod (пейринг, MRP-команды; ср. L-73 Siri proxy path).
4. BLE standalone-видимость без сети у не-Apple приёмников — судя по всему, отсутствует (только «подсказка IP»).

---



- hylarana (свой протокол скринмастинга, тег airplay вводит в заблуждение), bobcaygeon (Go, старый AP1), jburnhams/airplay2-rs (пустая заглушка).
- omarchy-airplay (UI-обёртка), ras (только RAOP), OpenAVNU/Zephyr gPTP (фоновые; у нас statime), хабр-статья про PTP (не AirPlay-специфична), npm-пакеты, дубли iDescriptor/shairplay-rust и doubletake-bin.
- RTSP-библиотеки общего назначения (SmolRTSP и др. — sender строит клиент сам), docker-образы древнего C-shairplay, пользовательские треды Apple/Samsung (фольклор), обзорные статьи.
- News tvOS 26, RTSP-камеры, Indigo-плагин как продукт (взяты только отдельные PR/факты), HA-треды.
- hap-controller (перекрывается pyatv-rs/hap-rs), generic-BLE-парсеры и ESP-IDF-доки, Habr-статья про AirDrop как «источник» (взяты только факты OWL ~10 с / nexmon setup).
- ShairportQt / Shairport4w (AP1-приёмник Windows; перекрывается уже имеющимися референсами; mDNS без Bonjour — не новое, BeamCast уже покрывает).
- AirReceiver (Android, проприетарный мод), Kali/pen-test туториалы (взяты только факты nexmon).

## Патч-напоминания для сборки v12

1. Перенести критерии RX/TX фазы 5 из §8 (L-20) в §2 — нормативную часть.
2. §1.1: переформулировать shairport-sync как «база AP1 + живой AP2-референс» (L-75).
3. ARCHITECTURE/SECURITY: явно решить владение UDP 319/320 (L-76).
4. Фазы 3–5: в эталонный захват добавить «feedback под нагрузкой» (L-68) и проверку TVRCSessionStart/SystemStatus (L-80).
5. VALIDATION: забрать методики soundcheck / signal-estimator / bit-exact паттерны (L-83).
6. D58/D80: сверка features-слова 0xBC354BD0 с живым HomePod 1 (L-69; есть железо).
7. WIRE-SPEC: внести BLE-наблюдения (L-56/L-90/L-91) как OBS; private-fact про cleartext IPv4 — в SECURITY.md.
8. §1.4/D12/D60: добавить нативный Apple-прецедент hub-модели из 27.2 (L-74).
9. Уличный профиль: hotspot-профиль (L-94) как deployment-кейс D16; AWDL — OBS/future (L-95), Wi-Fi Aware — future (L-96).
10. L-63 (пикер): переформулировать кейс — LAN-видимость сторонних приёмников ПОДТВЕРЖДЕНА (биты 26/51 → ThirdPartySpeaker; прецеденты shairport-sync/UxPlay/Sonos/Denon). Открытый кейс — только AWDL-секция пикера (см. секцию «Пикер», слой 5).
11. L-95/L-100: уточнённый критерий успеха AWDL-эксперимента — приёмник виден в пикере как P2P-цель. Приоритет мимикрии: model=AudioAccessory5,1 в анонсе на awdl0 — первичный механизм входа в пикер, обходит MFi-ловушку битов 26/51 (L-64). BLE 0x09-only первым шагом (0x16 — только если пикер молчит).
12. L-100: peer-to-peer AirPlay — только Apple; Sonos (MFi) без AWDL; ReAWDL — не прецедент стороннего стека; EZCast Pro D15 — вендорский прецедент (уровень: мануал).
13. L-100: план «HomePod-имитации» на nexmon RPi 4B (платформа — решение пользователя 2026-11-13); донгел MT7921AU/AR9280 — только при успехе теста видимости и потребности в throughput.
14. L-100: матрица трёх исходов AWDL-эксперимента (стримится / коннект не доходит / молчит) — в экспериментальную секцию плана.
15. L-100/L-95: UxPlay -p2p — референс-захват, шаг 0 эксперимента: дамп mDNS-анонсов с Mac (сборка UxPlay БЕЗ -DUSE_MDNS=1, обязательно -pin). ReAWDL — подтверждение состава сервисов (_airplay + _airplay-p2p + _raop). Факты MobiCom '18: SO_RECV_ANYIF на Linux не нужен; AirPlay-пир до RSSI −78. shairport-sync #933 — второе свидетельство «вы первые» (рядом с RPiPlay #9).
16. Сводная секция «Пикер» (после L-100, выше) — перенести в план: слои 1–2 как опора D80/D84, слой 4 как OBS в WIRE-SPEC (дополнение к патчу 7), слой 5 как связка уличного трека. Справочник классов и иконок — отдельный документ PICKER-DEVICE-CLASSES.md (не в тело плана, пополняется дампами; шаг 0 добавит точные model-строки Mac-приёмника).
17. ВЕРСИЯ СБОРКИ: применить ВСЕ накопленные патч-напоминания (1–16 этого файла) при сборке НОВОЙ версии плана — AGENT-PLAN-AP12-20261112 v12. НЕ выпускать очередную редакцию 11.x: изменения нормативные (критерии, архитектура, экспериментальная секция), а не точечные. Суммарный итог переносов см. в реестре и секции «Пикер».
