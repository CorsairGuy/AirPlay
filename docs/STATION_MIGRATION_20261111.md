# Migration checkpoint, 11 November 2026

Target remains a device-independent engine. Station-specific Quasar, ALSA capture/output, LED and installation stay in the Station adapter. Do not copy hard-coded paths, Station names, process control or NQPTP shared-memory assumptions into protocol crates.

## First port: receiver render minimum

Station cf1894c corrected double-counting250ms and user confirmed acoustic alignment on4 November. The core sender still applied the full total lead in sync plus the requested receiver minimum. Port adds a validated packetizer constructor with a separate render minimum; Session parses stream SETUP latencyMin, falling back to the11025 frames it requested. Total local deadline and encrypted audio bytes remain unchanged. Legacy low-level constructor semantics remain unchanged. This is NTP realtime, not the later PTP/continuous/ALSA-tap port. No installation or push.

## Remaining port inventory

| Area | Station source | Core status / next dependency |
|---|---|---|
| Persistent pairing, native PIN UI | station-onepath29dcc54..13cf9f2; remote equivalents | Core pair primitives exist; sender persistence and MRP59/60/61 orchestration need audit/port |
| Discovery, logical groups, cluster membership and volume | station-onepath group commits; remote outputs.rs | Core Coordinator exists but is inactive; core MRP Feed currently has no output membership. Port logical/physical mapping before claiming button support |
| PTP sender + participant list |6226d27..e316dbd and remote equivalents | Core clocks/coordinator primitives exist; runtime wiring and sender negotiation remain. Separate NQPTP smi10/smi11 adapters from core |
| Continuous pause/seek/track sessions |8c349ad..37e4d66, remote outputs/sender | Port state machine without Station process/file IPC; preserve generation and nonce/timeline invariants |
| One native PCM source |bc9cda1; remote7e5869c/3e33824 | Keep mini2tap/reader as Station capture adapter; core takes source-owned PCM, not CDN credentials |
| Recovery and autostart | existing guard/recovery, S90group-sync, a538a30 | Existing mechanisms are implemented, not new tasks; retain in adapter and verify integration |

## Group-button evidence

Fetched existing snapshots from Pi only; no device connections or control. Reduced descriptors saved in workspace outputs/group-button-20261111. Snapshot station-ht-0633 already contains logicalDeviceCount2, groupedDevices and selected Home Theater descriptor while user later still reports single-name button. Therefore adding a name or logical count alone is not a proven fix. hp121-stationsess-0633 still advertises its own theater group, whereas Station advertises its own relay group; these are historical captures predating subsequent PTP work and do not prove the current receiver state. Need date-matched MRP/mDNS group observations or an existing later capture before changing group IDs. Do not equate sound synchronization with topology adoption.

## Constraints

User is away and cannot hear audio. Read-only diagnosis and offline tests only; no Station install/restart/playback without new approval. No GitHub push without approval. Preserve unrelated pre-existing dirty files events.rs, audio_stream/net.rs and clock tests.
