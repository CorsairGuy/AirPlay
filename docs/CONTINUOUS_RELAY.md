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

## Остаётся

- Подключение async pump к frontend/PtpGroup и общей packet/broadcast очереди.
- Поколения в remote/local очередях и их согласованная очистка/тишина.
- Сетевые pause/seek/track операции и tolerance действующей Session. Исторические FLUSH/restart варианты требуют отдельной сверки.
- Debounce краткого !playing между треками и реальные interop/нагрузочные проверки.

Это ещё не включённая continuous relay-функция. Установка и звук на Станции не запускались.

Проверка:68 sender library tests passed (63 прежних +5 continuous-policy cases) в автономном контейнере Pi. Проверены gap silence, неизменная эпоха pause/resume, stale/future generation, lateness и pause expiry. Это не физическая или полная async/network проверка.

Следующий автономный прогон:73 sender library tests passed. Пять async-сценариев проверяют gap→content→EOF, Pause/поколения поверх PCM backlog, отмену/stall при полном выходе, смену управления во время ожидания capacity и закрытие control owner/регресс поколения.
