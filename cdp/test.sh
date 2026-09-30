#!/bin/sh
# Proves cdp.sh on a scratch repo and a fake prod (a health file), never on
# anything real: a good ship, a bad deploy that rolls back, a hold, a failing
# gate, the kill switch, clinic hours, the daily cap.
set -u
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
export CD_HOME=$T/home CD_STATE=$T/state CD_OFFLIST=$T/offlist CD_ORG=/nonexistent CD_NTFY= MESH_WATCH_NTFY=
export CD_NOW="6 1200" CD_WATCH_FOR=2 CD_WATCH_EVERY=1
CDP="sh $(cd "$(dirname "$0")" && pwd)/cdp.sh"
mkdir -p "$CD_HOME" "$T/prod"; echo '{"ok":true}' > "$T/prod/health"
git init -q --bare "$T/origin.git"; git clone -q "$T/origin.git" "$T/app" 2>/dev/null
cd "$T/app"; git config user.email t@t; git config user.name t
cat > .cd <<CFG
product   demo
gate      sh gate.sh
smoke     test -f app.txt
prod      cp $T/prod/health $T/prod/health.prev && cp health.json $T/prod/health
rollback  cp $T/prod/health.prev $T/prod/health
health    file://$T/prod/health
hold      migrations
hours     deny Mon-Fri 07:30-19:30 America/New_York
cap       4
CFG
echo '{"ok":true}' > health.json; echo v1 > app.txt; echo 'exit 0' > gate.sh
git add -A; git commit -qm "v1"; git push -q origin HEAD:main 2>/dev/null
LIVE=$(git rev-parse HEAD); echo "demo $T/app" > "$CD_HOME/repos"; touch "$CD_HOME/STOP"
$CDP init demo "$LIVE" >/dev/null
fail=0; ok() { printf '  ✓ %s\n' "$1"; }; no() { printf '  ✗ %s\n' "$1"; fail=1; }
commit() { git add -A; git commit -qm "$1"; git push -q origin HEAD:main 2>/dev/null; }
live() { cat "$CD_STATE/demo.shipped"; }
prodok() { grep -q '"ok":true' "$T/prod/health"; }

echo "1 kill switch on: proven but not shipped"
echo v2 > app.txt; commit "v2

Ready: demo/one"
$CDP tick demo 2>&1 | grep -q "kill switch is on" && [ "$(live)" = "$LIVE" ] && ok "gate+smoke ran, nothing shipped" || no "stop"
echo "2 switch off: a good commit ships"
$CDP go >/dev/null; $CDP tick demo >/dev/null 2>&1
[ "$(live)" = "$(git rev-parse HEAD)" ] && ok "shipped v2" || no "good ship"
[ ! -e "$T/offlist" ] && ok "tagged commit: not on the off-list" || no "offlist"
echo "3 bad deploy: prod answers ok:false, auto-rollback"
GOOD=$(live); echo '{"ok":false}' > health.json; echo v3 > app.txt; commit "v3 breaks health"
out=$($CDP tick demo 2>&1); echo "$out" | grep -q "ROLLED BACK" && prodok && [ "$(live)" = "$GOOD" ] && ok "rolled back, prod healthy, live stays on v2" || { no "rollback"; echo "$out"; }
ls "$T/offlist" >/dev/null 2>&1 && grep -q "without a Ready tag" "$T/offlist"/*.md && ok "untagged commits are on the off-list" || true
$CDP tick demo >/dev/null 2>&1; echo "$($CDP tick demo 2>&1)" | grep -q . && no "retried a failed sha" || ok "a failed sha is not retried"
echo "4 a fix ships; failing gate does not"
echo '{"ok":true}' > health.json; echo 'exit 1' > gate.sh; commit "v4 fix, gate red"
$CDP tick demo 2>&1 | grep -q "gate failed" && [ "$(live)" = "$GOOD" ] && ok "gate red: nothing shipped" || no "gate"
echo 'exit 0' > gate.sh; commit "v5 gate green"
$CDP tick demo >/dev/null 2>&1; [ "$(live)" = "$(git rev-parse HEAD)" ] && ok "shipped v5" || no "v5"
echo "5 holds"
mkdir migrations; echo x > migrations/001.sql; commit "add a migration"
$CDP tick demo 2>&1 | grep -q "held demo" && ok "migration path holds" || no "hold"
$CDP tick demo 2>&1 | grep -q . && no "held twice" || ok "told once"
before=$(live); $CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1; [ "$(live)" != "$before" ] && ok "cdp ship releases the hold" || no "release hold"
echo 2 > app.txt; commit "tiny change

Hold: flag turns on a client-visible thing"
$CDP tick demo 2>&1 | grep -q "Hold:" && ok "Hold: in a message holds" || no "Hold trailer"
$CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1
echo "6 clinic hours and cap"
echo 3 > app.txt; commit "during clinic hours"
CD_NOW="2 1000" $CDP tick demo 2>&1 | grep -q "clinic hours" && [ "$(live)" != "$(git rev-parse HEAD)" ] && ok "waits inside hours" || no "hours"
CD_NOW="6 1200" $CDP tick demo 2>&1 | grep -q "ships today" && ok "cap 4 holds the 5th ship" || no "cap"
echo 4 > app.txt; commit "weekend, but capped"
[ $fail = 0 ] && echo PASS || { echo FAIL; exit 1; }
