# Continuous relay: состояние источника

12ноября2026. Общий статус — [STATUS](STATUS.md).

`sender::continuous::Timeline` — чистый source-side автомат без I/O. Он получает ту же эпоху Instant, что MediaClock; один экземпляр на источник группы, не на получателя.

Перенесены gap grace40мс с последующей тишиной, пауза600с без продления повторным Pause, Resume без новой эпохи, отдельные поколения источника и пропуск слотов с чрезмерным опозданием. Допуск равен lead минус150мс, минимум100мс. PCM — целые S16BE/stereo/44.1kHz пакеты по352frames; неверный input не меняет позицию.

База — поздний Station sender ld5dy4 a538a30: continuous reader/GAP_FILL_AFTER_NS/lateness и pause window контроллера. Глобальная GENERATION и magic stdin marker в библиотеку не перенесены.

## Контракт

Step::Wait сохраняет input до указанного Instant. Frame/DiscardStale/Skip потребляют input. Frame штампуется один раз существующим MediaClock или PtpProducer, затем отправляется общей local/remote очереди. Timeline не меняет RTP/crypto state и не создаёт часов.

PauseExpired передаётся владельцу; чистый автомат не делает TEARDOWN. invalidate_source не является сетевым FLUSH и не удаляет уже принятые receiver/local buffers. Новое поколение не прыгает по media timeline.

## Остаётся

- Async source pump и подключение к frontend/PtpGroup.
- Поколения в remote/local очередях и их согласованная очистка/тишина.
- Сетевые pause/seek/track операции и tolerance действующей Session. Исторические FLUSH/restart варианты требуют отдельной сверки.
- Debounce краткого !playing между треками и реальные interop/нагрузочные проверки.

Это ещё не включённая continuous relay-функция. Установка и звук на Станции не запускались.

Проверка:68 sender library tests passed (63 прежних +5 continuous-policy cases) в автономном контейнере Pi. Проверены gap silence, неизменная эпоха pause/resume, stale/future generation, lateness и pause expiry. Это не физическая или полная async/network проверка.
