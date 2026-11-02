# Partial card updates and cached artwork

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
