#!/bin/sh
# Every launchd job named here should be loaded. If one is not, load it.
#
#   tunnel-watchdog.sh                     every cloudflared agent this user has
#   tunnel-watchdog.sh <label> …           just these
#
# This exists because of 2026-09-21. A tunnel was restarted over the ssh
# session the tunnel itself was carrying: `launchctl bootout` cut the wire
# mid-command, the `bootstrap` half never ran, and a job booted OUT of its
# domain is no longer managed — KeepAlive does not apply to a job launchd
# has been told to forget. Ten hostnames stayed down until somebody could
# walk to the machine, because the only way in was the thing that was down.
#
# KeepAlive answers "the process died". Nothing answered "the job is gone".
# This does, and it runs as a daemon so it is there at boot, before anybody
# logs in — which is the other half of why that machine could not heal.
set -eu
# Given no labels, watch every cloudflared agent this user has. That keeps
# the script *and* its plist identical on every machine in the mesh, and a
# tunnel added next month is watched without anybody remembering to add it.
if [ "$#" -eq 0 ]; then
  set -- $(ls "$HOME/Library/LaunchAgents" 2>/dev/null | sed -n 's/^\(com\.cloudflare\.cloudflared-.*\)\.plist$/\1/p')
fi
# it writes its own log, so the plist needs no per-user paths
exec >>"$HOME/Library/Logs/tunnel-watchdog.log" 2>&1
[ "${WATCHDOG_DRY:-}" = 1 ] && { echo "$(date -u +%FT%TZ) would watch: $*"; exit 0; }
UID_=$(stat -f %u "$HOME")
DOMAIN="gui/$UID_"
AGENTS="$HOME/Library/LaunchAgents"
for label in "$@"; do
  plist="$AGENTS/$label.plist"
  [ -f "$plist" ] || { echo "$(date -u +%FT%TZ) $label: no plist at $plist"; continue; }
  if launchctl print "$DOMAIN/$label" >/dev/null 2>&1; then
    continue                     # loaded; KeepAlive owns it from here
  fi
  echo "$(date -u +%FT%TZ) $label: not loaded — bootstrapping"
  if launchctl bootstrap "$DOMAIN" "$plist" >/dev/null 2>&1; then
    echo "$(date -u +%FT%TZ) $label: back"
  else
    echo "$(date -u +%FT%TZ) $label: bootstrap refused" >&2
  fi
done
