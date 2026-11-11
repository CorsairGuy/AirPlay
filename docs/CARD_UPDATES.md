# Partial card updates and cached artwork

<!-- current-status-20261112 -->
> Сверено12ноября2026. Текущий статус и ограничения — [STATUS](STATUS.md). Датированные замеры и этапы ниже сохранены как история, их слова «ещё не реализовано/не установлено» относятся к указанному этапу, а не автоматически к нынешнему main.


DeviceInfoUpdate now changes its existing type field in place, keeping field1
first. Although protobuf permits other field orders, the standard pyatv AirPlay
decoder requires leading0x08 and otherwise discards following messages in the
same packet. An independent full-packet regression reproduces that loss and
decodes all five corrected group-update messages.42AirPlay tests passed. The
effect on iPhone selected-row blinking remains a physical test, not an assertion
based only on pyatv behavior.

Correction: the first candidate's truncated queue inside SetState was rejected
after a physical Station trial broke Play/Next; rollback restored the controls.
An independent pyatv replay proves that this shape replaces the queue and loses
existing item metadata. The corrected implementation uses queue-free SetState
plus UpdateContentItem56(extension60) to merge the existing item's timeline.
Rust fixtures replayed through pyatv preserve title/artwork/archive/commands and
Pause/Play state. This explains a concrete defect; iPhone control recovery on
the corrected candidate remains physically unverified.

The older description below must be read with that correction: timeline items
are no longer sent inside a reduced SetState.PlaybackQueue.

The card receives receiver events, not a generic downstream MRP player-state
stream. Its DMAP metadata updates can omit fields: omission preserves current
fields, while a different title clears the old item's missing metadata, artwork,
duration and elapsed position before applying the new values. Same-title item
disambiguation remains limited by the sender's metadata; this is not a complete
track identity protocol.

For an already announced item, Pause/Play and position updates now send partial
SetState data containing playerPath, playback state/timestamp and item identifier
with elapsedTime/playbackRate/elapsedTimeTimestamp. Artwork availability, artwork
identifier, archive and bytes are omitted. Full metadata/cover changes still
send full state; artwork queue requests still return bytes when requested.

The goal is to avoid invalidating a loaded cover on Pause/Play without losing
artwork when opening the card during an existing track. Tests check preservation
of metadata across partial updates, reset on a new title, artwork-free playback
patches and a full late-join artwork request. This does not prove that every
cause of iPhone artwork flicker has been fixed, nor that external notifications
arrive instantly. Physical verification on Station is pending.

Reference capture on actual HomePod shows state and metadata arriving separately:
pause-rate metadata preceded state by about0.8s; Play state preceded timeline by
about2s in the sampled session. These are inter-message reception intervals,
not measured user-interface latency. Do not turn absent fields into resets or
invent a blanket timestamp filter for receiver events that lack source timestamps.
