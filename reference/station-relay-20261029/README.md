# Historical Station relay work (28–29 October 2026)

Preserved source reference, NOT a Cargo workspace member or a deployed component.
The reusable engine remains in crates/sender, crates/rtp, clock and audio_stream.
These snapshots retain device-specific adapter work for later extraction; do not
link Station paths, Alice services, ALSA taps or shell supervisors into core APIs.

Provenance: CorsairGuy/yandex_station_airplay branch codex/homepod-routes-wip,
commit 1c28920, plus original Pi source folders /root/work/codex-routes. Content
hashes are in MANIFEST.json. Sources only: no binaries, pairing credentials,
private stream URLs/keys, runtime caches or recorded user traffic.

- early-mrp: early output picker and content/source-reader orchestration.
- late-mrp: v5 + native-group era source; not declared fully working.
- sender-volume and sender-volume-review: historical sender and volume changes.
- capture-helpers: separate ALSA source taps and Alice media reader.
- local-sync-drafts: unpublished local timing/guard work, if available.

User-confirmed earlier: Alice music audible on a real HomePod from Station;
selection, deselection and individual volume worked. Not established as complete:
acoustic sync, long-term stability, AirPlay/Bluetooth relay validation.

A real deployment on 29 October was rolled back after audio/Alice regressions.
Do not replay old installation scripts: they used RAM-backed /run for binaries.
The source contains full-track buffering and other unfinished behavior; preserving
it is not acceptance of those choices for the universal engine. Migration must
use bounded streaming and typed, device-independent source/sink/volume contracts.

Original Alice/Bluetooth integration remains the responsibility of the Station
adapter. This archive does not advertise extra engine capabilities or change the
build. It was saved locally at the user's request; no GitHub publication implied.
