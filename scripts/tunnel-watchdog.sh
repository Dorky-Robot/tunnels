#!/bin/sh
# Every launchd job named here should be loaded. If one is not, load it.
# Every loaded agent that says it should be running, and is not, start it.
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
#
# And because of 2026-09-23. The macOS 27 upgrade rebooted doug-mini, and
# after login every agent that starts at load — the tunnel, sipag, monica,
# claude-rc, and this watchdog — sat loaded, `runs = 0`, `needs LWCR
# update`, for fourteen hours. Loaded, so the check above passed it. launchd
# had simply never tried; one `launchctl kickstart` each brought them up.
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
    continue                     # loaded; the pass below starts it if it must
  fi
  echo "$(date -u +%FT%TZ) $label: not loaded — bootstrapping"
  if launchctl bootstrap "$DOMAIN" "$plist" >/dev/null 2>&1; then
    echo "$(date -u +%FT%TZ) $label: back"
  else
    echo "$(date -u +%FT%TZ) $label: bootstrap refused" >&2
  fi
done
# Loaded is not running. This pass is over every agent, not just tunnels:
# it only does what the plist already declares, so it cannot start anything
# nobody meant to run. KeepAlive true means it should always be up; RunAtLoad
# with zero runs means launchd never kept the promise it made at load.
# A bare kickstart (no -k) never touches a process that is running.
for plist in "$AGENTS"/*.plist; do
  [ -f "$plist" ] || continue
  label=$(basename "$plist" .plist)
  st=$(launchctl print "$DOMAIN/$label" 2>/dev/null) || continue   # not loaded: not ours to load
  printf '%s\n' "$st" | grep -q '^	state = running' && continue
  runs=$(printf '%s\n' "$st" | sed -n 's/^	runs = //p')
  ka=$(plutil -extract KeepAlive raw -o - "$plist" 2>/dev/null || true)
  ral=$(plutil -extract RunAtLoad raw -o - "$plist" 2>/dev/null || true)
  if [ "$ka" = true ] || { [ "$ral" = true ] && [ "${runs:-0}" = 0 ]; }; then
    echo "$(date -u +%FT%TZ) $label: loaded, not running (runs=${runs:-?}) — kickstarting"
    launchctl kickstart "$DOMAIN/$label" >/dev/null 2>&1 \
      || echo "$(date -u +%FT%TZ) $label: kickstart refused" >&2
  fi
done
