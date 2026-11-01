#!/bin/sh
# Candidate only: no prepare/install path. Do not run on Station before root integration.
# arm SENDER_PID SOURCE STATS; SOURCE: music|airplay|bluetooth.
# Caller MUST await all receiver media-ready acknowledgements and validate source routing first.
# Unique STATS pathname per relay. Marker verified-SOURCE is made only by installation validation.
set -u
R=${MINI2_GROUP_RUN:-/run/mini2-group}
MIXER=${MINI2_GROUP_MIXER:-amixer}
pid=${2:-}; source=${3:-}; stats=${4:-}
case "$1:$pid:$source" in arm:*:music) control='Mini2 Music';; arm:*:airplay) control='Mini2 AirPlay';; arm:*:bluetooth) control='Mini2 Bluetooth';; *) exit 2;; esac
case "$pid" in ''|*[!0-9]*) exit 2;; esac
[ -f "$R/verified-$source" ] || exit 3
[ -n "$stats" ] || exit 2
start() { sed 's/.*) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }
sender_start=$(start "$pid"); [ -n "$sender_start" ] || exit 3
mkdir "$R/owner-$source" 2>/dev/null || exit 3
owner="$R/owner-$source"; muted=0
same_sender() { [ "$(start "$pid")" = "$sender_start" ]; }
level() { "$MIXER" -q -c 0 sset "$control" "$1" >/dev/null 2>&1; }
cleanup() {
 trap - EXIT INT TERM HUP
 # Stop only the sender whose start-time we captured, never a reused PID.
 if same_sender; then kill -TERM "$pid" 2>/dev/null || :; fi
 # Always attempt restore, even if mute returned an error after partial application.
 level 100% || echo 'group-sync: RESTORE FAILED' >&2
 rm -f "$owner/active"; rmdir "$owner" 2>/dev/null || :
}
trap cleanup EXIT
trap 'exit 0' INT TERM HUP
healthy() {
 same_sender || return 1
 [ -f "$stats" ] || return 1
 # No eval/source of externally written stats.
 line=$(cat "$stats")
 p=$(printf '%s\n' "$line" | tr ' ' '\n' | sed -n 's/^pid=//p')
 st=$(printf '%s\n' "$line" | tr ' ' '\n' | sed -n 's/^start_ticks=//p')
 case "$p:$st" in *[!0-9:]*|:|:*|*:) return 1;; esac
 [ "$(start "$p")" = "$st" ] || return 1
 now=$(awk '{printf "%.0f",$1*1000}' /proc/uptime)
 printf '%s\n' "$line" | awk -v now="$now" '
 {for(i=1;i<=NF;i++){split($i,a,"=");v[a[1]]=a[2]}}
 END {age=now-v["boot_ms"]; e=v["error_ms"];
 exit !(v["records"]>=20 && v["ready"]==1 && age>=0 && age<2000 && e>=-20 && e<=20)}'
}
# Startup gate bounded to five seconds. Original source stays audible during startup.
n=0
until healthy; do
 n=$((n+1)); [ "$n" -lt 6 ] || exit 4
 same_sender || exit 4
 sleep 1
done
# Mark mute intent before call so cleanup also handles partial mixer failures.
muted=1; level 0% || exit 5
printf 'sender_pid=%s sender_start=%s source=%s stats=%s\n' "$pid" "$sender_start" "$source" "$stats" > "$owner/active"
echo 'group-sync: active'
while healthy; do sleep 1; done
exit 6
