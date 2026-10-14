#!/bin/sh
# Proves cdp.sh on a scratch repo and a fake prod (a health file, an error
# count, an ask count), never on anything real: a good ship with its blip
# line, a bad deploy that rolls back in measured seconds, a fix that takes the
# same route, a failing gate, holds and their release, independent commits
# shipping past a held one while dependent ones wait, hold groups and their
# off switch, signals (max and baseline) rolling a ship back, the circuit
# breaker after two rollbacks, an anomaly between ships, the blip probe
# counting a swap that drops the face, one tick at a time per product,
# Everyday Vet's own hold rules (test-ev.hold), and the rollback contract of
# `ship.sh --undo {sha}`: 0 held, 1 refused and kept, 2 paged and paused,
# anything else paged, and nothing shipped between an anomaly and its undo.
set -u
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
export CD_HOME=$T/home CD_STATE=$T/state CD_OFFLIST=$T/offlist CD_ORG=/nonexistent CD_NTFY= MESH_WATCH_NTFY=
export CD_WATCH_EVERY=1
HERE=$(cd "$(dirname "$0")" && pwd); CDP="sh $HERE/cdp.sh"
mkdir -p "$CD_HOME" "$T/prod"; echo '{"ok":true}' > "$T/prod/health"; echo 0 > "$T/prod/errors"; echo 10 > "$T/prod/asks"
git init -q --bare "$T/origin.git"; git clone -q "$T/origin.git" "$T/app" 2>/dev/null
cd "$T/app"; git config user.email t@t; git config user.name t
cat > .cd <<CFG
product   demo
gate      sh gate.sh
smoke     test -f app.txt
prod      sh prod.sh
rollback  sh rollback.sh
health    file://$T/prod/health
hold      migrations
watch     2
signal    errors max 5 cat $T/prod/errors
signal    asks baseline 0.5 cat $T/prod/asks
CFG
cat > prod.sh <<SH
cp $T/prod/health $T/prod/health.prev; cp $T/prod/app.txt $T/prod/app.txt.prev 2>/dev/null
cp health.json $T/prod/health; cp app.txt $T/prod/app.txt
SH
cat > rollback.sh <<SH
cp $T/prod/health.prev $T/prod/health; cp $T/prod/app.txt.prev $T/prod/app.txt; echo 0 > $T/prod/errors; echo 10 > $T/prod/asks
SH
echo '{"ok":true}' > health.json; echo v1 > app.txt; echo 'exit 0' > gate.sh
mkdir migrations; echo x > migrations/000.sql
git add -A; git commit -qm "v1"; git push -q origin HEAD:main 2>/dev/null
LIVE=$(git rev-parse HEAD); echo "demo $T/app" > "$CD_HOME/repos"; touch "$CD_HOME/STOP"
$CDP init demo "$LIVE" >/dev/null
fail=0; ok() { printf '  ✓ %s\n' "$1"; }; no() { printf '  ✗ %s\n' "$1"; fail=1; }
commit() { git add -A; git commit -qm "$1"; git push -q origin HEAD:main 2>/dev/null; }
live() { cat "$CD_STATE/demo.shipped"; }
floor() { cat "$CD_STATE/demo.floor"; }
prodok() { grep -q '"ok":true' "$T/prod/health"; }
# prod has main's tree (its sha is main's until the first ship past a hold, then a deploy-ref commit)
onmain() { [ "$(git rev-parse "$(live)^{tree}")" = "$(git rev-parse "HEAD^{tree}")" ]; }
ships() { grep -c . "$CD_STATE/demo.ships" 2>/dev/null || echo 0; }
short() { echo "$1" | cut -c1-8; }

echo "1 kill switch on: proven but not shipped"
echo v2 > app.txt; commit "v2

Ready: demo/one"
$CDP tick demo 2>&1 | grep -q "kill switch is on" && [ "$(live)" = "$LIVE" ] && ok "gate+smoke ran, nothing shipped" || no "stop"
echo "2 switch off: a good commit ships, and the line says what the swap felt like"
$CDP go >/dev/null; out=$($CDP tick demo 2>&1)
onmain && [ "$(cat "$T/prod/app.txt")" = v2 ] && ok "shipped v2" || { no "good ship"; echo "$out"; }
echo "$out" | grep -q "shipped demo .* (1 commits) · blip: 0 failed, 0 slow of [1-9][0-9]* probes" && ok "the ship line counts the blip: none" || { no "blip line"; echo "$out"; }
[ ! -e "$T/offlist" ] && ok "tagged commit: not on the off-list" || no "offlist"
echo "3 bad deploy: prod answers ok:false, auto-rollback in seconds"
GOOD=$(live); echo '{"ok":false}' > health.json; echo v3 > app.txt; commit "v3 breaks health"
out=$($CDP tick demo 2>&1); echo "$out" | grep -q "ROLLED BACK demo .* in [0-9]*s: health1 is 'fail', not ok, 2 in a row" && prodok && [ "$(live)" = "$GOOD" ] \
  && ok "rolled back, time printed, prod healthy, live stays on v2" || { no "rollback"; echo "$out"; }
grep -q "s$" "$CD_STATE/demo.rollbacks" && ok "the rollback and its time are on record" || no "rollback record"
ls "$T/offlist" >/dev/null 2>&1 && grep -q "without a Ready tag" "$T/offlist"/*.md && ok "untagged commits are on the off-list" || true
$CDP tick demo >/dev/null 2>&1; echo "$($CDP tick demo 2>&1)" | grep -q . && no "retried a failed sha" || ok "a failed sha is not retried"
echo "4 the fix takes the same route; a failing gate does not ship"
echo '{"ok":true}' > health.json; echo 'exit 1' > gate.sh; commit "v4 fix, gate red"
$CDP tick demo 2>&1 | grep -q "gate failed" && [ "$(live)" = "$GOOD" ] && ok "gate red: nothing shipped" || no "gate"
echo 'exit 0' > gate.sh; commit "v5 gate green"
$CDP tick demo >/dev/null 2>&1; onmain && [ "$(cat "$T/prod/app.txt")" = v3 ] && ok "the fix shipped through gate, smoke, prod and watch" || no "v5"
echo "5 holds"
echo x > migrations/001.sql; commit "add a migration"
$CDP tick demo 2>&1 | grep -q "held demo .*: migrations/001.sql touches 'migrations'. Release: cdp ship demo" && ok "migration path holds" || no "hold"
$CDP tick demo 2>&1 | grep -q . && no "held twice" || ok "told once"
before=$(live); $CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1; onmain && ok "cdp ship releases the hold" || no "release hold"
echo 2 > app.txt; commit "tiny change

Hold: flag turns on a client-visible thing"
$CDP tick demo 2>&1 | grep -q "the commit says Hold:" && ok "Hold: in a message holds" || no "Hold trailer"
$CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1
echo "6 independent commits ship past a held one; dependent ones wait"
cat > gate.sh <<'SH'
git rev-parse HEAD > "$CD_STATE/gate-saw"; ls migrations >> "$CD_STATE/gate-saw"; exit 0
SH
commit "gate records what tree it saw"; $CDP tick demo >/dev/null 2>&1; MAIN0=$(git rev-parse HEAD)
echo y > migrations/002.sql; echo s > shared.txt; commit "H: a migration, and a file"; H=$(git rev-parse HEAD)
echo past > app.txt; commit "D1: independent of H"; D1=$(git rev-parse HEAD)
echo z >> shared.txt; echo n > notes.txt; commit "D2: touches H's file"; D2=$(git rev-parse HEAD)
echo o > other.txt; commit "D3: says it needs H

Needs: $(echo "$H" | cut -c1-10)"; D3=$(git rev-parse HEAD)
out=$($CDP tick demo 2>&1)
echo "$out" | grep -q "held demo $(echo "$H" | cut -c1-8): migrations/002.sql touches" && ok "H is held, and named" || { no "H held"; echo "$out"; }
echo "$out" | grep -q "demo $(echo "$D2" | cut -c1-8) waits: touches shared.txt, which a waiting commit changed" && ok "D2 waits: it touches H's file" || { no "D2"; echo "$out"; }
echo "$out" | grep -q "demo $(echo "$D3" | cut -c1-8) waits: needs $(echo "$H" | cut -c1-8), which waits" && ok "D3 waits: it says Needs: H" || { no "D3"; echo "$out"; }
echo "$out" | grep -q "shipped demo .* (1 commits) · blip.* · 3 wait behind $(echo "$H" | cut -c1-8)" && ok "D1 shipped, alone, past three that wait" || { no "ship past"; echo "$out"; }
C=$(live); [ "$(cat "$T/prod/app.txt")" = past ] && [ "$C" != "$(git rev-parse HEAD)" ] && ok "prod has D1's change, on a candidate off main" || no "prod content"
git cat-file -e "$C:migrations/002.sql" 2>/dev/null && no "H's file is in prod" || ok "H's files are not in what shipped"
git cat-file -e "$C:notes.txt" 2>/dev/null && no "D2's file is in prod" || ok "D2's file is not in what shipped"
git cat-file -e "$C:other.txt" 2>/dev/null && no "D3's file is in prod" || ok "D3's file is not in what shipped"
[ "$(head -1 "$CD_STATE/gate-saw")" = "$C" ] && ! grep -q 002 "$CD_STATE/gate-saw" && ok "the gate ran on the candidate tree, not on main" || no "gate tree"
[ "$(git -C "$T/origin.git" rev-parse refs/cd/demo)" = "$C" ] && ok "the candidate is on origin as refs/cd/demo" || no "candidate ref"
[ "$(floor)" = "$MAIN0" ] && ok "the floor stays under H" || no "floor"
$CDP tick demo 2>&1 | grep -q . && no "a second tick did something" || ok "a second tick is quiet: same main, same held set"
echo past2 > app.txt; commit "D4: another independent one"; out=$($CDP tick demo 2>&1)
echo "$out" | grep -q "shipped demo .* (1 commits)" && [ "$(cat "$T/prod/app.txt")" = past2 ] && ok "D4 ships past H too" || { no "D4"; echo "$out"; }
echo "$out" | grep -q "held demo" && no "H announced again" || ok "H is not announced again"
out=$($CDP ship demo "$H" 2>&1); M=$(git rev-parse HEAD)
[ "$(git rev-parse "$(live)^{tree}")" = "$(git rev-parse "$M^{tree}")" ] && [ "$(git rev-parse "$(live)^2")" = "$M" ] && [ "$(floor)" = "$M" ] \
  && echo "$out" | grep -q "shipped demo .* (3 commits)" && ok "releasing H ships H, D2 and D3: main's tree, main as a parent, the floor catches up" || { no "release"; echo "$out"; }
git -C "$T/origin.git" merge-base --is-ancestor "$C" "$(live)" && ok "prod only ever fast-forwards along refs/cd/demo" || no "ff"
echo k > app.txt; commit "on main again"; $CDP tick demo >/dev/null 2>&1; onmain && [ "$(git rev-parse "$(live)^2")" = "$(git rev-parse HEAD)" ] && ok "with nothing waiting, the next ship is main's tree with main as a parent" || no "back on main"
echo "7 hold groups: each is its own switch, and the switch itself holds"
cat >> .cd <<'CFG'
hold      words: src/words
holdline  words: * pub const SAYS
CFG
mkdir -p src/words; echo c > src/words/talk.txt; commit "wording rules"; $CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1
echo d >> src/words/talk.txt; commit "edit wording"; $CDP tick demo 2>&1 | grep -q "held demo .*talk.txt touches 'src/words'" && ok "the words group holds" || no "group"
echo "off words" >> .cd; commit "turn the words group off"
$CDP tick demo 2>&1 | grep -q "held demo .*: .cd touches '.cd'" && ok "turning a group off holds for Felix" || no "off holds"
$CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1
echo e >> src/words/talk.txt; echo 'pub const SAYS' >> app.txt; commit "wording edit with the group off"
out=$($CDP tick demo 2>&1); onmain && ok "with the group off, its path and line rules are off" || { no "group off"; echo "$out"; }
echo f >> migrations/001.sql; commit "a migration with words off"; $CDP tick demo 2>&1 | grep -q "held demo" && ok "the other groups still hold" || no "other groups"
$CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1
echo "off nosuch" >> .cd; commit "off for a group that does not exist"; $CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1
echo g > app.txt; commit "harmless"; $CDP tick demo 2>&1 | grep -q "held demo .*off rule 'nosuch' names no hold group" && ok "an off that names no group holds the line" || no "dead off"
sed -i.b '/off nosuch/d' .cd; rm .cd.b; echo "off builtin" >> .cd; commit "builtin off"; $CDP ship demo "$(git rev-parse HEAD)" >/dev/null 2>&1
echo p > x.plist; commit "a plist"; out=$($CDP tick demo 2>&1); onmain && ok "builtin off: a plist ships" || { no "builtin off"; echo "$out"; }
echo "8 signals: an anomaly rolls back; two rollbacks pause the product"
GOOD=$(live); printf 'cp health.json %s/prod/health; echo 9 > %s/prod/errors\n' "$T" "$T" > prod.sh; commit "a build that errors"
out=$($CDP tick demo 2>&1); echo "$out" | grep -q "ROLLED BACK demo .* in [0-9]*s: errors is '9', not max 5, 2 in a row" && [ "$(live)" = "$GOOD" ] && ok "errors over max: rolled back" || { no "errors"; echo "$out"; }
echo "$out" | grep -q "PAUSED demo: 2 rollbacks today" && [ -e "$CD_HOME/STOP.demo" ] && ok "second rollback of the day: paused, one alert" || { no "breaker"; echo "$out"; }
printf 'cp health.json %s/prod/health; cp app.txt %s/prod/app.txt\n' "$T" "$T" > prod.sh; commit "a good build"
$CDP tick demo 2>&1 | grep -q "proven on staging; not shipped: kill switch is on" && [ "$(live)" = "$GOOD" ] && ok "paused: proven, not shipped" || no "paused"
$CDP go demo >/dev/null; out=$($CDP tick demo 2>&1); onmain && ok "cdp go <product> lets it ship" || { no "go product"; echo "$out"; }
GOOD=$(live); printf 'cp health.json %s/prod/health; echo 3 > %s/prod/asks\n' "$T" "$T" > prod.sh; commit "a build that loses asks"
out=$($CDP tick demo 2>&1); echo "$out" | grep -q "ROLLED BACK demo .*: asks is '3', not at least 0.5 × 10" && [ "$(live)" = "$GOOD" ] && ok "asks under half the baseline: rolled back" || { no "baseline"; echo "$out"; }
echo "$out" | grep -q "PAUSED demo: 3 rollbacks today" && [ -e "$CD_HOME/STOP.demo" ] && ok "a third rollback after a go pauses again" || { no "paused again"; echo "$out"; }; $CDP go demo >/dev/null
printf 'cp health.json %s/prod/health; cp app.txt %s/prod/app.txt\n' "$T" "$T" > prod.sh; commit "good again"; $CDP tick demo >/dev/null 2>&1
echo "9 between ships: a young ship is the suspect, an old one is not"
PREV=$(cat "$CD_STATE/demo.previous"); echo 9 > "$T/prod/errors"
out=$($CDP tick demo 2>&1); echo "$out" | grep -q . && { no "one bad sample acted"; echo "$out"; } || ok "one bad sample is noise"
out=$($CDP tick demo 2>&1); echo "$out" | grep -q "ROLLED BACK demo .*: errors is '9', not max 5, 2 in a row, [0-9]*s after the ship. Back on $(echo "$PREV" | cut -c1-8)" \
  && [ "$(live)" = "$PREV" ] && ok "two in a row inside the undo window: the last ship is rolled back" || { no "late rollback"; echo "$out"; }
$CDP go demo >/dev/null; echo h > app.txt; commit "after the late rollback"; out=$($CDP tick demo 2>&1); onmain && ok "the line goes on from there" || { no "after late"; echo "$out"; }
sed -i.b '$s/ [0-9]*$/ 1000/' "$CD_STATE/demo.ships"; rm "$CD_STATE/demo.ships.b"; echo 9 > "$T/prod/errors"; $CDP tick demo >/dev/null 2>&1
out=$($CDP tick demo 2>&1); echo "$out" | grep -q "PAUSED demo: errors is '9'.*not the suspect" && [ -e "$CD_HOME/STOP.demo" ] && onmain \
  && ok "outside the window: paused, not rolled back" || { no "old ship"; echo "$out"; }
out=$($CDP tick demo 2>&1); echo "$out" | grep -q PAUSED && { no "alerted twice"; echo "$out"; } || ok "told once"; echo 0 > "$T/prod/errors"; $CDP go demo >/dev/null
echo "10 the blip probe sees a swap that drops the face"
printf 'rm %s/prod/health; sleep 0.7; cp health.json %s/prod/health; cp app.txt %s/prod/app.txt\n' "$T" "$T" "$T" > prod.sh; commit "a swap with a gap"
$CDP tick demo 2>&1 | grep -q "blip: [1-9][0-9]* failed, 0 slow of [0-9]* probes" && ok "failed probes are counted on the ship line" || no "blip"
printf 'cp health.json %s/prod/health; cp app.txt %s/prod/app.txt\n' "$T" "$T" > prod.sh; commit "atomic again"; $CDP tick demo >/dev/null 2>&1
echo "11 one tick at a time per product"
mkdir "$CD_STATE/demo.lock"; echo $$ > "$CD_STATE/demo.lock/pid"; echo i > app.txt; commit "while a tick runs"
$CDP tick demo 2>&1 | grep -q "a tick is still running" && ! onmain && ok "a live tick keeps the next one out" || no "lock"
echo 99999999 > "$CD_STATE/demo.lock/pid"; out=$($CDP tick demo 2>&1); onmain && ok "a dead tick's lock is taken over" || { no "stale lock"; echo "$out"; }
echo 'sleep 2; exit 0' > gate.sh; echo j > app.txt; commit "slow gate"; n=$(ships)
$CDP tick demo > "$T/t1" 2>&1 & sleep 0.5; $CDP tick demo > "$T/t2" 2>&1; wait
grep -q "a tick is still running" "$T/t2" && [ "$(ships)" = $((n+1)) ] && ok "two ticks a minute apart: one ship, no overlap" || { no "overlap"; cat "$T/t1" "$T/t2"; }
# ── Everyday Vet's rules, on a scratch repo with its real file names ──
git init -q --bare "$T/ev.git"; git clone -q "$T/ev.git" "$T/ev" 2>/dev/null
cd "$T/ev"; git config user.email t@t; git config user.name t
{ cat <<CFG; cat "$HERE/test-ev.hold"; } > .cd
product   ev
prod      echo {sha} >> $T/evprod
health    file://$T/ok
watch     1
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
echo "12 Everyday Vet: consent, STOP, wording, migrations, deletions and flags hold"
evheld "consent: src/consent/legal.rs"        add src/consent/legal.rs "// reworded"
evheld "consent: a new file in src/consent"   add src/consent/new.rs "// new"
evheld "STOP: the send gate, thread/sms.rs"   add src/thread/sms.rs "// x"
evheld "what may be sent: thread/files.rs"    add src/thread/files.rs "// x"
evheld "opt-outs: src/bin/optouts.rs"         add src/bin/optouts.rs "// x"
evheld "consent moved out of src/consent"     git mv src/consent/legal.rs src/desk/legal.rs
evheld "may_text called from the desk"        add src/desk/mod.rs "if may_text(c) { send() }"
evheld "the word STOP in a string"            add src/day/mod.rs 'let s = "Reply STOP to end";'
evheld "STOP added to docs/messaging.md"      add docs/messaging.md "Reply STOP to end"
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
echo "13 Everyday Vet: ordinary work still ships"
evships "desk code and a doc"                 eval 'add src/desk/mod.rs "fn stopped() {} // STOPPED, helpful"; add docs/desk.md "more"'
evships "STOP, HELP and opt-out in a handoff"  eval 'add HANDOFF.md "STOP and HELP replies, the opt-out list"; add docs/desk.md "Reply STOP to opt out"'
evships "a dependency that is not kita"       swap Cargo.toml 1.0.1 1.0.2
echo "14 a hold rule that matches no file holds the line"
add .cd "hold      src/optout* migrations/"; commit "rules for files this repo does not have"
$CDP tick ev 2>&1 | grep -q "touches '.cd'" && ok "a change to .cd holds" || no ".cd"
$CDP ship ev "$(git rev-parse HEAD)" >/dev/null 2>&1; EV=$(git rev-parse HEAD)
add docs/desk.md "harmless"; commit "a harmless doc change"
out=$($CDP tick ev 2>&1); echo "$out" | grep -q "held ev.*hold rule 'src/optout\*' matches no file" && [ "$(cat "$CD_STATE/ev.shipped")" = "$EV" ] \
  && ok "held, and it names the rule" || { no "dead rule"; echo "$out"; }
swap .cd "^hold .*optout.*" "holdline  nosuch\/*.rs secret"; commit "a line rule for no file"
$CDP ship ev "$(git rev-parse HEAD)" >/dev/null 2>&1; EV=$(cat "$CD_STATE/ev.shipped")
[ "$EV" != "$(git rev-parse HEAD)" ] && git cat-file -e "$EV:.cd" && ok "the .cd fix shipped past the doc change that still waits" || no "ship past on ev"
add docs/desk.md "harmless"; commit "another harmless doc change"
out=$($CDP tick ev 2>&1); echo "$out" | grep -q "hold rule 'nosuch/\*.rs secret' matches no file" && [ "$(cat "$CD_STATE/ev.shipped")" = "$EV" ] && ok "a holdline for no file holds too" || { no "dead holdline"; echo "$out"; }
# the fixture is a copy; say so when Everyday Vet's own .cd has moved on
theirs=$(git -C "$HERE/../../dev/everyday-vet-admin" show main:.cd 2>/dev/null | grep -E '^hold') || true
[ -z "$theirs" ] || [ "$theirs" = "$(cat "$HERE/test-ev.hold")" ] || echo "  ! test-ev.hold differs from everyday-vet-admin's .cd: copy its hold lines here"
# ── the rollback contract: `ship.sh --undo {sha}` answers 0, 1 or 2 ──
# A fake ship.sh that keeps one release back, as Everyday Vet's does: --undo
# refuses (1) unless {sha} is the release before the newest, and a second
# undo has nothing kept to go back to. $T/u/rc says how an undo ends.
U=$T/u; mkdir -p "$U"; echo '{"ok":true}' > "$U/health"; git init -q --bare "$T/undo.git"; git clone -q "$T/undo.git" "$T/uapp" 2>/dev/null
cd "$T/uapp"; git config user.email t@t; git config user.name t
cat > .cd <<CFG
product   undo
prod      sh ship.sh {sha}
rollback  sh ship.sh --undo {sha}
health    file://$U/health
watch     2
undo      3600
CFG
cat > ship.sh <<SH
[ ! -e $U/prodfail ] || [ "\$1" = --undo ] || exit 1
if [ "\$1" = --undo ]; then echo "\$2" >> $U/undos; rc=\$(cat $U/rc 2>/dev/null || echo 0)
  [ -f $U/was ] && [ "\$(cat $U/was)" = "\$2" ] || exit 1
  case \$rc in 0) mv $U/was $U/live; mv $U/health.was $U/health ;; 2) mv $U/was $U/live; rm $U/health.was ;; esac; exit \$rc; fi
cp $U/live $U/was 2>/dev/null; cp $U/health $U/health.was; echo "\$1" > $U/live; cp health.json $U/health; echo "\$1" >> $U/ships
SH
echo '{"ok":true}' > health.json; git add -A; git commit -qm "u1"; git push -q origin HEAD:main 2>/dev/null
git rev-parse HEAD > "$U/live"; echo "undo $T/uapp" >> "$CD_HOME/repos"; $CDP init undo "$(cat "$U/live")" >/dev/null
ulive() { cat "$CD_STATE/undo.shipped"; }; uships() { grep -c . "$U/ships" 2>/dev/null || echo 0; }; undos() { grep -c . "$U/undos" 2>/dev/null || echo 0; }
ugood() { echo '{"ok":true}' > health.json; echo "$1" >> notes.txt; commit "$1"; }
ubad() { echo '{"ok":false}' > health.json; echo "$1" >> notes.txt; commit "$1"; }
ugood "u2"; $CDP tick undo >/dev/null 2>&1; [ "$(ulive)" = "$(git rev-parse HEAD)" ] && ok "a good ship on the scratch desk" || no "u2"
echo "15 exit 0: rolled back and smoked; the undone sha is held for good"
G=$(ulive); ubad "u3 breaks health"; B=$(git rev-parse HEAD)
out=$($CDP tick undo 2>&1); echo "$out" | grep -q "ROLLED BACK undo $(short "$B") in [0-9]*s: health1 is 'fail'.*Back on $(short "$G")" && [ "$(ulive)" = "$G" ] && [ "$(cat "$U/live")" = "$G" ] \
  && ok "ship.sh --undo {sha} got the commit it goes back to, and prod is there" || { no "exit 0"; echo "$out"; cat "$U/undos"; }
grep -qx "$B" "$CD_STATE/undo.undone" && ok "the undone sha is held" || no "undone not held"
n=$(uships); u=$(undos); $CDP tick undo >/dev/null 2>&1; $CDP tick undo >/dev/null 2>&1
[ "$(uships)" = "$n" ] && [ "$(undos)" = "$u" ] && ok "never retried, never undone twice" || no "retried"
echo bad > "$CD_STATE/undo.failed"; $CDP tick undo >/dev/null 2>&1
[ "$(uships)" = "$n" ] && ok "held by the undone list even when 'failed' has moved on" || no "undone list"
ugood "u4 the fix"; $CDP tick undo >/dev/null 2>&1; [ "$(ulive)" = "$(git rev-parse HEAD)" ] && ok "the fix ships by the same route" || no "u4"
echo "16 exit 1: refused, nothing moved; nothing ships over the suspect until a person clears it"
echo 1 > "$U/rc"; ubad "u5 breaks health"; B=$(git rev-parse HEAD)
out=$($CDP tick undo 2>&1); echo "$out" | grep -q "ROLLBACK REFUSED undo $(short "$B"): health1.*Nothing moved; $(short "$B") is live. Nothing ships until a person looks: cdp go undo" \
  && [ -e "$CD_STATE/undo.anomaly" ] && ok "refused: alerted, the anomaly kept" || { no "exit 1"; echo "$out"; }
[ "$(ulive)" = "$B" ] && [ "$(cat "$U/live")" = "$B" ] && ok "the record says what really runs: the suspect" || no "refused state"
echo "$out" | grep -q "^.*\[urgent\]" && no "a refusal paged" || ok "a refusal alerts, it does not page"
ugood "u6 a good commit lands"; n=$(uships); u=$(undos)
out=$($CDP tick undo 2>&1); echo "$out" | grep -q "not shipped: undo has an anomaly a person has not cleared" && [ "$(uships)" = "$n" ] && ok "a newer commit is proven, not shipped" || { no "shipped over"; echo "$out"; }
out=$($CDP tick undo 2>&1); [ "$(uships)" = "$n" ] && [ "$(undos)" = "$u" ] && ! echo "$out" | grep -q . && ok "and the next tick is quiet: no ship, no second undo, no second alert" || { no "quiet"; echo "$out"; }
$CDP status | grep -q "^undo .*ANOMALY: rollback of $(short "$B") refused" && ok "status shows the anomaly" || no "status"
echo 0 > "$U/rc"; $CDP go undo >/dev/null; [ ! -e "$CD_STATE/undo.anomaly" ] && ok "cdp go undo clears it" || no "go"
out=$($CDP tick undo 2>&1); [ "$(uships)" = "$n" ] && echo "$out" | grep -q "a signal read bad on its last sample; not shipping" && ok "still unwell: the tick looks before it ships, and waits" || { no "looked"; echo "$out"; }
out=$($CDP tick undo 2>&1); echo "$out" | grep -q "ROLLED BACK undo $(short "$B")" && [ "$(uships)" = "$n" ] && [ "$(cat "$U/live")" = "$(cat "$CD_STATE/undo.previous")" ] \
  && ok "two bad: the suspect is rolled back first, nothing shipped in between" || { no "roll before ship"; echo "$out"; }
$CDP go undo >/dev/null; out=$($CDP tick undo 2>&1); [ "$(ulive)" = "$(git rev-parse HEAD)" ] && ok "then the good commit ships" || { no "u6"; echo "$out"; }
echo "17 exit 2: rolled back, but a face does not answer: page and pause"
echo 2 > "$U/rc"; G=$(ulive); ubad "u7 breaks health"; B=$(git rev-parse HEAD)
out=$($CDP tick undo 2>&1); echo "$out" | grep -q "\[urgent\] ROLLED BACK undo $(short "$B") in [0-9]*s, and a face or the smoke does not answer on $(short "$G").*undo is paused" \
  && [ -e "$CD_HOME/STOP.undo" ] && [ -e "$CD_STATE/undo.anomaly" ] && ok "paged, paused" || { no "exit 2"; echo "$out"; }
[ "$(ulive)" = "$G" ] && grep -qx "$B" "$CD_STATE/undo.undone" && ok "the record is back on $(short "$G") and the undone sha is held" || no "exit 2 state"
echo 0 > "$U/rc"; echo '{"ok":true}' > "$U/health"; ugood "u8"; n=$(uships)
$CDP tick undo >/dev/null 2>&1; $CDP tick undo >/dev/null 2>&1; [ "$(uships)" = "$n" ] && ok "paused: nothing ships" || no "shipped while paused"
$CDP go undo >/dev/null; $CDP tick undo >/dev/null 2>&1; [ "$(ulive)" = "$(git rev-parse HEAD)" ] && ok "cdp go undo, and it ships" || no "after go"
echo "18 any other exit (3, ssh's 255): what is live is unknown: page and pause"
echo 3 > "$U/rc"; ubad "u9 breaks health"; B=$(git rev-parse HEAD); n=$(uships)
out=$($CDP tick undo 2>&1); echo "$out" | grep -q "\[urgent\] ROLLBACK FAILED undo $(short "$B") (exit 3).*What is live is unknown" && [ -e "$CD_HOME/STOP.undo" ] \
  && grep -qx "$B" "$CD_STATE/undo.undone" && ok "paged, paused, the suspect never retried" || { no "exit 3"; echo "$out"; }
echo "19 between ships: one bad sample and a new commit; the live ship is rolled back before anything ships"
# the person: puts the desk back on what the record says, by hand, then go
cp "$CD_STATE/undo.shipped" "$U/live"; rm -f "$U/was"; echo 0 > "$U/rc"; echo '{"ok":true}' > "$U/health"; $CDP go undo >/dev/null
ugood "u10"; $CDP tick undo >/dev/null 2>&1
L=$(ulive); P0=$(cat "$CD_STATE/undo.previous"); echo '{"ok":false}' > "$U/health"; $CDP tick undo >/dev/null 2>&1
echo '{"ok":false}' > "$U/health"; ugood "u11 lands on an unwell box"; n=$(uships)
out=$($CDP tick undo 2>&1); [ "$(uships)" = "$n" ] && echo "$out" | grep -q "ROLLED BACK undo $(short "$L")" && [ "$(cat "$U/live")" = "$P0" ] \
  && ok "the young ship is undone to $(short "$P0"), and u11 did not ship over it" || { no "one back"; echo "$out"; }
echo "20 a prod step that fails, and an undo that is refused: the record stays on what ran before"
echo '{"ok":true}' > "$U/health"; $CDP go undo >/dev/null; $CDP tick undo >/dev/null 2>&1; $CDP tick undo >/dev/null 2>&1; L=$(ulive); [ "$(cat "$U/live")" = "$L" ] || no "20 setup"
touch "$U/prodfail"; ugood "u12 its prod step fails"; out=$($CDP tick undo 2>&1)
echo "$out" | grep -q "ROLLBACK REFUSED undo .*: prod step failed. Nothing moved; $(short "$L") is live" && [ "$(ulive)" = "$L" ] && [ -e "$CD_STATE/undo.anomaly" ] \
  && ok "refused after a failed prod step: the record stays on $(short "$L"), and a person is asked" || { no "prod fail"; echo "$out"; }
rm "$U/prodfail"; ugood "u13"; n=$(uships); $CDP tick undo >/dev/null 2>&1; [ "$(uships)" = "$n" ] && ok "and nothing ships until they clear it" || no "shipped past a refusal"
[ $fail = 0 ] && echo PASS || { echo FAIL; exit 1; }
