# Единый план проверок Станции и ядра

Срез: 13 ноября 2026. Это единая очередь ручных проверок из документов обоих
репозиториев, а не заявление о завершении миграции. Автоматические unit/interop/
fuzz-проверки сохраняются в CI; их функции не надо повторять вручную.

Сканированы все 32 отслеживаемых Markdown-файла ядра и 41 файл Станции, включая
историю и WIP. Старые инструкции объединены по наблюдаемому поведению; архивные
команды установки не становятся разрешением их выполнить. Реестр источников ниже.

## Условия и обозначения

- Сегодня: два iPhone, Яндекс Музыка, Станция, HomePod в стереопаре с Apple TV.
- Проверяется установленная сборка; новый core-host ещё не установлен. Зелёный
  CI ядра и галочка маршрута не доказывают работу установленной колонки.
- Home Theater выбирается общей строкой; не разъединять пару и не создавать
  независимые потоки на её followers. Home Theater не выше33%, Станция тихо.
- Голос Алисы остаётся локальным. Контент HomePod во время ответа НЕ приглушать.
- Между сравниваемыми опытами не менять прошивки, capabilities, gid и калибровку.
- PASS — свежий результат; FAIL — воспроизведённый дефект; PARTIAL — неполный
  результат; TODO — не проверено; BLOCKED — отсутствует необходимая реализация/стенд.
- Перезапуск, установка, остановка служб, смена маршрутов, fault injection и
  удаление pairing требуют отдельного конкретного согласия. Их не запускать
  только потому, что они перечислены здесь. Обычные ручные тесты идут по порядку.

## A. Текущая Станция без ретрансляции

| ID | Действие и критерий | Результат13ноября |
|---|---|---|
| A01 | Музыка Алисы → карточка на втором iPhone → Pause/Play/Next → вопрос о времени. Метаданные обновляются, музыка и голос работают, микрофоны слышат пользователя | PASS, пользователь |
| A02 | Яндекс Музыка iPhone1 → AirPlay на Станцию; iPhone2 открывает карточку посреди трека. Обложка есть; 3 Pause/5s/Play не сбрасывают трек/обложку и не скачут по времени | PASS, пользователь |
| A03 | Pause/5s/Play на iPhone-источнике отражаются на втором телефоне. Фиксировать задержки отдельно для Pause и Play | PASS: оба сразу, пользователь |
| A04 | Оставить AirPlay подключённым на паузе, затем явно отключить. Источник карточки меняется только после отключения, не при паузе | PASS, пользователь |
| A05 | Bluetooth → AirPlay → Bluetooth. Каждый раз слышен выбранный источник, фоновые подключения не перехватывают звук | PASS по звуку, пользователь |
| A06 | Первый AirPlay без предварительного Bluetooth, затем BT→AirPlay; AirPlay сразу ровный белый84, без синего/мигания; настоящий BT синий, анимации Алисы сохранены | FAIL: первый AirPlay без белого; после Bluetooth белый появился. Остальные подпункты не закрывают этот дефект |
| A07 | Ползунок карточки второго iPhone меняет звук и возвращает уровень на iPhone-источник; физические кнопки громкости отражаются на обоих | PARTIAL/FAIL: ползунок второго не обновляет первый; кнопки Станции обновляют оба |
| A08 | При играющем AirPlay и Bluetooth-планшете на паузе Play/Pause/Next/Prev Станции управляют активным AirPlay, не планшетом | FAIL: Play/Pause отправляется Bluetooth-планшету; активный AirPlay подтверждён пользователем |
| A09 | Музыка Алисы: 3 Pause/Play, смена трека и повторное открытие посреди нового трека. Уже загруженная обложка не мигает; новая загружается | PASS: 3 Pause/Play, Next и повторное открытие; обложки/команды исправны, пользователь13ноября |
| A10 | Вопрос Алисе отдельно при AirPlay и Bluetooth: голос только локальный, местный контент приглушается/восстанавливается, источник не теряется | PASS для AirPlay и Bluetooth: голос/приглушение и возврат; отдельный дефект возврата native после BT вынесен в A13 |
| A11 | Смена отправляющего телефона, 3 переподключения, открытие/закрытие карточки5раз; стабильные identity, metadata и команды | PASS takeover1→2→1: окно замены, один источник, свежая карточка и команды; длительная серия переподключений TODO |
| A12 | Центр/свайпы/Next/Prev, «хватит»/«играй», голосовая громкость. Команды относятся к выбранному источнику; возврат не запускает старый чужой контент | TODO |
| A13 | Без ретрансляции: музыка Алисы → Bluetooth → отключить Bluetooth на планшете → «Алиса, продолжай»; явное «включи музыку» проверять отдельно | FAIL подтверждён в чистом повторе: тишина, Music255/255, нет sender/local_player/owner, receiver Stopped; статус music_thin/playing=true. Явное «Алиса, включи музыку» сразу после сбоя восстановило звук без изменения микшера/служб (подтверждено пользователем). Причина возобновления не установлена; групповой mute исключён для этого повтора |

При FAIL записываются источник, шаг, состояния обоих телефонов и время. Результаты
A01–A08 относятся к живому установленному стеку, не к новому ядру.

## B. Установленная ретрансляция — сначала музыка Алисы

Начинать с B01. Если выбор/звук не работает, сначала сохранить журналы; не
переходить к массовой смене параметров. UI и аудио оцениваются раздельно.

| ID | Действие | Критерий и что записать | Статус |
|---|---|---|---|
| B01 | Выбрать общую строку Home Theater при музыке Алисы | Галочка держится; фактически играют Станция и оба HomePod; состав стереопары сохранён, транспорт без непреднамеренных дубликатов. Если нужен PIN — отдельный сценарий D03 | PASS выбора/звука после подтверждения маршрута; начальный защитный отказ сохранён. Топология транспорта отдельно TODO |
| B02 | Слушать общие удары/слова после соединения | Нет слышимого эха; отдельно описать стартовый переход/его длительность и направление остаточного смещения. Это не доказательство одного сэмпла | PASS на слух, пользователь13ноября; не измерение одного сэмпла |
| B03 | Спросить Алису о времени при выбранной группе | Голос только на Станции; локальная музыка приглушается/возвращается; HomePod-контент продолжает играть на прежнем уровне | FAIL: контент HomePod прерывается, пока Алиса слушает; голос локальный, остальные подпункты соответствуют |
| B04 | Свернуть список устройств в карточку, открыть снова | Кнопка представляет Станцию + Home Theater; выбранные строки и согласованные ID сохраняются. Одинарная подпись остаётся дефектом независимо от хорошего звука | FAIL: только «Станция Мини», пользователь13ноября |
| B05 | Список без касаний20s, затем вести отдельный ползунок Home Theater10–30%15s и ползунок Станции | Нет потери захвата пальца/сброса значения. Само мигание строки не равно потере захвата; записать отдельно | PASS: захват обоих ползунков сохраняется, пользователь13ноября |
| B06 | Отдельные уровни Станции и Home Theater, затем общий ползунок и кнопки iPhone | Индивидуальный меняет свой выход; общий меняет оба с сохранением пропорций. После общего нуля и возврата прежние относительные уровни восстанавливаются | PASS: индивидуальные независимы, пропорции/звук восстановились после нуля |
| B07 | Снять общую галочку; выбрать снова при продолжающемся треке | Отключается, не появляется снова сама; локальный звук не теряется; повторный join не перезапускает источник (S1/S2) | PARTIAL: галочка/звук/обложка корректны; позже найден orphan mute Music0 без sender/renderer/owner. Возврат direct route не закрыт |
| B08 | Pause/10s/Play, Next/Prev, seek+20s при группе | Команды/карточки согласованы; нет старого PCM после перехода, новый трек слышен, синхронность восстанавливается без постоянных перезапусков | PASS пользователя: Pause/10s/Play/Next/seek в группе |
| B09 | Закрыть/открыть карточку, присоединить второй контроллер посреди трека | Текущая обложка/позиция появляются сразу; группа не распадается, нет stale metadata | TODO |
| B10 | При сохранённом выборе группы отдельно подать входящий AirPlay и Bluetooth | Передаётся только выбранный медиапоток, без голоса Алисы/смешивания; отдельно проверить pause/seek/track/source transition. Сначала подтвердить, что текущие маршруты этих источников поддержаны | TODO, готовность маршрутов не установлена |
| B11 | Home Theater уже воспроизводит свой контент, затем запрос Станции | Поведение takeover/reject явно зафиксировано; нет ложной галочки, скрытого смешивания или потери прежней сессии без понятного перехода (S11) | TODO, после согласования переключения |
| B12 | Эталонная группа Apple: захват metadata/mDNS/MRP до/после join/leave, ползунков, Pause/Play/seek | Датированные сопоставимые наблюдения; поля/типы/порядок и состав группы сравниваются с нашим стеком. Использовать прежний рабочий прокси, не изобретать новый перед записью | TODO, отдельное разрешение на захват |

Нельзя выдавать B02 за sample-for-sample, B01 за native adoption, B04 за
подтверждение clock domain. Для последнего нужны согласованные наблюдения всех
участников; правка gid наугад запрещена.

## C. Сбои и восстановление — отдельное согласование

| ID | Проверка | Приёмка | Связь v12 |
|---|---|---|---|
| C01 | Потеря приёмника на2s и30s | Остальные выходы/Станция живы; восстановление либо корректное удаление участника, без ghost volume | S3/S4/S7 |
| C02 | Потеря выбранных часов/смена роли/daemon restart | Старая lease отозвана, нет скрытого второго PTP/NTP fallback; локальный звук восстанавливает существующий guard | S5/S10 |
| C03 | Wi-Fi/сеть/DHCP, уход и возврат iPhone | Корректные teardown/reconnect и metadata; ограниченные очереди/повторы | S6 |
| C04 | Отказ/EOF/сбой renderer или sender, отмена выбора во время запуска | Stop+await и существующее recovery; прямой звук не отключается до готовности либо возвращается при ошибке; голос/BT сохранены | S12 |
| C05 | Перезагрузка/автозапуск; остановка вспомогательного Pi | Один экземпляр служб/владелец319/320, сохранённые identity/pairing, звук и BT автономны; после cold boot повторить A06 | S12 |
| C06 | Потери/дубли/reorder UDP, feedback под нагрузкой | Ограниченные retransmit/NACK и память; нет воспроизведения просроченных кадров | S13 |

Эти fault-сценарии не выполнялись13ноября. Не останавливать Apple TV, Bluetooth,
mediad, NQPTP или Pi только ради отметки в таблице без согласованного опыта.

## D. Приёмка после переключения Станции на ядро

Все A/B повторяются на точно указанном core pin. Сегодняшний PASS старого стека
не переносится автоматически в новую сборку.

| ID | Что требуется до отметки PASS |
|---|---|
| D01 | Проверенные host/frontend для Alice/AirPlay/BT; длинная пауза600s, seek/track/gap/takeover, bounded queues и source generation; уже потреблённые receiver buffers оцениваются отдельно |
| D02 | Один physical-clock owner, независимая аттестация daemon/роли/ABI, реальная RAW↔Instant correlation, свежесть/uncertainty; native/external peer interests и освобождение одного владельца без остановки остальных |
| D03 | PIN на iPhone: верный/неверный/отмена/timeout/закрытие карточки; повторное подключение по сохранённому ключу без повторного PIN; без transient fallback при ошибке |
| D04 | Импорт прежних identity/controller seed/accessory keys с точными physical UID; отказ перезаписи, приватность, reconnect после restart; никаких новых identity вместо переноса |
| D05 | Реальный renderer/stats/guard, точный набор получателей, calibrated local lead без повторных250ms; разделение network lead и местной калибровки |
| D06 | Пакет audio-only, pin/lock/hash/ABI/runtime closure; явный opt-in, частичный отказ установки, rollback и повторный старт с прежним backend |
| D07 | Точное соответствие MRP/mDNS/фактически выбранных outputs; readback громкости отдельно от requested; частичные metadata updates, stable IDs и native UI |
| D08 | Матрица iOS×приложение×AP1/AP2 profile×приёмник. Яндекс Музыка buffered-only — отдельный null receiver с записью фактического SETUP/codec до изменения рекламы; ALAC не обещается принудительно |
| D09 | ≥2ч: измеренные offset/jitter/drift, CPU/RSS/rootfs/log/queue growth, переходы seek/track, cold join. Цель — сэмпл-в-сэмпл, измерительная точность должна позволять её оценить; deadlines и слух не являются измерением выхода (S8/S9, D9) |
| D10 | ALSA timestamp actual type/accuracy, hw position/delay, DSP/выходная задержка и fallback; ASRC/drift при общей шкале. NIC timestamping/PHC только при фактической поддержке, они не синхронизируют ЦАП автоматически |
| D11 | Bit-exact lossless отдельно от timing: эталонные PCM/ALAC, gain/ASRC-политика явно описана; каналы/endianness/sample format/rate, отсутствие clipping; AAC оценивается как lossy |
| D12 | Долгий RTP wraparound, холодное присоединение к живой шкале, followed receiver/grandmaster profile, неподдержанный профиль — явный отказ/сохранение местного звука; ёмкость и >8 outputs проверяются на отдельном стенде (S9/S10/S14) |

D01–D12 пока BLOCKED для полной аппаратной приёмки: frontend/legacy control owner/
guard integration и разрешённая установка не завершены. Автономные прогоны не
заменяют эту приёмку.

## E. Будущие профили v12 — не запускать как готовые функции Станции

| Профиль/область | Независимая приёмка после реализации |
|---|---|
| AP1 RX | RAOP/RTSP, RSA/legacy authentication, PCM/ALAC/AAC по профилю, NTP/RTP/retransmit, DACP/Active-Remote feedback, reconnect/teardown |
| AP1 TX | Подключение к независимым RAOP-only приёмникам, encode/channel/rate, pacing/clock, volume/DACP, bounded32/64/session limits |
| AP2 realtime/buffered | Аутентификация, согласованный type96/type103, codec/timing profile, pause/flush/seek/source changes и фактический вывод; не скрещивать AP1/AP2 управление без явного bridge |
| MRP/conformance | §6 плана: framing/crypto/pairing vectors, namespaces/identifier, subscription, unknown fields, supported commands/queue/artwork/timeline, incremental topology/volume, malformed boundaries и контроль через независимый декодер |
| Modern mirroring RX | Pairing/SETUP/ключи/backchannels, decode audio/video и измеренный A/V sync, Pause/Resume/Stop, teardown/лимиты; R0–R7 исследования отдельно и на изолированных identities |
| Modern mirroring TX | Capture→encode→session→реальный приёмник, обратные подключения/firewall, A/V sync, commands/reconnect/cleanup; успех RX не закрывает TX |
| URL/HLS/photo без DRM | Отдельно URL, HLS, фото и V1/V2; точные capabilities, redirect/URL validation, остановка/ошибки/память; неподдержанные DRM/codecs явно отклоняются |
| Companion/Home app | Только согласованный профиль: pairing/reconnect/trust, TVRCSessionStart/SystemStatus где применимы; служба-заглушка и иконка HomePod не подтверждают поддержку |
| AWDL/P2P/классы пикера | Сначала отдельный стенд/identity и доказанная реализация; mDNS/BLE/interface scopes, фактический direct path, иконки/flags — наблюдения, а не догадки |
| Stress/security/CI | fmt+Clippy+workspace tests, audio-only+pyatv, FFmpeg, Windows/macOS/Linux, ARM; fuzz длины/framing/HTTP/plist/crypto, resource bounds, provenance и отсутствие секретов в журналах |

Источники норматива: [v12 §2/§5/§6](AGENT-PLAN-AP12-20261112-v12.md),
[групповая кнопка](GROUP-BUTTON-EXPERIMENTS.md), [профили](SESSION_PROFILES.md),
[измерения](AUDIO_RATE_ESTIMATOR.md), [MRP updates](CARD_UPDATES.md),
[PCM host](PCM_HOST.md), [continuous relay](CONTINUOUS_RELAY.md),
[авторизация](ROUTE_AUTHORIZATION.md), [зеркалирование](MIRRORING-AIRPLAY2-RESEARCH.md).
Станционные исходные сценарии: TESTING, LED, HOME_APP, AIRPLAY_PAUSED_SESSION,
AIRPLAY_GROUP_ENDPOINTS, DEVICE_LIST_FLICKER, LOCAL_SYNC*, RELAY_RENDER_LATENCY,
CORE_MIGRATION_INVENTORY, CORE_SENDER_ADAPTER и FUTURE из второго репозитория.

## Текущий журнал и доказательства

- Снимок только чтения13ноября: MRP executable SHA256
  `3e3df64b780aa9fbe6d1c72d110ca49790fc17224eab9d9e03dfc80d2c700581`;
  receiver `bae9d25afed4d08043735e581b243348ca8686f7dd5763ebacde868ad5490aec`;
  NQPTP `5f6f54ceaf02963f1baa85ea6846b5de7d2c80606ec8eb684a9d4ff6a6afa122`,
  sender SHM ABI10. Из совпадения ABI не выводится полная аттестация patch/role.
- `MINI2_GROUP_SYNC=1`, local offset0, прямые Mini2 Music/AirPlay/Bluetooth100%
  в момент снимка без выбранной ретрансляции. Свободно9.8MiB rootfs и27.4MiB data;
  MemAvailable около118MiB. Ничего не установлено/перезапущено.
- Музыкальный маршрут подтверждён секундным приглушением с независимым восстановлением;
  оба канала вернулись255/255. С разрешения пользователя и после его подтверждения
  создан только /run/mini2-group/verified-music для текущей загрузки.
  Маркеры AirPlay/Bluetooth не создавались; установки/перезапуска не было.
- На устройстве обнаружен /etc/init.d/S55source_broker.disabled; приватная шина
  /run/mini2-bt-source отсутствует. Бинарник source_broker установлен,
  но активация отключена. Это вероятный общий фактор A06/A08, не проверенный фикс.
- После снятия группы свежая проверка не нашла sender/local_player/source-reader
  и owner-music; оставшиеся stats — исторические файлы, не живые владельцы.
  При этом Music оставался0/0: после BT→native это привело к тишине.
  Сохранены инварианты отсутствия relay/owner, выполнен только возврат Music255/255
  штатным amixer через установленный timeout; пользователь подтвердил музыку.
  Bluetooth, общая громкость, конфигурации и службы не менялись.
  Существующее recovery в исходниках уже содержит restore; сравнить с установленным
  стеком, не писать второй механизм восстановления.
- При A08 receiver status=`Playing`, native control playerType=`bluetooth`,
  playing=false; led-owner отсутствует. Это наблюдение расхождения владельцев,
  не доказательство единственной причины всех дефектов.
- Локальные доказательства: STATION-ACCEPTANCE-READONLY-20261113.log,
  STATION-ACCEPTANCE-IDENTITY-20261113.log, STATION-CONTROL-BASELINE-20261113.log.
- Для каждого следующего результата дописывать: дата/сборка/источник/действие/
  ожидаемое/факт/ограничения. Ключи, PIN и токены в открытый журнал не добавлять.

## Реестр просмотренных документов

Срез до создания этого файла. SHA256 указан для точности источника; это не
вердикт его актуальности. История/WIP не переопределяют текущие решения пользователя.

| Репозиторий | Документ | SHA256 (префикс) |
|---|---|---|
| core | `README.md` | `9aac705a44f8` |
| core | `crates/mrp/proto/LICENSE-pyatv.md` | `015ff73f6090` |
| core | `crates/sender/PROVENANCE.md` | `ba80d436750a` |
| core | `crates/sender/README.md` | `c2e9542663c7` |
| core | `docs/AGENT-PLAN-AP12-20261112-v12.md` | `1e0b8f2cfef1` |
| core | `docs/AIRPLAY-WIRE-SPEC.md` | `a19b37c1d013` |
| core | `docs/ARCHITECTURE.md` | `00a8a6d11a1f` |
| core | `docs/AUDIO_RATE_ESTIMATOR.md` | `7eba281f4ff8` |
| core | `docs/BUILD_PROFILES.md` | `85ca0085beec` |
| core | `docs/CARD_UPDATES.md` | `1692c0eea6fe` |
| core | `docs/COMPONENTS-AND-NAMING.md` | `ec567532a60f` |
| core | `docs/CONTINUOUS_RELAY.md` | `f720c49f62c3` |
| core | `docs/DOCUMENT_MIGRATION_20261112.md` | `84bec2fa5cfb` |
| core | `docs/GROUP-BUTTON-EXPERIMENTS.md` | `6ac3f5393efb` |
| core | `docs/HANDOFF.md` | `37b7aa62b14d` |
| core | `docs/MIRRORING-AIRPLAY2-RESEARCH.md` | `0fa9a01603e4` |
| core | `docs/PCM_HOST.md` | `7aec4a223a1f` |
| core | `docs/PICKER-DEVICE-CLASSES.md` | `4144ccc8a3f9` |
| core | `docs/PROTOCOL.md` | `e4bdb6433640` |
| core | `docs/README.md` | `7e33b8c0c33c` |
| core | `docs/RELAY_IPC.md` | `9fd5d6cef935` |
| core | `docs/ROUTE_AUTHORIZATION.md` | `9e7047c41bb7` |
| core | `docs/SECURITY.md` | `e6a0c34e089b` |
| core | `docs/SESSION_PROFILES.md` | `4f9998af2352` |
| core | `docs/STATION_MIGRATION_20261111.md` | `24c86e38979f` |
| core | `docs/STATUS.md` | `78cd9b99c584` |
| core | `docs/VALIDATION.md` | `4d2eb30c9ee2` |
| core | `docs/history/AIRPLAY-ARCHITECTURE-AND-MIGRATION.md` | `48655bce7803` |
| core | `docs/history/AIRPLAY-PROTOCOL.md` | `a1866ff01714` |
| core | `docs/history/HANDOFF.md` | `fe5250f39d19` |
| core | `reference/station-relay-20261029/README.md` | `50c132261b16` |
| core | `reference/station-relay-20261029/local-sync-drafts/INTEGRATION.md` | `fe44e3bffcee` |
| station | `LICENSES/pyatv-MIT.md` | `d0c63aa9cb29` |
| station | `README.md` | `3bd5b7f45186` |
| station | `THIRD_PARTY.md` | `6cefcfbb8057` |
| station | `adapters/core-host/README.md` | `527bd8c5fa29` |
| station | `docs/AIRPLAY_GROUP_ENDPOINTS.md` | `7e7ecee76024` |
| station | `docs/AIRPLAY_PAUSED_SESSION.md` | `cdd61b351e4b` |
| station | `docs/ARCHITECTURE.md` | `e5af4ec1e9b0` |
| station | `docs/CLOCK_MIGRATION.md` | `8b51e1b6f268` |
| station | `docs/COMPONENTS.md` | `f0e73ef20e76` |
| station | `docs/CORE_MIGRATION_INVENTORY.md` | `6b9754c89000` |
| station | `docs/CORE_SENDER_ADAPTER.md` | `1d2349eec543` |
| station | `docs/DEVICE_LIST_FLICKER.md` | `251be0d11413` |
| station | `docs/FUTURE.md` | `1d0c68daec01` |
| station | `docs/GLAGOL_RELIABILITY.md` | `84367fc5110b` |
| station | `docs/HANDOFF_PR1.md` | `a1502468b95a` |
| station | `docs/HOMEPOD_PICKER_HANDOFF.md` | `44c135355dd9` |
| station | `docs/HOME_APP.md` | `e66d61c9afe6` |
| station | `docs/LED.md` | `08c93eb1808f` |
| station | `docs/LOCAL_SYNC.md` | `b0393af73bff` |
| station | `docs/LOCAL_SYNC_CANDIDATE.md` | `9389e4991f18` |
| station | `docs/LOCAL_SYNC_ROUTE_RESTORE.md` | `d3c7ab632c3f` |
| station | `docs/MAIND_RE.md` | `793f7eda7d1b` |
| station | `docs/MQTT.md` | `6c5257444e77` |
| station | `docs/MRP_DRAFT_REVIEW_20261111.md` | `88cc8d93c4e6` |
| station | `docs/MRP_GROUPS.md` | `07a3650683bb` |
| station | `docs/NETWORK_SOURCE_AUDIT.md` | `92eaddcddca5` |
| station | `docs/OUTPUT_NOTIFICATIONS.md` | `affe7d2d7049` |
| station | `docs/RELAY_RENDER_LATENCY.md` | `ac985b077772` |
| station | `docs/RELAY_TIMING_CANDIDATE.md` | `0a25db1a0956` |
| station | `docs/RUNTIME.md` | `0b9998a69bbc` |
| station | `docs/SETUP.md` | `aa91d2b8e040` |
| station | `docs/STATUS.md` | `f0cee9117d7a` |
| station | `docs/STREAMING_READER_AUDIT.md` | `523217dfc5b9` |
| station | `docs/SYSTEM_CHANGES.md` | `b83661704f1d` |
| station | `docs/TESTING.md` | `14c4ad16c37a` |
| station | `docs/VALIDATION.md` | `43e47e75786e` |
| station | `pi/mosquitto/README.md` | `4529e2e59f5a` |
| station | `src/airplay_mrp/tests/core_compat/README.md` | `1ef30a42af67` |
| station | `wip/codex-homepod-routes/README.md` | `56f6ac4d4071` |
| station | `wip/codex-homepod-routes/ROUTING-WORK-IN-PROGRESS.md` | `376b06650757` |
| station | `wip/codex-homepod-routes/agents/local-sync/INTEGRATION.md` | `a80e74223335` |
