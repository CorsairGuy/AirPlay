# Continuous relay: состояние источника

12ноября2026. Общий статус — [STATUS](STATUS.md).

`sender::continuous::Timeline` — чистый source-side автомат без I/O. Он получает ту же эпоху Instant, что MediaClock; один экземпляр на источник группы, не на получателя.

Перенесены gap grace40мс с последующей тишиной, пауза600с без продления повторным Pause, Resume без новой эпохи, отдельные поколения источника и пропуск слотов с чрезмерным опозданием. Допуск равен lead минус150мс, минимум100мс. PCM — целые S16BE/stereo/44.1kHz пакеты по352frames; неверный input не меняет позицию.

База — поздний Station sender ld5dy4 a538a30: continuous reader/GAP_FILL_AFTER_NS/lateness и pause window контроллера. Глобальная GENERATION и magic stdin marker в библиотеку не перенесены.

## Контракт

Step::Wait сохраняет input до указанного Instant. Frame/DiscardStale/Skip потребляют input. Frame штампуется один раз существующим MediaClock или PtpProducer, затем отправляется общей local/remote очереди. Timeline не меняет RTP/crypto state и не создаёт часов.

PauseExpired передаётся владельцу; чистый автомат не делает TEARDOWN. invalidate_source не является сетевым FLUSH и не удаляет уже принятые receiver/local buffers. Новое поколение не прыгает по media timeline.

## Асинхронная подача PCM

`sender::continuous_source::run` подключает Timeline к bounded input/output mpsc и отдельному watch State(generation, paused). Сначала публикуется новое control state, затем PCM его поколения; регресс поколений отклоняется. Watch не стоит за очередью декодера. Закрытие PCM означает окончательный EOF; между треками Sender сохраняется живым. Закрытие control owner или stop означает отмену.

Pump удерживает не более одного pending input и одного scheduled output; каждая очередь ограничена512 элементами. Пустой источник заполняется тишиной; выходной stall имеет deadline, который не продлевается обновлениями управления. При Pause/смене поколения ещё не переданный output заменяется тишиной. Уже переданные в downstream очередь кадры требуют отдельного generation guard.

На выходе ScheduledFrame с index/generation/PCM; его ещё должен один раз штамповать общий MediaClock/PTP producer. Pump не создаёт часов, сессий, фоновых задач или сетевого управления.

## Общая рассылка и защита очередей

`continuous_source::broadcast` связывает pump с общей packet broadcast через bounded промежуточную очередь8 кадров. Callback штампует кадр существующим MediaClock или PtpProducer; проверяются тот же epoch/index/PCM. Получатели получают один immutable Arc, не независимые штампы времени. Финальный EOF дренирует принятые scheduled frames; stop/error отменяет pump/forward без скрытых spawned tasks.

ScheduledFrame и AudioPacket сохраняют версию source-control watch, а не только число поколения. Любое обновление управления делает старую версию stale, даже после Pause→Resume с возвратом к прежним значениям. Закрытие владельца также инвалидирует кадры. Caller должен подавлять лишние уведомления с одинаковым состоянием, если не хочет инвалидировать очередь.

Remote Session пропускает stale до pacing/проверки срока и повторно перед packetization: RTP sequence/nonce для таких кадров не расходуются. Local run_local подаёт тишину на прежнее presentation time, а уже просроченный stale кадр пропускает. LocalPcmPipe повторно проверяет revision при сериализации. Исходный PCM для других потребителей не меняется; legacy packets без guard сохраняют прежнее поведение.

Это защита ещё не потреблённых host queues. Она не отзывает уже принятые hardware/receiver buffers и не заменяет согласованные сетевые pause/flush/seek операции. Защита retransmit cache добавлена отдельно ниже.

## Остаётся

- Подключение готового broadcast API к рабочему frontend/PtpGroup и реальным source/control adapters.
- Согласование receiver/hardware buffers и control операций при переходах состояния.
- Сетевые pause/seek/track операции и tolerance действующей Session. Исторические FLUSH/restart варианты требуют отдельной сверки.
- Debounce краткого !playing между треками и реальные interop/нагрузочные проверки.

Это ещё не включённая continuous relay-функция. Установка и звук на Станции не запускались.

Проверка:68 sender library tests passed (63 прежних +5 continuous-policy cases) в автономном контейнере Pi. Проверены gap silence, неизменная эпоха pause/resume, stale/future generation, lateness и pause expiry. Это не физическая или полная async/network проверка.

Следующий автономный прогон:73 sender library tests passed. Пять async-сценариев проверяют gap→content→EOF, Pause/поколения поверх PCM backlog, отмену/stall при полном выходе, смену управления во время ожидания capacity и закрытие control owner/регресс поколения.

Дополнение к контракту: stale scheduled slots не удаляются из общей рассылки до local/remote разветвления — иначе локальная ветка потеряет временной слот. Они сохраняют guard; remote пропускает их, local заполняет тишиной. Отдельный тест покрывает coalesced Pause→Resume во время ожидания capacity: pending PCM не становится снова актуальным. Custom sinks с собственными очередями должны проверять source_is_current при фактическом потреблении; уже переданные hardware/network buffers не отзываются автоматически.

Проверка:78 sender library tests passed. Новые случаи: один shared Arc/один stamp, неверная эпоха до рассылки, невосстановимость старой revision, локальная тишина на прежнем deadline и сохранение stale scheduled slot. Зашифрованный PTP loopback дополнен stale future frame: он не расходует RTP/nonce и не ломает pacing; mock по-прежнему получает ровно два актуальных audio packets.

## Retransmit cache при смене источника

PcmPacketizer::clear_retransmit_cache удаляет только сохранённый ciphertext, не сбрасывая sequence/nonce, media origin или first-marker state. NTP/PTP wrappers используют один контракт. Session очищает cache перед первым новым source revision; если revision старого отправленного кадра уже invalid, поздний retransmit запрос не получает старые байты даже до поступления нового PCM. Legacy unmanaged packets сохраняют прежнее поведение.

Сверка поздней станционной ветки: native mini2_music tap переносит pause/seek/track в той же сессии; старые варианты FLUSH/restart не являются универсальным текущим рецептом. Этот cache guard не отправляет самодельный RTSP PAUSE или FLUSH и не объявляет завершённой отмену remote/hardware buffers.

Проверка:94 library tests passed (RTP16 + sender78). Новый RTP regression очищает cache на переходе sequence65535→0 и подтверждает nonce1, прежний media origin, отсутствие повторного marker и точный ciphertext нового retransmit. Sender regression проверяет различение прежней/новой source revision; существующий encrypted PTP loopback также прошёл.
