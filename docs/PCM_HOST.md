# Библиотечный host для существующего PCM-tap

PtpGroup::run_card_pcm_with_local принимает Card, RawPcmSource, GroupOutputs и LocalSink. Он соединяет read_pcm, источник continuous-потока, observer карточки, qualified PTP pool и локальный renderer в одном ожидаемом future. Общие часы и PtpGroup создаются владельцем заранее; host не запускает clock/discovery/device services или дочерние процессы.

RawPcmSource содержит reader, явную media epoch, latency250мс..10с и queue_capacity1..512. Вход — чистый S16BE/44.1kHz/stereo от уже выбранного источника, без служебных маркеров. Чтение не меняет endian, не декодирует и не делает ASRC. Источник и MediaClock используют одну epoch; сеть и локальный sink получают общий поток.

read_pcm хранит один частичный пакет352фрейма, сохраняет границы при дробном чтении и смене control revision. Frame-aligned последний хвост дополняется нулями; оборванный стереофрейм — ошибка. Между треками владелец сохраняет reader открытым; EOF означает окончательное завершение источника. Backpressure ограничен ёмкостью канала и отменяется stop.

SourceFrame::guarded фиксирует revision при получении PCM. Pump отвергает другой control channel и отбрасывает устаревшую очередь даже при coalesced Pause→Play без смены generation. SourceFrame::new оставляет адаптеру управление собственной очередью. Struct literals заменены конструкторами: это изменение Rust API до миграции адаптера.

Guard относится к кадрам, уже принятым этим API. Он не определяет возраст байтов, оставшихся внутри upstream tap/pipe, и не отзывает уже переданные hardware/receiver buffers. Адаптер отдельно согласует source events, буферы tap и фактическую смену контента.

EOF дренирует pipeline и вызывает finish sink; отмена/ошибка освобождает sink через Drop без проигрывания накопленной очереди. Ошибка чтения или sink, stop и окончание pipeline отменяют остальные ветки; host дожидается cleanup, включая reader и Card observer. Закрытие consumer означает Stopped. Для штатного завершения требуется stop + await. Маршруты, восстановление прямого вывода, ALSA latency/gain и выбранный native/AirPlay/BT источник остаются у адаптера; голос Алисы в этот content tap не включается.

Это opt-in библиотечная композиция. Station outputs.rs не переключён, CLI send остаётся отдельным NTP-путём. Следующая интеграция — запуск этого API в адаптере с существующими tap/renderer/route guards, метаданными и фактическим shared-clock owner.


## Adapter calibration (2026-11-13)

`PtpGroup::run_card_pcm_with_local_lead` accepts a separate local presentation lead
(250ms..10s). `RawPcmSource::latency` continues to set source scheduling tolerance;
local calibration does not shift its epoch, RTP/PTP stamps or receiver session lead.
The original `run_card_pcm_with_local` remains a wrapper using equal leads.
A caller still supplies negotiated receiver configuration and a qualified shared-clock
group; this API does not start clocks, select targets, mute routes or prove readiness.


External daemon profile: publish qualified cached observations through
`timesync::external_media`, construct `PtpGroup::new_external`, then use the same
raw-PCM/local pipeline. Owner supplies attested role/daemon instance, scheduled
reader/correlation updates and timing-peer control. No synthetic local Service
snapshot or second319/320 listener. Adapter default/frontend remain unchanged.

External peer ownership (2026-11-13): PtpGroup::with_external_peer_control connects
an already-owned external daemon controller to the same QualifiedPreparer active/
finish lifecycle as the built-in Service. Updates include pinned Domain and fresh
nonzero owner token; external binding cannot relabel a local clock group or
replace an existing binding. Calls are bounded to1s. Cleanup submits an empty
interest for that owner, not daemon shutdown. The supplied controller MUST reject
stale domains, preserve other owners' interests and serialize withdrawal after
partially applied/cancelled updates. This interface does not implement/attest a
legacy NQPTP control controller; no command/socket is automatically started.
