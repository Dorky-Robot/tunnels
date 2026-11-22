#!/bin/sh
# Put the weekly Claude Code update on this box: the one thing that updates
# Claude Code on our machines (Felix, 11-22: "i think its probably safe to
# auto update if its like 4 am"). docs/lead-keeper.md, ecosystem-log
# how/claude-code-updates.md.
#
#   sh mesh/install-claude-update.sh     install, or bring this box up to date
#
# It does three things and nothing else:
#   1. copies scripts/claude-update.py, scripts/claude_bg.py and
#      mesh/leads.conf to ~/.local/libexec/claude-update (what the job runs)
#   2. writes com.dorkyrobot.claude-update from the template, at this box's
#      minute, and loads it. This job carries no connection, so reloading it
#      over ssh is safe.
#   3. sets env.DISABLE_AUTOUPDATER=1 in ~/.claude/settings.json if it is not
#      set, and touches no other key, so Claude Code's own updater (any hour)
#      never runs beside this one (off since 10-07, Felix's yes).
# It never runs `claude update` and never restarts a session. Alerts go to
# MESH_WATCH_NTFY in ~/.config/mesh-watch/env; it says so if that is missing.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/.." && pwd)
H=$(hostname -s)
L=com.dorkyrobot.claude-update
dest=$HOME/.local/libexec/claude-update

# Our boxes only, staggered so they do not all download at once, inside the
# 04:00-06:00 the script enforces. Doug's mini and Sara's Mac are not ours to
# update: they get nothing.
case "$H" in
  dorkyrobot3) m=30 ;; dorkyrobot2) m=33 ;; dorkyrobot1) m=36 ;;
  felixs-mac-mini) m=39 ;; felixs-mac-2024) m=42 ;; felixs-mac-2019) m=45 ;;
  *) echo "install-claude-update: $H is not one of our boxes; nothing installed"; exit 0 ;;
esac

mkdir -p "$dest/scripts" "$dest/mesh" "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
cp "$repo/scripts/claude-update.py" "$repo/scripts/claude_bg.py" "$dest/scripts/"
cp "$repo/mesh/leads.conf" "$dest/mesh/"
chmod 755 "$dest/scripts/claude-update.py"

P=$HOME/Library/LaunchAgents/$L.plist
new=$(mktemp)
sed -e "s#__HOME__#$HOME#g" -e "s#<key>Minute</key><integer>31</integer>#<key>Minute</key><integer>$m</integer>#" "$here/$L.plist" > "$new"
plutil -lint -s "$new"
if ! cmp -s "$new" "$P" || ! launchctl print "gui/$(id -u)/$L" >/dev/null 2>&1; then
  launchctl bootout "gui/$(id -u)/$L" 2>/dev/null || true
  cp "$new" "$P"
  launchctl bootstrap "gui/$(id -u)" "$P"
  echo "· $L: loaded, Sunday 04:$m"
fi
rm -f "$new"

/usr/bin/python3 - <<'PY'
import json, os
p = os.path.expanduser("~/.claude/settings.json")
try:
    s = json.load(open(p))
except FileNotFoundError:
    s = {}
if str(s.get("env", {}).get("DISABLE_AUTOUPDATER")) != "1":
    s.setdefault("env", {})["DISABLE_AUTOUPDATER"] = "1"
    tmp = p + ".tmp"
    with open(tmp, "w") as f:
        json.dump(s, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.replace(tmp, p)
    print("· ~/.claude/settings.json: env.DISABLE_AUTOUPDATER=1")
PY

grep -q '^MESH_WATCH_NTFY=' "$HOME/.config/mesh-watch/env" 2>/dev/null \
  || echo "install-claude-update: no MESH_WATCH_NTFY in ~/.config/mesh-watch/env, so its alerts cannot reach Felix" >&2
printf '%s  claude-update %s  Sunday 04:%s\n' "$H" "$(shasum -a 256 "$dest/scripts/claude-update.py" | cut -c1-12)" "$m"
