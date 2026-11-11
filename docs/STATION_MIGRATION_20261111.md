# Migration checkpoint, 11 November 2026

Target remains a device-independent engine. Station-specific Quasar, ALSA capture/output, LED and installation stay in the Station adapter. Do not copy hard-coded paths, Station names, process control or NQPTP shared-memory assumptions into protocol crates.

## First port: receiver render minimum

Station cf1894c corrected double-counting250ms and user confirmed acoustic alignment on4 November. The core sender still applied the full total lead in sync plus the requested receiver minimum. Port adds a validated packetizer constructor with a separate render minimum; Session parses stream SETUP latencyMin, falling back to the11025 frames it requested. Total local deadline and encrypted audio bytes remain unchanged. Legacy low-level constructor semantics remain unchanged. This is NTP realtime, not the later PTP/continuous/ALSA-tap port. No installation or push.

## Remaining port inventory

Second port: SessionConfig.preserve_receiver_volume defaults false for compatibility. When enabled, SETUP sends no initial volume; the run control worker waits for an explicit watch revision, including one equal to the initial placeholder. A queued final request is retained even when its writer closes. Coalescing and RTSP/media isolation remain. Offline RTP/sender tests:43 passed. This is a library policy only; CLI/group adapter wiring remains to be migrated.

| Area | Station source | Core status / next dependency |
|---|---|---|
| Persistent pairing, native PIN UI | station-onepath29dcc54..13cf9f2; remote equivalents | Sender accepts saved credentials and performs pair-verify; PinPairing begin/finish implements first setup. Persistence adapter and MRP59/60/61 orchestration remain |
| Discovery, logical groups, cluster membership and volume | station-onepath group commits; remote outputs.rs | Core Coordinator exists but is inactive; core MRP Feed currently has no output membership. Port logical/physical mapping before claiming button support |
| PTP sender + participant list |6226d27..e316dbd and remote equivalents | Core clocks/coordinator primitives exist; runtime wiring and sender negotiation remain. Separate NQPTP smi10/smi11 adapters from core |
| Continuous pause/seek/track sessions |8c349ad..37e4d66, remote outputs/sender | Port state machine without Station process/file IPC; preserve generation and nonce/timeline invariants |
| One native PCM source |bc9cda1; remote7e5869c/3e33824 | Keep mini2tap/reader as Station capture adapter; core takes source-owned PCM, not CDN credentials |
| Recovery and autostart | existing guard/recovery, S90group-sync, a538a30 | Existing mechanisms are implemented, not new tasks; retain in adapter and verify integration |

## Group-button evidence

Fetched existing snapshots from Pi only; no device connections or control. Reduced descriptors saved in workspace outputs/group-button-20261111. Snapshot station-ht-0633 already contains logicalDeviceCount2, groupedDevices and selected Home Theater descriptor while user later still reports single-name button. Therefore adding a name or logical count alone is not a proven fix. hp121-stationsess-0633 still advertises its own theater group, whereas Station advertises its own relay group; these are historical captures predating subsequent PTP work and do not prove the current receiver state. Need date-matched MRP/mDNS group observations or an existing later capture before changing group IDs. Do not equate sound synchronization with topology adoption.

## Deferred live check: Yandex Music buffered-only

At home, use a separate silent diagnostic receiver, not the working Station. Record a baseline Yandex Music SETUP, including stream type and codec. Then advertise/support buffered-only and repeat with the same iPhone/app/track. Record either type103 negotiation and successful decoding or rejection/no media SETUP; do not assume the sender must fall back. Compare with Apple Music to distinguish an AirPlay stream from native HomePod playback. Verify pause/play and timeline on a second iPhone. Do not enable microphone capture, audible output, alter Station advertisements, or claim ALAC from type96 alone. User requested this check be added to the plan on11 November; no live test performed while user is away.

## Constraints

User is away and cannot hear audio. Read-only diagnosis and offline tests only; no Station install/restart/playback without new approval. No GitHub push without approval. Preserve unrelated pre-existing dirty files events.rs, audio_stream/net.rs and clock tests.

Third port: SRP M3 proof computation now runs in Tokio's blocking pool, with a bounded caller deadline, retaining proof state and receiver proof verification. It does not own the RTSP connection; cancellation drops that connection while finite CPU work can finish separately. Real transient pairing/key agreement test on a current-thread runtime passed; RTP/sender total44 tests passed. Persistent pairing/PIN UI and PTP runtime wiring remain separate ports.

Fourth port: caller-supplied PairingCredentials selects pair-verify/HKP3 without PIN start or pair-setup; absent credentials keeps transient/HKP4. Credentials Debug is redacted, no hidden fallback on authentication failure, shared secret stays fresh per verify handshake. Core ControllerVerify validates M2/M4 states. Loopback exchange authenticates against a pinned receiver key and checks matching channel keys; malformed/incorrect M4 states rejected. Pair/RTP/sender54 tests passed. No live device test or installation. This is reconnect support, not the complete first-time PIN UX or persistent storage implementation.

Fifth port: public PinPairing::begin starts HKP3 PIN setup and keeps its TCP connection while the application obtains the displayed code; finish(pin) performs offloaded SRP, verifies server proof and signed accessory credentials, then returns PairingCredentials. No SETUP/RECORD/audio, implicit PIN or file writes. Dropping the pending exchange closes its connection. Adapter must handle UI cancellation, input timeout and secure persistence. Loopback tests cover real successful PIN pairing, wrong PIN with zero registered controllers and invalid parameters before any connection. Pair/RTP/sender56 tests passed. Native iPhone MRP authorization UI is still not wired.

Sixth port: ap_mrp::authorization owns route PIN prompt59, response60 and result61. Added source proto descriptors and regenerated schema (extension tags63/64/65). Numeric PIN parsing distinguishes cancelled/empty input from invalid UTF8, nonnumeric and oversized input; Debug hides code. Response preserves optional request identifier so the connection owner can correlate replies; no overlapping prompts or identifier-less reply policy is assumed by this wire layer. Wire-tag/roundtrip/cancellation/error tests passed. Total MRP/pair/RTP/sender66 tests passed. This does NOT yet show a PIN sheet from the active core card: its route-selection/session coordinator and connection-scoped pending-request lifetime still need wiring. Reuse this module instead of duplicating wire builders in the legacy station_mrp adapter.

Next bridge: bin/airplay::route_pairing::authorize links PinPairing to ap_mrp messages using private per-request controller channels. It starts receiver PIN setup before prompting, validates response identifiers when supplied, returns verified credentials, reports result and bounds both the full operation and final status write. Cancellation drops pending receiver connection; no playback/session creation or persistence occurs. This public adapter helper is not yet invoked by the core card's ModifyOutputContext path, whose remote output backend is still absent. Do not claim the PIN sheet appears on iPhone until that integration and live test exist.

Bridge validation: all43 application-library tests passed from an exact committed-source archive plus the new bridge (unrelated dirty events/net/clock edits excluded). Full RTSP/SRP loopback covers successful PIN, cancelled sheet, controller reply-channel disconnect, external stop, and rejection of a different request's PIN reply. No SETUP/RECORD permitted in the test peer. Output remains an adapter API; connecting Card output selection and secure credentials storage is still pending.
