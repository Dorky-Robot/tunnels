#!/bin/sh
# bin/pocket-id for a box where the job may only be kickstarted, never booted out (doug-mini):
# the plist stays as v1 had it and this file takes the binary's place. It adds what v2 needs that
# the plist does not set, and its first start after `launchctl kickstart -k` (v1 already stopped)
# does the switch upgrade.sh does: a cold copy of data/ (db, -wal, -shm and keys together), v2's
# migration on a spare loopback port, keepkey putting v1's signing key back. If any of that fails
# it puts the copy back and runs v1, and keeps running v1 until v2-failed is removed.
# Install: README.md in this directory. Rollback: rollback-launcher.sh.
D=$(cd "$(dirname "$0")/.." && pwd)
V2=$D/bin/pocket-id-2.19.1 V1=$D/bin/pocket-id-1.16.0 KEEPKEY=$D/bin/keepkey-2.19.1
SPARE=14199
export ENCRYPTION_KEY_FILE=$D/encryption.key ACTORS_HOST=127.0.0.1
export PATH=/usr/bin:/bin:/usr/sbin:/sbin
log() { echo "[$(date '+%F %T')] launcher: $*" | tee -a "$D/cutover.log"; }
kid_on() { curl -s -m 5 "http://127.0.0.1:$1/.well-known/jwks.json" | plutil -extract keys.0.kid raw - 2>/dev/null; }
with_limit() { s=$1; shift; "$@" & p=$!; ( sleep "$s"; kill -9 "$p" ) >/dev/null 2>&1 & w=$!; wait "$p"; rc=$?; kill "$w" 2>/dev/null; return $rc; }
cd "$D" || exit 1

[ -e v2-failed ] && exec "$V1"
[ -e v2-key-kept ] && exec "$V2"

OLDKID=$(plutil -extract kid raw data/keys/jwt_private_key.json)
COLD=data.prev-v1-$(date +%Y%m%d-%H%M%S)
MIG=
fail() {
  log "FAILED: $1; v1 runs on the cold copy"
  [ -n "$MIG" ] && kill "$MIG" 2>/dev/null && sleep 3
  # copy first, swap after: v1 started on an empty data/ would mint a new key and forget everyone
  [ -d "$COLD" ] && rm -rf data.restoring data.v2-failed && cp -Rp "$COLD" data.restoring &&
    mv data data.v2-failed && mv data.restoring data
  touch v2-failed
  exec "$V1"
}

log "first v2 start, kid $OLDKID"
[ -n "$OLDKID" ] || fail "no v1 key in data/keys"
cp -Rp data "$COLD" || fail "cold copy"
log "cold copy $COLD ($(ls "$COLD" | tr '\n' ' '))"

PORT="$SPARE" ACTORS_PORT=$((SPARE + 1)) "$V2" >> "$D/upgrade-migrate.log" 2>&1 &
MIG=$!
NEWKID=
for _ in $(seq 24); do NEWKID=$(kid_on "$SPARE"); [ -n "$NEWKID" ] && break; sleep 5; done
[ -n "$NEWKID" ] || fail "v2 did not answer on :$SPARE (upgrade-migrate.log)"
kill "$MIG"; wait "$MIG" 2>/dev/null; MIG=
log "v2 migrated the data and generated kid $NEWKID; stopped"

OUT=$(with_limit 60 "$KEEPKEY" -db data/pocket-id.db -key data/keys/jwt_private_key.json 2>&1)
log "keepkey: $(echo "$OUT" | tr '\n' ';')"
echo "$OUT" | grep -q "^now: *kid=$OLDKID " || fail "keepkey did not put kid $OLDKID back"

touch v2-key-kept
log "v2 starts with kid $OLDKID (unchanged); v1 copy $COLD"
exec "$V2"
