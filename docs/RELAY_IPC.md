# Relay process IPC v1 (candidate)

<!-- current-status-20261112 -->
> Сверено12ноября2026. Текущий статус и ограничения — [STATUS](STATUS.md). Датированные замеры и этапы ниже сохранены как история, их слова «ещё не реализовано/не установлено» относятся к указанному этапу, а не автоматически к нынешнему main.


Opt-in `airplay send --control-socket /absolute/private-dir/session.sock`.
The directory must already exist, belong to the effective user and have no group/other
permissions. Socket mode0600; peers must have the same effective UID. No TCP listener.
An existing path is never deleted/replaced at startup. Cleanup removes only the same
device/inode created by this instance. Use a unique per-run directory/socket.

One UTF-8 command terminated by LF per connection, at most255 bytes before LF;
one response then close. The whole connection has a1s deadline. Connections are
handled sequentially, independently of media tasks. No unbounded command queue.

## Commands

`STATUS\n` returns:

```text
OK started=0 targets=1
TARGET 10.0.0.2:7000 requested_db=-20.100000 packets=0 frame=0
END
```

- endpoint is the resolved negotiated target address, with brackets for IPv6.
- requested_db is the desired volume, NOT a receiver acknowledgement.
- packets/frame are successful UDP audio send count and last source frame index.
- started=1 means at least one successful UDP audio send to EVERY configured target.
  It is startup observation, NOT acoustic acknowledgement, ongoing health or
  current play/pause status. Use sender lifecycle and local renderer stats as well.
- zero targets during connect/cleanup is valid. It must not trigger local handoff.
- rows and started are computed from the same controller snapshot.

`VOLUME endpoint db\n` changes only that active endpoint's Volume watch:

```text
VOLUME 10.0.0.2:7000 -20.1
```

Success: `OK queued\n`. This acknowledges local queueing only. Range[-30,0]dB
or mute sentinel-144; NaN/infinity and unknown/inactive targets are rejected.
Each target coalesces rapid updates to its last desired level; RTSP remains off
the UDP media task. No source/card metadata or credentials cross this interface.

Errors are fixed `ERR ...\n` strings; callers must treat them as failures.
An oversized tail is consumed only through a bounded LF/deadline; excessively
large or incomplete requests may be closed without a reply.
No STOP, membership editing, source switching or route/mixer commands in v1.
The parent still owns process stop and restoring the direct sound path.

## Station integration boundary

The Station controller candidate has core_bridge.rs with bounded STATUS parsing and
target-volume requests. It is not yet selected by outputs.rs; legacy sender remains
the active source path. Do not replace its executable alone: readiness, volume,
per-session gate/stats and process lifecycle must be connected together.

Tests use Unix sockets in a network-isolated container, no HomePod or audio device.

Station startup predicate:Status::ready_for(expected) requires the exact nonempty endpoint set and transmitted packets for every member; a started flag from a previous/partial selection is insufficient. The parent must independently qualify child liveness, shared clock and local renderer. The candidate remains unselected by outputs.rs. The present CLI send path is NTP; the library PtpGroup/continuous runtime needs a dedicated adapter host before group migration.81 Station MRP tests passed for this predicate/socket contract; no installation occurred.
