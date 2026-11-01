# Review candidate: group local playback

Prepared without Station mutations or playback. Pi copies are in
`/root/work/codex-routes/agents/local-sync`; Windows source in this directory.

## Candidate files and validation

`prepare_local_sync.py` accepts Claude/current/output directories and writes copies
only. It refuses mismatched source text. Generated local_player provides atomic
200ms heartbeat with player PID/startticks, ready flag after first content time,
CLOCK_MONOTONIC timeline plus CLOCK_BOOTTIME heartbeat, and drops rather than drains
on shutdown. Generated readers use unity gain, duck 0.4, final S16 saturation.

`group-sync-guard.sh` has no install/prepare command. Requires independently verified
source route marker, sender PID identity, valid player identity, fresh heartbeat,
20 records and error within 20ms before muting. A per-source mkdir ownership lock
prevents old cleanup unmuting a new generation. Unique stats path is mandatory.
On sender/player loss, stale heartbeat, or guard TERM it restores direct source.
Its active file means a handoff happened; it alone is NOT current sync proof.

ARMHF compile with existing mini2-airplay-armhf passed -Wall -Wextra -Werror.
300 zero records through explicit ALSA `null` in no-network/device-free container
passed protocol/identity/heartbeat/EOF checks. Null PCM has no physical sample
clock, so this does NOT validate acoustic alignment or drift control. Mock mixer
checks passed sender death, player death, guard TERM, each mute0/restore100 plus
ownership cleanup. Nothing was installed on Station.

## Required integration in root-owned outputs.rs

Keep current sender-volume and asynchronous slider worker. Do NOT deploy PR1
outputs.rs or replace sender wholesale. Port local-output support into sender-volume.
Map native to guard source `music`, other source names unchanged. Feature gate
defaults OFF; offset must be validated finite integer, e.g. -200..2000ms. Keep
latency 500ms initially. Local process and sender run on Station, same MONOTONIC.

Use a unique per-relay stats path under a private run directory and local command:

```
exec /data/local/opt/airplay-armhf-control/lib/ld-linux-armhf.so.3 --library-path /data/local/opt/airplay-armhf-control/lib:/usr/lib:/lib /data/local/opt/group-sync/local_player mini2_group UNIQUE_STATS_PATH
```

Pass --local-cmd and --local-offset-ms. Spawn guard only after every selected
receiver acknowledges actual media readiness on the shared timeline, not just
RTSP RECORD or existing `streaming` log. Await guard `group-sync: active` while
also monitoring child exits; retain the guard Child in Relay. On every startup
failure, source/track change, deselect, pause, shutdown or ended path, stop guard
and await it before starting another relay. Guard TERM triggers sender TERM and
restore; wait children and apply kill deadline as existing stop does. Never hold
an owner lock after its process has exited.

Advertise active sync only while sender identity alive, guard alive/owner matches,
player identity alive, fresh heartbeat and error bound hold, and all receiver
state reports remain fresh. Distinguish selected outputs / connecting / active /
fallback. Read current stats, not desired config or the stale active file alone.
Guard failure must clear active and tear down delayed copy; direct native output
continues. On control-interface startup, stale owner recovery needs a startup
restore/reconciliation step before accepting any new relay. SIGKILL cannot run
shell traps: an independent supervisor must restore on guard/controller loss.

## Blocking sender timing defect in Claude implementation

Claude writes local play_at=packet.produced_ns+latency+offset, but receiver RTP/NTP
head starts before RECORD/FLUSH/volume, and advances only by frame count. Packet
production time does not enter receiver timing. Draining pairing backlog does not
align these timelines. Independent receiver handshake/reconnect times can create
different offsets. Fix with a process-wide media epoch: NTP and monotonic anchors,
shared packet frame index (or fixed-rate packet presentation timestamp), and map
that same media position to RTP/NTP for every target and local timestamp. Explicitly
rebase/drop queued packets consistently after reconnect; do not hide this behind
a fixed local offset. Prove header/control mapping with offline decoded packets
before acoustic calibration. Ensure local mpsc backpressure cannot stall network
indefinitely; timeout/fail closed to delayed copy and restore direct route.

## Installation / calibration order (root only, scheduled)

1. Archive known working config, binaries and boot entries. Preserve Bluetooth
   service command, native output, and original ALSA config. Feature OFF.
2. Verify stock quasar SHA, generated structural diff, supported libasound and
   separate pipeline property inheritance on real firmware. Claude per-type SINK
   override is an assumption, not yet established. Never mute unverified routing.
3. Install candidate dependencies/files inactive, then a controlled RAM overlay
   with rollback. Initialize softvol controls at 100 before native player opens.
   Restart mediad/quasar only during the agreed setup window. Inspect route per
   source. Verify Alice speech still default; test Bluetooth first and rollback
   immediately on missing local sound. Existing source observer does not reroute.
4. Validate each source route independently before creating verified-source marker.
   Bluetooth uses bluealsa-aplay -d mini2_bluetooth. AirPlay airplay_duck slave must
   traverse mini2_airplay; merely appending definitions changes nothing. Watch
   for double duck if copied PCM already traverses a duck gain stage.
5. Enable local sync for one source, with original route still audible until guard
   activation. Induce sender/localplayer/controller/guard exits and input stalls;
   verify restore. Then repeat source switch/pause/resume/track change and both pods.
6. With user's agreed test level, equal-distance microphone recording of clicks,
   measure pair offsets. Positive local offset delays Station further; if Station
   click leads HomePod by 40ms, add +40ms. Repeat each pod, reconnect and long run.
   Account for sound propagation (~2.9ms per metre). A fixed offset is calibration,
   not evidence of a common receiver media clock.

## Important limits

ALSA official pcm_softvol.c handles cur_vol==0 by snd_pcm_areas_silence before dB
table conversion; 0% is mute in that implementation despite default min_dB=-51.
Do not change min_dB to extreme values. Check Station's actual implementation
using isolated null/file slave if needed, not by touching live controls.
Source: https://raw.githubusercontent.com/alsa-project/alsa-lib/master/src/pcm/pcm_softvol.c

Unity-gain readers raise outgoing amplitude 12dB relative to existing 0.25 and
remove deliberate clipping. Roll them out with coordinated receiver volume only;
do not silently replace current readers independently of the sync feature.

Reader-baked duck arrives with delayed PCM, 500ms after voice begins/ends. For
immediate voice duck, apply a live gain at local_player playback and receiver
SET_PARAMETER volume composed with latest user slider value. Retain desired
slider separately so release restores current user level. Avoid double duck.
Network SET_PARAMETER latency remains an acoustic measurement requirement.
