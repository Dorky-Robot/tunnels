#!/bin/sh
# Proves cdp.sh on a scratch repo and a fake prod (a health file), never on
# anything real: a good ship, a bad deploy that rolls back, a hold, a failing
# gate, the kill switch, clinic hours, the daily cap, Everyday Vet's own hold
# rules (test-ev.hold) and a hold rule that matches no file.
set -u
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
export CD_HOME=$T/home CD_STATE=$T/state CD_OFFLIST=$T/offlist CD_ORG=/nonexistent CD_NTFY= MESH_WATCH_NTFY=
export CD_NOW="6 1200" CD_WATCH_FOR=2 CD_WATCH_EVERY=1
HERE=$(cd "$(dirname "$0")" && pwd); CDP="sh $HERE/cdp.sh"
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
mkdir migrations; echo x > migrations/000.sql
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
echo x > migrations/001.sql; commit "add a migration"
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
# ── Everyday Vet's rules, on a scratch repo with its real file names ──
git init -q --bare "$T/ev.git"; git clone -q "$T/ev.git" "$T/ev" 2>/dev/null
cd "$T/ev"; git config user.email t@t; git config user.name t
{ cat <<CFG; cat "$HERE/test-ev.hold"; } > .cd
product   ev
prod      echo {sha} >> $T/evprod
health    file://$T/ok
cap       9
CFG
echo '{"ok":true}' > "$T/ok"
for f in src/consent/legal.rs src/consent/optouts.rs src/bin/optouts.rs tests/optouts.rs src/thread/sms.rs \
  src/thread/files.rs src/thread/prune.rs docs/messaging.md docs/a2p-campaign.md src/words/talk.rs \
  src/words/people.rs tests/acts/golden.txt tests/acts.rs KITA_REV src/bin/migrate.rs src/import.rs \
  src/bin/import.rs src/bin/unbundle.rs src/bin/together.rs src/bin/transcripts.rs src/bin/twilio_leftovers.rs \
  src/bin/scrub.rs .env.example run.sh release.sh ship.sh smoke.sh check.sh backup.sh src/auth.rs src/push.rs \
  assets/sw.js src/desk/mod.rs src/day/mod.rs docs/desk.md Cargo.lock; do mkdir -p "$(dirname $f)"; echo "// $f" > $f; done
printf '[dependencies]\nserde = "1.0.1"\nkita = { git = "ssh://x/kita2.git", rev = "aaa" }\n' > Cargo.toml
git add -A; git commit -qm "ev v1"; git push -q origin HEAD:main 2>/dev/null
EV=$(git rev-parse HEAD); echo "ev $T/ev" >> "$CD_HOME/repos"; $CDP init ev "$EV" >/dev/null
# one commit made by "$@", which must be held with nothing shipped; then main goes back
evheld() { what=$1; shift; "$@"; commit "$what"
  out=$($CDP tick ev 2>&1); echo "$out" | grep -q "held ev" && [ "$(cat "$CD_STATE/ev.shipped")" = "$EV" ] && [ ! -e "$T/evprod" ] \
    && ok "held: $what" || { no "NOT held: $what"; echo "$out"; }
  git reset -q --hard "$EV"; git push -qf origin HEAD:main 2>/dev/null; }
# one commit made by "$@", which must ship
evships() { what=$1; shift; "$@"; commit "$what"; out=$($CDP tick ev 2>&1); EV=$(git rev-parse HEAD)
  [ "$(cat "$CD_STATE/ev.shipped")" = "$EV" ] && grep -q "$EV" "$T/evprod" && ok "ships: $what" || { no "did not ship: $what"; echo "$out"; }; }
add() { echo "$2" >> "$1"; }
swap() { sed "s/$2/$3/" "$1" > "$1.t" && mv "$1.t" "$1"; }
echo "7 Everyday Vet: consent, STOP, wording, migrations, deletions and flags hold"
evheld "consent: src/consent/legal.rs"        add src/consent/legal.rs "// reworded"
evheld "consent: a new file in src/consent"   add src/consent/new.rs "// new"
evheld "STOP: the send gate, thread/sms.rs"   add src/thread/sms.rs "// x"
evheld "what may be sent: thread/files.rs"    add src/thread/files.rs "// x"
evheld "opt-outs: src/bin/optouts.rs"         add src/bin/optouts.rs "// x"
evheld "consent moved out of src/consent"     git mv src/consent/legal.rs src/desk/legal.rs
evheld "may_text called from the desk"        add src/desk/mod.rs "if may_text(c) { send() }"
evheld "the word STOP in a string"            add src/day/mod.rs 'let s = "Reply STOP to end";'
evheld "wording: src/words/talk.rs"           add src/words/talk.rs "// x"
evheld "wording: consent_ask_text elsewhere"  add src/desk/mod.rs "let t = consent_ask_text(p);"
evheld "what an act means: golden.txt"        add tests/acts/golden.txt "act"
evheld "the kita pin: KITA_REV"               add KITA_REV "bbb"
evheld "the kita pin: Cargo.toml's kita line" swap Cargo.toml aaa bbb
evheld "the kita pin: Cargo.lock"             add Cargo.lock 'source = "git+ssh://x/kita2.git?rev=bbb#bbb"'
evheld "an importer: src/import.rs"           add src/import.rs "// x"
evheld "a deleter: src/bin/scrub.rs"          add src/bin/scrub.rs "// x"
evheld "retract( in ordinary code"            add src/desk/mod.rs "kita.retract(id);"
evheld "rm -rf in a script"                   add backup.sh 'rm -rf "$OLD"'
evheld "a flag: EV_SEND_FILES"                add src/desk/mod.rs 'env::var("EV_SEND_FILES")'
evheld "the deploy path: release.sh"          add release.sh "# x"
evheld "the service worker: assets/"          add assets/sw.js "// x"
echo "8 Everyday Vet: ordinary work still ships"
evships "desk code and a doc"                 eval 'add src/desk/mod.rs "fn stopped() {} // STOPPED, helpful"; add docs/desk.md "more"'
evships "a dependency that is not kita"       swap Cargo.toml 1.0.1 1.0.2
echo "9 a hold rule that matches no file holds the line"
add .cd "hold      src/optout* migrations/"; commit "rules for files this repo does not have"
$CDP tick ev 2>&1 | grep -q "touches '.cd'" && ok "a change to .cd holds" || no ".cd"
$CDP ship ev "$(git rev-parse HEAD)" >/dev/null 2>&1; EV=$(git rev-parse HEAD)
add docs/desk.md "harmless"; commit "a harmless doc change"
out=$($CDP tick ev 2>&1); echo "$out" | grep -q "held ev.*hold rule 'src/optout\*' matches no file" && [ "$(cat "$CD_STATE/ev.shipped")" = "$EV" ] \
  && ok "held, and it names the rule" || { no "dead rule"; echo "$out"; }
swap .cd "^hold .*optout.*" "holdline  nosuch\/*.rs secret"; commit "a line rule for no file"
$CDP ship ev "$(git rev-parse HEAD)" >/dev/null 2>&1; EV=$(git rev-parse HEAD)
add docs/desk.md "harmless"; commit "another harmless doc change"
$CDP tick ev 2>&1 | grep -q "hold rule 'nosuch/\*.rs secret' matches no file" && [ "$(cat "$CD_STATE/ev.shipped")" = "$EV" ] && ok "a holdline for no file holds too" || no "dead holdline"
# the fixture is a copy; say so when Everyday Vet's own .cd has moved on
theirs=$(git -C "$HERE/../../dev/everyday-vet-admin" show main:.cd 2>/dev/null | grep '^hold') || true
[ -z "$theirs" ] || [ "$theirs" = "$(cat "$HERE/test-ev.hold")" ] || echo "  ! test-ev.hold differs from everyday-vet-admin's .cd: copy its hold lines here"
[ $fail = 0 ] && echo PASS || { echo FAIL; exit 1; }
