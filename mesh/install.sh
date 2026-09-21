#!/bin/sh
# Put the mesh's shared setup on this machine, identical to every other one.
#
#   sh mesh/install.sh
#
# Idempotent: run it again and nothing changes. It adds and never removes,
# and it backs up any file of yours it has to edit.
#
# What it installs — the same bytes on every machine:
#   ~/.ssh/config.d/mesh.conf         every box, three ways in (see its header)
#   ~/.ssh/config.d/mesh_known_hosts  host keys taken from each machine itself
#   ~/.local/bin/tunnel-watchdog.sh   brings back a tunnel that was booted out
#   com.dorkyrobot.tunnel-watchdog    runs it every five minutes, finds its own tunnels
#
# What it does NOT do, because it cannot be done from one machine:
#   - make this machine's mesh key and authorize it on the others (see
#     docs/remote-access.md, "Adding a machine")
#   - install Tailscale (needs a password and a browser)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/.." && pwd)

d="$HOME/.ssh/config.d"; mkdir -p "$d"; chmod 700 "$d"
cp "$here/mesh.conf" "$here/mesh_known_hosts" "$d/"
chmod 644 "$d/mesh.conf" "$d/mesh_known_hosts"

# the Include goes after any Includes already at the top: an Include written
# below a Host line is scoped to that Host, and the mesh would silently only
# apply to it
c="$HOME/.ssh/config"; [ -f "$c" ] || : > "$c"
if ! grep -q "config.d/mesh.conf" "$c"; then
  cp "$c" "$c.bak-mesh-$(date +%s)"
  awk 'BEGIN{done=0} !done && !/^Include/ {print "Include ~/.ssh/config.d/mesh.conf"; done=1} {print} END{if(!done) print "Include ~/.ssh/config.d/mesh.conf"}' "$c" > "$c.new"
  mv "$c.new" "$c"; chmod 600 "$c"
fi

mkdir -p "$HOME/.local/bin" "$HOME/Library/Logs"
cp "$repo/scripts/tunnel-watchdog.sh" "$HOME/.local/bin/tunnel-watchdog.sh"
chmod +x "$HOME/.local/bin/tunnel-watchdog.sh"
P="$HOME/Library/LaunchAgents/com.dorkyrobot.tunnel-watchdog.plist"
launchctl bootout "gui/$(id -u)/com.dorkyrobot.tunnel-watchdog" 2>/dev/null || true
cp "$here/com.dorkyrobot.tunnel-watchdog.plist" "$P"
launchctl bootstrap "gui/$(id -u)" "$P"

printf '%s  mesh.conf %s  known_hosts %s  watchdog %s\n' "$(hostname -s)" \
  "$(shasum -a 256 "$d/mesh.conf" | cut -c1-12)" \
  "$(shasum -a 256 "$d/mesh_known_hosts" | cut -c1-12)" \
  "$(shasum -a 256 "$HOME/.local/bin/tunnel-watchdog.sh" | cut -c1-12)"
