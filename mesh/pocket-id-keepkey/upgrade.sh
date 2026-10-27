#!/bin/sh
# The v1 -> v2 switch for one native Pocket ID, on its own box, detached so it outlives ssh:
#   nohup sh upgrade.sh <dir> <label> >> <dir>/upgrade.log 2>&1 &
#
# Before it (README.md): backup.sh ran; the v2 binary is installed and checksummed; keepkey is in
# <dir>/bin; encryption.key (0600) exists; <label>.plist.v1 is the old plist and <label>.plist the
# new one (v2 binary, ENCRYPTION_KEY_FILE, ACTORS_HOST=127.0.0.1).
#
# It stops v1, takes a cold copy of data/ (db, -wal, -shm and keys together: stop does not
# checkpoint), lets v2 migrate on a spare loopback port, puts v1's signing key back with keepkey,
# and only then starts the job, so the public name never serves a new key. Any failure puts v1
# back as it was.
set -u
DIR=$1 LABEL=$2
SPARE=${SPARE:-14199}
KEEPKEY=${KEEPKEY:-$DIR/bin/keepkey}
U=$(id -u) PL=$DIR/$LABEL.plist AGENT=$HOME/Library/LaunchAgents/$LABEL.plist
export PATH=/usr/bin:/bin:/usr/sbin:/sbin
log() { echo "[$(date '+%F %T')] $*" | tee -a "$DIR/cutover.log"; }
cd "$DIR" || exit 1

BIN=$(/usr/libexec/PlistBuddy -c "Print :ProgramArguments:0" "$PL")
PORT=$(/usr/libexec/PlistBuddy -c "Print :EnvironmentVariables:PORT" "$PL")
ENVS=$(/usr/libexec/PlistBuddy -c "Print :EnvironmentVariables" "$PL" | sed -n 's/^ *\([A-Z_]*\) = \(.*\)$/\1=\2/p')
OLDKID=$(plutil -extract kid raw data/keys/jwt_private_key.json)
kid_on() { curl -s -m 5 "http://127.0.0.1:$1/.well-known/jwks.json" | plutil -extract keys.0.kid raw - 2>/dev/null; }
# Runs a command for at most $1 seconds. A binary that arrived by Taildrop is quarantined, and
# Gatekeeper hangs it with no screen to ask on: on 10-27 that held EV's Pocket ID down 5 minutes.
with_limit() { s=$1; shift; "$@" & p=$!; ( sleep "$s"; kill -9 "$p" ) >/dev/null 2>&1 & w=$!; wait "$p"; rc=$?; kill "$w" 2>/dev/null; return $rc; }
wait_kid() { for _ in $(seq 24); do k=$(kid_on "$1"); [ -n "$k" ] && { echo "$k"; return 0; }; sleep 5; done; return 1; }

for f in "$BIN" "$KEEPKEY" "$PL.v1" encryption.key; do [ -e "$f" ] || { log "missing $f, nothing done"; exit 1; }; done
[ -n "$OLDKID" ] && [ "$(kid_on "$PORT")" = "$OLDKID" ] || { log "v1 is not serving kid $OLDKID on :$PORT, nothing done"; exit 1; }
lsof -nP -iTCP:"$SPARE" -sTCP:LISTEN >/dev/null && { log "spare port $SPARE is taken, nothing done"; exit 1; }
with_limit 15 "$KEEPKEY" -h >/dev/null 2>&1 && with_limit 15 "$BIN" version >/dev/null 2>&1 ||
  { log "keepkey or $BIN does not run within 15 s (quarantine? xattr -d com.apple.quarantine), nothing done"; exit 1; }

COLD=data.prev-v1-$(date +%Y%m%d-%H%M%S)
MIG=
back_to_v1() {
  log "FAILED: $1; putting v1 back"
  [ -n "$MIG" ] && kill "$MIG" 2>/dev/null && sleep 3
  launchctl bootout "gui/$U/$LABEL" 2>/dev/null; sleep 2
  [ -d "$COLD" ] && { rm -rf data.v2-failed; mv data data.v2-failed; cp -Rp "$COLD" data; }
  cp "$PL" "$PL.v2"; cp "$PL.v1" "$PL"; [ -L "$AGENT" ] || cp "$PL" "$AGENT"
  launchctl bootstrap "gui/$U" "$AGENT"
  log "v1 back, serving kid $(wait_kid "$PORT"); the failed v2 data is data.v2-failed"
  exit 1
}

log "v1 -> $(basename "$BIN"), kid $OLDKID"
launchctl bootout "gui/$U/$LABEL"
for _ in $(seq 20); do lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null || break; sleep 1; done
lsof -nP -iTCP:"$PORT" -sTCP:LISTEN >/dev/null && back_to_v1 "v1 still listening on :$PORT"
cp -Rp data "$COLD" || back_to_v1 "cold copy"
log "v1 stopped, cold copy $COLD ($(ls "$COLD" | tr '\n' ' '))"

# v2's first start migrates the database and generates a key nobody has seen yet
env $ENVS PORT="$SPARE" ACTORS_PORT=$((SPARE + 1)) "$BIN" >> "$DIR/upgrade-migrate.log" 2>&1 &
MIG=$!
NEWKID=$(wait_kid "$SPARE") || back_to_v1 "v2 did not answer on :$SPARE (upgrade-migrate.log)"
kill "$MIG"; wait "$MIG" 2>/dev/null; MIG=
log "v2 migrated the data and generated kid $NEWKID; stopped"

OUT=$(with_limit 60 env $ENVS "$KEEPKEY" -db data/pocket-id.db -key data/keys/jwt_private_key.json 2>&1)
log "keepkey: $(echo "$OUT" | tr '\n' ';')"
echo "$OUT" | grep -q "^now: *kid=$OLDKID " || back_to_v1 "keepkey did not put kid $OLDKID back"

[ -L "$AGENT" ] || cp "$PL" "$AGENT"
launchctl bootstrap "gui/$U" "$AGENT" || back_to_v1 "bootstrap"
KID=$(wait_kid "$PORT") || back_to_v1 "v2 job did not answer on :$PORT"
[ "$KID" = "$OLDKID" ] || back_to_v1 "v2 job serves kid $KID, not $OLDKID"
log "v2 up on :$PORT with kid $KID (unchanged); v1 copy $COLD, v1 plist $PL.v1"
