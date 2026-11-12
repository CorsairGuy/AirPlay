# APPENDIX — AGENT-PLAN-AP12-20261112 (v11-addendum: D57–D63)

Маркер: current-status-20261112
Приложение к v11, НЕ заменяет его. Применяется поверх v11 целиком.
Меняет: §2 (фазы), §5 (S-матрица), §6 (пункты 14–16), §7 (лицензии), §0 (правило 5).
Добавляет: §A (D57–D63), §B (OBS-D57-01..35 — факты с провода для WIRE-SPEC).
Формулировки D1–D51, D55 — читать из v11, здесь не дублируются.
ID D52–D54 и D56 — зарезервированы за репозиторием Станции (не использовать).

---

## §A. Новые решения (D57–D63)

- **D57 — music-assistant/airplay-cli (GPL-3.0, чек закрыт).** Единый C-бинарь: RAOP, AP2
  RAOP-compat, AP2 native с PTP. Главный кросс-чек фаз 3–4 и лестницы кнопки. Их DESIGN.md —
  implementer-RFC, добытый на реальном железе (fleet A/B 2026-07-31: Sonos Era 100 ×2 + Bookshelf,
  WiiM Pro, Samsung HW-LS60D; Edifier MS50A — ниже порога рендера, вердикта нет). Код не портируем;
  wire-факты импортируются как OBS-D57-01..35 в WIRE-SPEC (§B).
  Лицензия (разрешена, обновляет §7): репозиторий GPL-3.0; файлы src/ — per-file SPDX Apache-2.0,
  КРОМЕ src/cliairplay.c — GPL-2.0-or-later (производная libraop/cliraop.c); libraop — git-подмодуль
  (philippe44/libraop, RAOP-ядро от raop_play GPL-2.0+); mdnssd (bundled mDNS-клиент) — GPL-3.0;
  THIRD_PARTY_NOTICES.md — инвентаризация компонентов с каждым релизом.
  Вывод: для проекта GPL-3.0-or-later совместимость OK; код libraop/cliairplay.c не копировать вне
  GPL-контекста; OBS и вектора — свободны.
  КАРТА src/ (структурный референт для разбиения crates фаз 2–3): cliairplay.c (entry/dispatch/
  cmdpipe), ap2_client.c (оркестратор, realtime sender, anchor&pacing), ap2_session.c (flush-and-
  refill engine на одном stdin), raop_session.c, ap2_hap.c (HAP pair-verify/transient, шифрованный
  RTSP), ap2_io.c (deadline I/O RTSP+MRP), ap2_mrp.c (/command builders, proto2+bplist, type-130
  DataStream), ap2_plist.c / ap2_bplist.cpp (bplist writer/reader), ap2_ptp.c (gPTP BMCA,
  hold-grandmaster, unicast grants + daemon), ap2_ptp_shm.c, alac_ext.cpp (ALAC encoder override
  с корректной 24-бит поддержкой).
  Ссылочная база их реализации (провенанс-строка для WIRE-SPEC): libraop (RAOP-ядро), owntone
  rtp_common.c (layout RTP sync-пакетов), libairptp (gPTP grandmaster dataset), pyatv
  (MediaRemote/DataStream + номера полей protobuf), openairplay/airplay2-receiver (rig захвата
  now-playing), shairport-sync (якорная математика), Cozzi notes (D36).

- **D58 — openairplay/airplay-spec (сайт-спецификация, раздел service discovery).** Таблица
  групповых mDNS-полей gid / igl / gcgl / pgid / pgcgl / gpn / hgid / hmid с примерами переходов
  состояний приёмника (idle / играет аудио / видео / mirroring): при входе в приём gid меняется,
  igl/gcgl обнуляются, pgid наследуется. Это ПРИЁМНАЯ сторона кнопки: чек-лист того, что приёмник
  обязан отражать в mDNS при join/leave группы, иначе iPhone не покажет группу корректно.
  → WIRE-SPEC + шаг 0 лестницы GROUP-BUTTON-EXPERIMENTS (что снимать в эталонном захвате).
  OBS-only, без копирования кода.

- **D59 — owntone-server issue #1413.** Правило: pgid имеет приоритет над gid; группа = участники
  с общим pgid; стереопара — подгруппа с родительской группой поверх. Ключевое OBS: при старте
  воспроизведения на группу из web-UI owntone открывает НЕЗАВИСИМЫЕ сессии, приёмники меняют
  свои state-флаги — и «группа магически растворяется» в UI iPhone. Точное описание ловушки
  fan-out. → Новый кейс в GROUP-BUTTON-EXPERIMENTS (шаги 2–3) + второе независимое подтверждение
  §1.4 (hub-модель). Экспериментальная ветка owntone с SPEAKER_GROUPING — референс sender-side
  группировки без нативного adoption.

- **D60 — pyatv issue #2005 (закрыт #2006).** pyatv рапортовал громкость не своей, а последней
  присоединившейся колонки: группировка шла по последнему VOLUME_CONTROL_CAPABILITIES_DID_CHANGE_
  MESSAGE, в котором при входе в группу приходит UID группы. Для блока 1 миграции (устойчивые
  ползунки) и D12: ползунок обязан матчить СОБСТВЕННЫЙ UID, а не последний пришедший volume-апдейт.

- **D61 — контракт clock-readiness и cold-join (из D57).** Тест-матрица §5 дополняется S9
  (см. §C). Факты: приёмник начинает Delay_Req ~1 с после CONNECT независимо от якоря/START;
  готовность = streak 2.3 с (замеренный Sonos servo-lock window) / для Apple-моделей — третий
  обмен + 250 мс; START не ускоряет пробу. Методика измерения готовности для D9: streak/exchanges,
  projection jitter 50 мс, сравнение с последней отправленной строкой, а не с последним сэмплом.
  PTP_MAX_PEERS=8: приёмник за лимитом не получает unicast timing и остаётся cold навсегда —
  таймаут ожидания на стороне вызывающего обязателен.
  Флаг START_JOIN НЕ СУЩЕСТВУЕТ (ранняя гипотеза снята): механизм позднего входа выражается через
  START_UNIX_MS на замороженной сплайс-линии группы — joiner получает START с инстантом на
  групповом таймлайне, зазор заполняется кодированным тишиной (см. OBS-D57-15).

- **D62 — DACP ID как единый identity (из D57).** DACP ID (16 hex) — единственный источник
  identity: из него получаются и colon-form deviceID/macAddress в SETUP, и 64-битный PTP clock
  identity; поэтому grandmasterIdentity всегда совпадает с ClockID сессии. В нашей реализации
  clock identity и deviceID/macAddress ОБЯЗАНЫ быть согласованы с DACP ID — иначе
  grandmasterIdentity ≠ ClockID сессии и групповые сессии рассыпаются. → WIRE-SPEC; проверка
  фаз 3–4; кросс-чек §1.4.

- **D63 — MRP now-playing факт-лист (из D57).** Три дополнения к §6 (внесены как пункты 14–16):
  (14) транспорт now-playing — POST /command по основному шифрованному RTSP-каналу (тот же, что
  /feedback), Content-Type: x-apple-binary-plist; type-130 для этого НЕ используется; DEVICE_INFO
  несёт lastSupportedMessageType=139 и session/group UUID в полях 41/42; (15) replace-push БЕЗ
  ArtworkData переворачивает artworkAvailable в false (замерено на Apple TV 4K, tvOS 26/27,
  с полями identifier и без них) — приёмник не кэширует по identifier; артовок обязателен в каждом
  replace-push (или merge-push модель); (16) keepalive 250 мс и reverse event channel idle ~30 с —
  в телеметрию S-сценариев.

---

## §B. OBS-D57-01..35 — wire-факты для WIRE-SPEC (импортировать как OBS, не как код)

PTP и identity:
- **01.** gPTP-фрейминг: majorSdoId=1 в первом байте; приёмники отбрасывают plain IEEE-1588
  (majorSdoId=0). Один бит решает, «быть часами» или «быть невидимым».
- **02.** iOS grandmaster dataset: priority1=128, clockClass=6 (GPS-locked), accuracy=0x21
  (100 нс), variance=0x436A, timeSource=GPS.
- **03.** → D62 (DACP ID как единый identity).
- **04.** → D61 (PTP_MAX_PEERS=8, cold навсегда за лимитом).
- **05.** Followed receivers: если приёмник сам шлёт Announce, движок записывает его
  grandmasterIdentity, снимает local→peer offset с unicast two-step Sync/Follow_Up (~8 Гц)
  и не шлёт ему свои Announce/Sync; до 8 followed clocks в shm (layout v3); START-якорь
  сдвигается из CLOCK_REALTIME в followed-таймлайн per receiver. Session SETUP при этом всё ещё
  называет наши часы — замерено, на поведение не влияет.
- **06.** → D61 (clock readiness: пробы с CONNECT, не с якоря; Samsung Music Frame HW-LS60D:
  SETPEERS T+0.00 → первый Delay_Req T+1.078, затем 187 проб за 24 с чистого idle).
- **07.** Session SETUP: timingProtocol=PTP, deviceID/macAddress (из DACP ID), session/group
  UUIDs, timingPeerInfo/timingPeerList (наши clock ID и advertised address; --publish-ip —
  override для Docker/NAT). Ответ session SETUP даёт eventPort как keep-open reverse TCP.

Фрейминг и шифрование:
- **08.** HAP-фрейминг RTSP: [2-б LE длина][ChaCha20-Poly1305 ciphertext + 16-б tag], max 1024
  байта plaintext на фрейм; nonce = 4 нулевых байта + 8-б LE per-direction counter.
- **09.** shk = первые 32 байта shared secret: raw X25519 (pair-verify), SHA512(S)[:32]
  (transient). Кросс-чек D19 (key_len 64 при transient pairing → шифрование аудио 32): согласуется.
- **10.** Аудио: ChaCha20-Poly1305, key=shk; nonce = 12 байт, нули кроме 2-б RTP sequence на
  offset 4 (native byte order, owntone-совместимо); AAD = байты RTP-заголовка 4..11 (timestamp +
  SSRC); последние 8 байт — low nonce bytes для реконструкции приёмником. SSRC=0 для PTP-сессий
  (стрим ключуется по PTP clock identity), streamConnectionID — для NTP. Кросс-чек D29
  (streamConnectionID): согласуется.

Парсинг и пароли:
- **11.** Ответ stream SETUP парсить ТОЛЬКО по ключам бинарного plist: приёмники обычно
  сериализуют controlPort ДО dataPort — позиционный парсинг шлёт аудио на control-порт и мьютит
  устройство.
- **12.** Пароли: pw=true лишь означает «рекламируется», digest используется только при реальном
  challenge. AP2 единого механизма нет: часть приёмников подставляет пароль вместо фиксированного
  3939 transient SRP secret; реальные HomePod отвечают 401 на session SETUP ПОСЛЕ успешного
  handshake. Каждая «нога» пейринга — на СВОЁМ соединении: приёмник, отклонивший пейринг, клинит
  state machine на этом сокете; смерть «ноги» в проводе (не отклонение) — конец попытки, ретраев нет.
- **13.** Error contract: перед [ERROR] ровно одна машинная строка [STATUS] error code=<slug>
  http=<int> detail="..."; slug'и: auth_required / auth_failed / connect_failed / start_failed /
  flush_failed; TLV-отклонение приходит как HTTP 200.

Сплайс-таймлайн и join:
- **14.** Сплайс-таймлайн: якорная линия, замороженная на первом START, неизменна всю сессию;
  flush-верб на этом пути не шлётся никогда; тёплые границы (seek/next FLUSH+START, pause/resume,
  standby park/resume, starvation recovery) держат провод живым тишинным keepalive. Delivery stall
  длиннее pacing depth → REANCHOR (splice-pad таймлайна вперёд), не burst queued-контента.
  Deny-list ap2_splice_denied (по умолчанию пуст) сохраняет классический flush+re-anchor путь.
  Групповая пауза проводит участников через standby park.
- **15.** Cold-join контракт (обновлён по README, флага START_JOIN нет):
  (а) Apple-приёмники: сплайс-таймлайн с одной замороженной якорной линией; joiner стартует с
  START_UNIX_MS, указывающим на позицию в групповом таймлайне; зазор между текущим head и
  commanded start заполняется КОДИРОВАННЫМ ТИШИНОЙ, битстрим непрерывен. Ack: [STATUS] flushed
  head_unix_ms=<ms> — audible instant замороженной головы доставки; commanded start обязан лежать
  ПОЗАДИ head каждого участника, иначе этот участник сплайсится на собственном head.
  (б) Контракт ре-синхронизации группы (VERIFIED): невозможный START исправляется вперёд на
  ближайший осуществимый (минимальный lead 250 мс, 200 мс на RAOP); ack [STATUS] started
  requested_unix_ms=<ms> at_unix_ms=<ms> всегда несёт истинный запланированный инстант; вызывающий
  сравнивает два значения и ре-синхронизирует группу, посылая РЕ-START каждому участнику на
  НАИБОЛЬШИЙ reported instant.
  (в) Одноразовая линия [STATUS] audio buffered_ms=<ms> — «трек готов к START»; один и тот же
  START шлётся всем участникам после подтверждения по всем.
  (г) Cold-join через ПЕРВЫЙ START (cold-clock): коррекция сама эмитит [STATUS] started с
  исправленным инстантом; контент НЕ режется (иначе сдвиг дважды). Warm START после FLUSH при уже
  ушедшем ack: [STATUS] anchor_corrected requested_unix_ms= from_unix_ms= at_unix_ms=
  content_cut_ms= и [STATUS] content_cut requested_ms= cut_ms= cut_bytes= drain_ms=; queued-контент
  сдвигается на ту же величину — первый сохранённый сэмпл ложится ровно на назначенную точку
  групповой шкалы, участник входит в синхроне. Групповой/solo ORIGIN-start только логирует
  shortfall: приёмники self-seat свежую сессию за ~20 мс; сдвиг одного участника group-origin
  рассинхронизирует группу (подтверждено на слух). Исправленный инстант логируется WARN и виден по
  [STATUS] started.
- **16.** Latency: latencyMin/latencyMax из stream SETUP клампят lead и размеряют pacing window;
  Sonos эхо окна не шлёт → default 1,75 с; arrivalToRenderLatencyMs (Apple TV ~100 мс
  decode+HDMI+TV; Sonos опускает ключ) — информационно, в график НЕ добавляется; [STATUS] latency
  lead_ms= device_min_frames= device_max_frames= device_render_ms= warm_lead_ms=. Числа: минимальный
  тёплый lead 250 мс, pacing depth 600 мс, потолок depth 3000 мс.

MRP now-playing (дополняет §6, см. D63):
- **17.** → D63 (14): транспорт POST /command, НЕ type-130; DEVICE_INFO: lastSupportedMessageType=
  139; session и group UUIDs в полях 41/42.
- **18.** updateMRNowPlayingInfo: envelope npi-text/mergePolicy обязателен, иначе HTTP 400;
  Timestamp — CFDate (bplist marker 0x33); UniqueIdentifier — uint64, стабилен по треку.
- **19.** → D63 (15) + OBS-32: replace-push без ArtworkData → artworkAvailable=false;
  byte-идентичный бандл пропускается end-to-end; DMAP SET_PARAMETER на Sonos-классе ре-шлётся при
  каждом identity change; замороженный elapsed при pause/resume — с новым Timestamp.
- **20.** Артовки: известная sender-форма — 600×600, baseline JPEG ~43 КиБ; подтверждённых порогов
  (64 КиБ, 600px, прогрессивность) НЕТ — методика probe-матрицы (tests/mrp_artwork_matrix.py);
  капу не объявлять до замера на конкретном приёмнике. Уточнение: сигнатурная проверка, кап 5 МиБ;
  JPEG-probe требует SOI+EOI без полного декода, guard 1 МиБ на staging-аллокацию; CLIAIRPLAY_MRP=0
  — выключить MRP-путь для сравнения/диагностики.
- **21.** updateMRSupportedCommands: {params:{mrSupportedCommandsFromSender:[...]}} — каждый элемент
  это сериализованный command-info bplist (kCommandInfoCommandKey / EnabledKey / OptionsKey, с
  shuffle/repeat/scrub). Дополняет §6 п.9 (supportedCommands).

Жизненный цикл сессии:
- **22.** Keepalive: бюджет 250 мс (укладывается в paced-очередь); ответ не читается; advance
  только write nonce. Приёмник, пропустивший 3 keepalive, может не прочитать — shutdown сразу после
  третьего. → D63 (16).
- **23.** Reverse event channel: pair-verified сессии деривят независимые Events-Salt ключи,
  расшифровывают HTTP-запросы приёмника, отвечают зашифрованным 200 OK с echoed CSeq; idle на этом
  канале > ~30 с — tvOS рвёт MediaRemote-активный стрим. → D63 (16).
- **24.** EOF/idle: EOF-драин ≤ 4 с (2 с lead request + 2 с; depth ceiling 3000 мс держит очередь
  внутри бюджета); orphan idle timeout 120 с — arm'ится на connect / STANDBY / успешный FLUSH /
  закрытие input, disarm только успешный START; на экспирации [STATUS] idle_timeout, exit 0,
  без [ERROR].
- **25.** REANCHOR-счётчики: shifted_frames / total_shifted_frames в PCM-фреймах; total — с
  последнего START (commanded start обнуляет дрейф-базелин); только native AP2, только во время
  стриминга; повторные recovery складываются в уже поставленный pad.
- **26.** Разделение потоков: [STATUS] route/capabilities/latency/mrp path= — stdout;
  connected/clock_ready/audio/started/clock_verified/anchor_corrected/content_cut/flushed/REANCHOR/
  playing/paused — stderr; корреляция только по содержимому, не по порядку (два независимых буфера,
  даже при 2>&1). Прямой образец для RELAY_IPC-телеметрии (D8).

Прочее:
- **27.** Громкость: 0–100 → линейно-в-дБ на −30..0 дБ (raopcl_float_volume; так же iOS,
  shairport-sync, libraop) — позиция ползунка одинаково громкая на всех путях. Кросс-чек D60/D12.
- **28.** Realtime-сбои: локальный UDP backpressure = ограниченный transient drop (advance
  sequence/RTP/schedule); encode/alloc/encrypt/socket/control-ошибки — terminal, nonzero exit,
  а не ложный EOF. Проверять в S-сценариях §5.
- **29.** Мультирум-топология: один cliairplay --ptp-daemon на ХОСТ (не на устройство) — биндит
  UDP 319/320 один раз, публикует избранные часы в POSIX shm (/cliairplay-ptp) и держит localhost
  UDP control 127.0.0.1:9010, где потоки регистрируют IP своих приёмников; стримы с --ptp-shared
  прикрепляются к shm read-only и НИКОГДА не биндят 319/320; при отсутствии живого демона —
  fallback на in-process engine. MA поднимает демона на первом PTP-стриме и глушит (SIGTERM) на
  последнем. → Кросс-чек D9/фазы 4: альтернатива «один демон на хост + shm» против per-sink
  engine (сохраняет архитектуру одной общей PTP-службы, см. D9).
- **30.** Аудио-вход: один персистентный PCM-поток на stdin на ВСЁ время жизни процесса; seek и
  next-track = ACTION=FLUSH + новый ACTION=START на том же живом потоке; соединение и stdin не
  переоткрываются. Релевантно архитектуре нашего TX (фаза 3).
- **31.** Sonos не издаёт аудио, пока не получил метаданные: бинарник пушит начальный набор
  метаданных на первом commanded start, чтобы звук шёл даже без SENDMETA. → OBS в §5 (Sonos-профиль).
- **32.** ITEMID-семантика now-playing: одинаковый ITEMID = апдейт in place (уточнение тегов не
  подаётся как новый трек); новый ITEMID = смена трека с 0:00; без ITEMID identity фолбэк на tuple
  title/artist/album. Byte-идентичный ARTWORK игнорируется; PROGRESS-коррекция едет lean merge на
  MediaRemote-сессиях (seek не перерисовывает карточку). → Дополняет D63(15)/OBS-19 и D12.
- **33.** → OBS-20 (уточнение по артовкам).
- **34.** → D57 (карта src/).
- **35.** → D57 (ссылочная база).

---

## §C. §5 тест-матрица — дельта (добавить строки)

| ID | Сценарий | Критерий успеха |
|----|----------|-----------------|
| S9 | Cold-join на живой групповой таймлайн (D61) | joiner входит в синхроне без рестарта источника; первый сэмпл ровно на назначенной точке групповой шкалы; anchor_corrected/content_cut телеметрия полная |
| S10 | HomePod OS27+ как приёмник-грандмастер (см. §E) | приёмник, анонсирующий себя grandmaster, обрабатывается по followed-модели OBS-05; при отсутствии follow — воспроизведение не молчит |

Дополнение к S8/S10 (критический OBS, Roon community + Music Assistant 2.9.8): на HomePodOS 27
beta HomePod может ПРОДОЛЖАТЬ анонсировать себя как PTP grandmaster и молчать, если отправитель НЕ
следует часам приёмника; «Prefer AirPlay 2» восстанавливает анонсы и воспроизведение. Это живое
подтверждение механики followed receivers (OBS-05) на самом свежем железе. → SESSION_PROFILES
дополняется профилем «HomePod OS27+».

Прочие дополнения к §5: OBS-31 (Sonos требует метаданные до звука) — в Sonos-профиль;
OBS-28 (terminal-ошибки не маскируются как EOF) — критерий для всех S-сценариев.

---

## §D. §6 MRP-чеклист — дельта (добавить пункты 14–16)

14. **Транспорт now-playing (D63)**: POST /command по основному шифрованному RTSP-каналу
    (тот же, что /feedback), Content-Type: x-apple-binary-plist; type-130 для now-playing НЕ
    используется. DEVICE_INFO: lastSupportedMessageType=139; session и group UUIDs — поля 41/42.
15. **updateMRNowPlayingInfo (D63/OBS-18/19/32)**: envelope npi-text/mergePolicy обязателен
    (иначе HTTP 400); Timestamp — CFDate (marker 0x33); UniqueIdentifier — uint64; ITEMID-семантика
    (in place vs смена трека); replace-push без ArtworkData → artworkAvailable=false.
16. **Канальная гигиена (D63/OBS-22/23)**: keepalive 250 мс; reverse event channel idle ~30 с —
    tvOS рвёт MediaRemote-стрим; проверить в телеметрии S-сценариев.

---

## §E. Привязки к фазам (дельта к §2 v11)

- Фаза 3: + D47 (уже), D48-encode (уже), + D57-кросс-чек, OBS-30 (персистентный stdin).
- Фаза 4: + D61 (методика готовности), OBS-29 (демон на хосте + shm как альтернатива).
- Фаза 5: + S10 (HomePodOS 27), OBS-05.
- Фаза 6: + D63 (§6 пункты 14–16), D55-вектора (уже).
- Фазы 3–4 (кросс-чек): D62 (DACP ID → identity).
- Лестница кнопки: D58 (шаг 0), D59 (шаги 2–3), D61/OBS-15 (шаг 2), D57 (главный кросс-чек).
- Блок 1 миграции: D60 (ползунки по собственному UID).

---

## §F. Лицензионная сводка — дельта к §7 v11

- D57 airplay-cli: лицензия РАЗРЕШЕНА — GPL-3.0 (проект), src/ per-file Apache-2.0 кроме
  cliairplay.c (GPL-2.0+); libraop — подмодуль GPL-2.0+; mdnssd — GPL-3.0. Совместимо с
  GPL-3.0-or-later. Снять D57 со списка license-check.
- D58 airplay-spec: OBS-only (документация), без копирования кода.
- Кандидат D49 buffa остаётся в списке license-check (не проверен).

---

## §G. История версий — дельта

- v11-addendum (этот файл): D57–D63 (седьмой заход: airplay-cli DESIGN.md вычитан целиком + README,
  групповые mDNS-семантики airplay-spec, owntone #1413, pyatv #2005); OBS-D57-01..35; §5 + S9/S10;
  §6 + пункты 14–16; лицензия D57 закрыта; флаг START_JOIN снят (механизм — START_UNIX_MS);
  HomePodOS 27 followed-кейс.
