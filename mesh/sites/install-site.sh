#!/bin/bash
# Installs one small site on dorkyrobot1 under its own name, and (re)starts it. Run ON dr1.
#
#   bash install-site.sh <name> <port> static     serves ~/Projects/humOS/<name>/public with static.mjs
#   bash install-site.sh <name> <port> app        runs ~/Projects/humOS/<name>/app/server.mjs (PORT in its environment)
#
# WHY (Felix, 11-20: "get all the app servers into dr1"): hibla, una, jerry and super-vision served from
# their dev folders on the mini under Homebrew's python and node. Here each is a folder under
# ~/Projects/humOS (where-things-run.md, "An instance is a folder and a process"), run by a program
# named for it (d-172, "A service has its own name"): a copy of the nodejs.org Node this box already
# checked by SHA-256 for overview (overview's scripts/program.sh), signed again with this box's
# certificate under the site's own identifier. Loopback only: cloudflared on this box is its one caller,
# so it needs no firewall entry and does not wait on Tailscale after a restart.
#
# Layout:  ~/Projects/humOS/<name>/{public|app}/   what is served (data lives inside app/)
#          ~/Projects/humOS/<name>/program/<name>  the program, never inside what is served
#          ~/Projects/humOS/<name>/logs/site.log
set -euo pipefail
name=${1:?name}; port=${2:?port}; kind=${3:?static|app}
die() { echo "install-site: $*" >&2; exit 1; }
[ "$(scutil --get LocalHostName 2>/dev/null)" = dorkyrobot1 ] || [ -n "${SITE_ANY_HOST:-}" ] || die "this installs on dorkyrobot1"
home=$HOME/Projects/humOS/$name
label=com.dorkyrobot.$name
prog=$home/program/$name
src=${SITE_NODE_FROM:-$HOME/Projects/humOS/overview/program/overview}
KC=$HOME/Library/Keychains/bahay-signing.keychain-db
PW=$HOME/services/bahay/secrets/signing-keychain-password
here=$(cd "$(dirname "$0")" && pwd)

case $kind in
  static) [ -d "$home/public" ] || die "no $home/public"; cp "$here/static.mjs" "$home/static.mjs"
          args=("$prog" "$home/static.mjs" "$port" "$home/public"); wd=$home ;;
  app)    [ -f "$home/app/server.mjs" ] || die "no $home/app/server.mjs"
          args=("$prog" server.mjs); wd=$home/app ;;
  *) die "kind is static or app" ;;
esac
mkdir -p "$home/program" "$home/logs"

want=$("$src" --version) || die "no Node to copy at $src"
if [ -x "$prog" ] && [ "$("$prog" --version 2>/dev/null)" = "$want" ] \
   && codesign -d -r- "$prog" 2>&1 | grep -q "identifier \"$label\" and certificate"; then
  echo "program: $prog is $want, signed as $label"
else
  [ -f "$KC" ] && [ -f "$PW" ] || die "no signing keychain at $KC"
  sign=$(security find-identity -p codesigning "$KC" | sed -n 's/.*"\(bahay desk ([^)]*)\)".*/\1/p' | head -1)
  [ -n "$sign" ] || die "no 'bahay desk (…)' certificate in $KC"
  work=$(mktemp -d "${TMPDIR:-/tmp}/site-node.XXXXXX"); trap 'rm -rf "$work"' EXIT INT TERM HUP
  cp "$src" "$work/node"
  if otool -L "$work/node" | tail -n +2 | awk '{print $1}' | grep -v -E '^(/System/Library/|/usr/lib/)'; then die "the Node at $src links something outside the system"; fi
  cat > "$work/ent.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>com.apple.security.cs.allow-jit</key><true/>
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
</dict></plist>
PLIST
  printf 'unlock-keychain -p "%s" "%s"\n' "$(cat "$PW")" "$KC" | security -i >/dev/null
  codesign -f --keychain "$KC" -s "$sign" -i "$label" -o runtime --entitlements "$work/ent.plist" "$work/node" 2>/dev/null
  codesign -d -r- "$work/node" 2>&1 | grep -q "identifier \"$label\" and certificate" || die "not signed by $sign"
  "$work/node" -e 'new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0])); require("node:http")' || die "the signed program cannot start"
  # a new file renamed into place, never a copy over a running program
  [ -f "$prog" ] && cp -p "$prog" "$prog.prev"
  cp "$work/node" "$prog.new"; xattr -c "$prog.new"; mv -f "$prog.new" "$prog"
  echo "program: installed $prog, $want, signed by $sign as $label"
fi

plist=$HOME/Library/LaunchAgents/$label.plist
{
  cat <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$label</string>
  <key>ProgramArguments</key><array>
PL
  for a in "${args[@]}"; do echo "    <string>$a</string>"; done
  cat <<PL
  </array>
  <key>WorkingDirectory</key><string>$wd</string>
  <key>EnvironmentVariables</key><dict><key>PORT</key><string>$port</string><key>HOST</key><string>127.0.0.1</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>$home/logs/site.log</string>
  <key>StandardErrorPath</key><string>$home/logs/site.log</string>
</dict></plist>
PL
} > "$plist.new"
plutil -lint "$plist.new" >/dev/null
mv -f "$plist.new" "$plist"
if launchctl print "gui/$UID/$label" >/dev/null 2>&1; then launchctl kickstart -k "gui/$UID/$label"
else launchctl bootstrap "gui/$UID" "$plist"; fi
for _ in $(seq 40); do
  if curl -s -o /dev/null -m 2 "http://127.0.0.1:$port/"; then
    pid=$(launchctl print "gui/$UID/$label" | sed -n 's/^[[:space:]]*pid = //p')
    echo "site: $name answers on 127.0.0.1:$port (pid $pid, $(ps -o comm= -p "$pid"))"; exit 0
  fi
  sleep 0.5
done
die "$name did not answer on 127.0.0.1:$port in 20 s; see $home/logs/site.log"
