#!/bin/sh
# Continuous deployment: one tick ships what a merge to main brought, if the
# policy (mesh-lead/cd/policy.md) allows. Plain sh, no server; run by hand or
# by a launchd job every minute (cdp/com.dorkyrobot.cdp.plist).
#
#   cdp.sh init <product> <sha>     say which commit is live now (the baseline)
#   cdp.sh tick [product]           look at origin/main and ship what is allowed
#   cdp.sh ship <product> <sha>     Felix's yes to a held commit (the full sha)
#   cdp.sh stop | go  [product]     the kill switch on | off, for all or for one
#   cdp.sh status                   what each product has shipped, held, failed
#
# Files (CD_HOME, default ~/.config/cdp):
#   STOP, STOP.<product>  the kill switch. While one exists nothing of that
#            product reaches prod; the gate, staging and smoke still run, so a
#            `go` finds a proven candidate. STOP.<product> is also what the
#            circuit breaker sets after two rollbacks in a day.
#   repos    one line per product: <product> <path to a checkout>
# State (CD_STATE, default ~/.local/state/cdp): per product, what is live,
# the floor, the held, released and failed shas, a line per ship and per
# rollback, and the signal counters.
#
# Each repo's own <checkout>/.cd says how to gate, stage, smoke, ship, roll
# back and watch it, and what holds for Felix. It is read from the LIVE
# commit, so a commit cannot loosen the rules it is itself judged by.
#
# Ship-past: every commit of main since the floor is judged on its own diff.
# A commit that holds, and every later commit that touches one of its files
# (or says `Needs: <sha>`), is withheld; the rest ship as one candidate: main
# with the withheld commits reverted on top, gated and proven as its own tree
# and pushed to refs/cd/<product> so the desk's own release path can fetch
# it. When nothing is withheld the candidate is main itself.
set -u
CD_HOME=${CD_HOME:-$HOME/.config/cdp}
CD_STATE=${CD_STATE:-$HOME/.local/state/cdp}
WATCH_EVERY=${CD_WATCH_EVERY:-10}      # seconds between samples in the post-ship watch
mkdir -p "$CD_STATE"

say() { printf '%s cdp: %s\n' "$(date +%H:%M:%S)" "$*"; }
# ntfy: the same topic mesh-watch uses. Without one, the line is only printed.
ntfy() { say "$*"
  url=${CD_NTFY:-${MESH_WATCH_NTFY:-$(cat "$CD_HOME/ntfy" 2>/dev/null)}}
  [ -z "$url" ] || curl -sS -f -m 20 -H "Title: cdp" --data-binary "$*" "$url" >/dev/null 2>&1 || true; }
st() { cat "$CD_STATE/$P.$1" 2>/dev/null || true; }          # read state
put() { printf '%s\n' "$2" > "$CD_STATE/$P.$1"; }            # write state
has() { grep -qx "$2" "$CD_STATE/$P.$1" 2>/dev/null; }       # is $2 a line of state $1
# a key of the live commit's .cd (CDTXT): first word is the key, the rest the value
cfg() { printf '%s\n' "$CDTXT" | sed -n "s/^$1[[:space:]][[:space:]]*//p" | head -1; }
cfgall() { printf '%s\n' "$CDTXT" | sed -n "s/^$1[[:space:]][[:space:]]*//p"; }
sub() { printf '%s' "$1" | sed "s/{sha}/$2/g"; }
short() { echo "$1" | cut -c1-8; }
now_s() { date +%s; }
# is $1 a number that stands $2 (<= or >=) $3
numck() { awk -v v="$1" -v op="$2" -v a="$3" 'BEGIN{ if (v !~ /^-?[0-9]+(\.[0-9]+)?$/) exit 1
  if (op=="<=") exit !(v+0<=a+0); if (op==">=") exit !(v+0>=a+0); exit 1 }'; }

# ── one tick at a time per product ──
# mkdir is atomic; the pid inside says whether the holder is still alive, so a
# tick killed mid-gate does not lock its product forever.
LOCK=
lock() { d=$CD_STATE/$P.lock
  if ! mkdir "$d" 2>/dev/null; then
    pid=$(cat "$d/pid" 2>/dev/null)
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then say "$P: a tick is still running (pid $pid)"; return 1; fi
    rm -rf "$d"; mkdir "$d" 2>/dev/null || return 1
  fi
  echo $$ > "$d/pid"; LOCK=$d; }
unlock() { [ -z "$LOCK" ] || rm -rf "$LOCK"; LOCK=; }
trap unlock EXIT

# ── hold rules ──
# `hold [<group>:] <globs>` and `holdline [<group>:] <glob> <ERE>` may repeat;
# `off <group>` turns a group off. The built-in paths are the group `builtin`;
# a rule without a group is in the group `hold` or `holdline`.
is_off() { case " $OFFS " in *" $1 "*) return 0 ;; esac; return 1; }
rules() { cfgall "$1" | while read -r first rest; do
  case "$first" in *:) g=${first%:}; line=$rest ;; *) g=$1; line="$first${rest:+ $rest}" ;; esac
  is_off "$g" || printf '%s\n' "$line"; done; }
groups() { { cfgall hold; cfgall holdline; } | sed -n 's/^\([^ :]*\):.*/\1/p' | tr '\n' ' '; }
# The paths on stdin that glob $1 covers. sh `case` globs, which let * cross
# a /; a name counts at any depth and a directory covers what is under it.
covered() { g=${1%/}; g=${g%/\*\*}; while IFS= read -r f; do
  # shellcheck disable=SC2254
  case "$f" in $g|$g/*|*/$g|*/$g/*) printf '%s\n' "$f" ;; esac
done; }
# the lines a diff on stdin adds or removes, without their + or -
changed() { awk '/^diff --git/{h=1} /^@@/{h=0;next} !h && /^[+-]/{print substr($0,2)}'; }
# `holdline <glob> <ERE>`: hold when a line added or removed in a file the
# glob covers matches. For what has no path of its own: a word, a flag, a pin.
hold_lines() {
  rules holdline | while read -r g re; do
    [ -n "$re" ] && { echo | grep -qE -- "$re"; [ $? -le 1 ]; } || { echo "hold rule '$g $re' is not a pattern grep can read"; exit; }
    [ -n "$(echo "$tracked" | covered "$g" | head -1)" ] || { echo "hold rule '$g $re' matches no file"; exit; }
    fs=$(echo "$files" | covered "$g"); [ -n "$fs" ] || continue
    hit=$(echo "$fs" | tr '\n' '\0' | xargs -0 git --literal-pathspecs -C "$DIR" diff --no-renames -U0 "$FROM" "$TO" -- | changed | grep -E -m1 -- "$re")
    [ -z "$hit" ] || { echo "a changed line matches holdline '$g': $(printf '%s' "$hit" | cut -c1-80)"; exit; }
  done
}
# why the diff $1..$2 needs Felix's yes; empty when nothing does. Rules and
# the file list they are checked against both come from LIVE.
hold_reason() { FROM=$1; TO=$2
  set -f   # we are inside $( ): keep every glob a pattern, never the cwd's files
  # --no-renames: a held file that moves must still show under its held name
  files=$(git -c core.quotePath=false -C "$DIR" diff --no-renames --name-only "$FROM" "$TO" 2>/dev/null) || { echo "cannot read the diff"; return; }
  [ -n "$files" ] || { echo "empty diff"; return; }
  # A rule that matches nothing protects nothing: a typo or a moved file would
  # let the very change it was written for ship. The same for an `off` that
  # names no group: the switch Felix meant to flip is not the one that moved.
  tracked=$(git -c core.quotePath=false -C "$DIR" ls-tree -r --name-only "$LIVE") || { echo "cannot list the live files"; return; }
  for o in $OFFS; do case " $(groups) builtin hold holdline " in *" $o "*) ;; *) echo "off rule '$o' names no hold group"; return ;; esac; done
  for g in $(rules hold); do
    [ -n "$(echo "$tracked" | covered "$g" | head -1)" ] || { echo "hold rule '$g' matches no file"; return; }
  done
  builtin=; is_off builtin || builtin='.cd *.plist *migrat* *schema* *consent* *optout* *opt-out* *stop_gate* *tcpa* *dns* *billing* *payment*'
  for g in $(rules hold) $builtin; do
    f=$(echo "$files" | covered "$g" | head -1); [ -z "$f" ] || { echo "$f touches '$g'"; return; }
  done
  r=$(hold_lines); [ -z "$r" ] || { echo "$r"; return; }
  git -C "$DIR" log -1 --format=%B "$TO" | grep -qi '^Hold:' && { echo "the commit says Hold:"; return; }
  :
}

# ── which commits ship, which wait ──
# Sets WITHHELD (shas, oldest first), FIRSTHELD (the oldest of them) and
# WAITS (one line per commit that waits behind another). A commit waits when
# it holds on its own diff (unless Felix released it), says `Needs: <sha>` of
# a waiting commit, or touches a file a waiting commit touched. Main is
# walked by first parent, so a merge is one unit, reverted with -m 1.
judge() { WITHHELD=; FIRSTHELD=; WAITS=; wf=$CD_STATE/$P.wfiles; : > "$wf"
  for c in $(git -C "$DIR" rev-list --first-parent --reverse "$FLOOR..$TARGET"); do
    cf=$(git -c core.quotePath=false -C "$DIR" diff --no-renames --name-only "$c^" "$c" 2>/dev/null)
    why=
    if ! has released "$c"; then why=$(hold_reason "$c^" "$c"); fi
    if [ -z "$why" ] && [ -n "$WITHHELD" ]; then
      for n in $(git -C "$DIR" log -1 --format=%B "$c" | sed -n 's/^Needs: *//p'); do
        for w in $WITHHELD; do case "$w" in "$n"*) why="needs $(short "$w"), which waits"; break 2 ;; esac; done
      done
      if [ -z "$why" ] && [ -s "$wf" ]; then
        f=$(printf '%s\n' "$cf" | grep -Fx -m1 -f "$wf") || true
        [ -z "$f" ] || why="touches $f, which a waiting commit changed"
      fi
    fi
    [ -n "$why" ] || continue
    WITHHELD="$WITHHELD $c"; [ -n "$FIRSTHELD" ] || FIRSTHELD=$c
    printf '%s\n' "$cf" | grep . >> "$wf"
    case "$why" in needs*|touches*,*) WAITS="$WAITS$P $(short "$c") waits: $why
" ;;
      *) has held "$c" || { printf '%s\n' "$c" >> "$CD_STATE/$P.held"; ntfy "held $P $(short "$c"): $why. Release: cdp ship $P $c"; } ;;
    esac
  done
}
# Main with the withheld commits reverted, newest first, as a commit whose sha
# is the same each tick (fixed author, dates and message), so a candidate is
# gated once and a failed one is not retried. Prints the sha; nothing if a
# revert does not apply, in which case the whole line waits.
candidate() {
  [ -n "$WITHHELD" ] || { echo "$TARGET"; return; }
  git -C "$W" checkout -q --detach "$TARGET" || return
  d=$(git -C "$DIR" log -1 --format=%cI "$TARGET")
  for w in $(echo "$WITHHELD" | tr ' ' '\n' | grep . | sed '1!G;h;$!d'); do
    m=; [ "$(git -C "$DIR" rev-list --parents -n1 "$w" | wc -w)" -le 2 ] || m="-m 1"
    GIT_AUTHOR_DATE=$d GIT_COMMITTER_DATE=$d git -C "$W" -c user.name=cdp -c user.email=cdp@mesh \
      revert --no-edit $m "$w" >/dev/null 2>&1 || { git -C "$W" revert --abort 2>/dev/null; git -C "$W" checkout -q --detach "$TARGET"
        say "$P: the revert of $(short "$w") does not apply on main; the line waits" >&2; return; }
  done
  git -C "$W" rev-parse HEAD
}
# the first withheld commit's parent: everything before it is on prod as on main
new_floor() { if [ -n "$FIRSTHELD" ]; then git -C "$DIR" rev-parse "$FIRSTHELD^"; else echo "$TARGET"; fi; }

# ── signals ──
# `signal <name> max <n> <cmd>` prints a number; anomaly above n
# `signal <name> min <n> <cmd>` prints a number; anomaly below n
# `signal <name> ok <cmd>`      prints ok or fail
# `signal <name> baseline <f> <cmd>` a number, sampled before prod; anomaly
#    when a later sample falls under f × that one
# The `health` URLs are the signals health1, health2… (ok when the body says
# "ok":true). One bad sample is noise; two in a row on one signal is an
# anomaly. `sample once` asks whether everything is good right now.
sample() { th=2; [ "$1" != once ] || th=1; i=0
  { for u in $(cfg health); do i=$((i+1)); echo "health$i ok curl -sS -f -m 10 $u 2>/dev/null | grep -q '\"ok\":true' && echo ok || echo fail"; done; cfgall signal; } \
  | while read -r name kind rest; do
    [ -n "$name" ] || continue
    case "$kind" in ok) cmd=$rest; arg= ;; *) arg=${rest%% *}; cmd=${rest#* } ;; esac
    v=$(cd "$DIR" && sh -c "$cmd" 2>/dev/null | head -1 | tr -d ' ')
    if [ "$1" = baseline ]; then [ "$kind" != baseline ] || put "base.$name" "$v"; continue; fi
    want="$kind${arg:+ $arg}"
    case "$kind" in
      ok) [ "$v" = ok ] ;;
      max) numck "$v" "<=" "$arg" ;;
      min) numck "$v" ">=" "$arg" ;;
      baseline) b=$(st "base.$name")
        if [ -z "$b" ] || [ "$b" = 0 ]; then true; else want="at least $arg × $b"; numck "$v" ">=" "$(awk -v f="$arg" -v b="$b" 'BEGIN{print f*b}')"; fi ;;
      *) false ;;
    esac && { put "bad.$name" 0; continue; }
    n=$(st "bad.$name"); n=$(( ${n:-0} + 1 )); put "bad.$name" "$n"
    [ "$n" -lt "$th" ] || { echo "$name is '$v', not $want${th:+, $n in a row}"; break; }
  done
}
clear_bad() { rm -f "$CD_STATE/$P".bad.* 2>/dev/null; }

# ── the blip: a probe across the prod step ──
# One request every 200 ms to the `blip` URL (default the first health URL),
# counting failed and slow (>1 s) answers, so every ship line says what a
# person at the desk could have felt.
blip_start() { u=$(cfg blip); u=${u:-$(cfg health | awk '{print $1}')}; BLIPOUT=$CD_STATE/$P.blip; rm -f "$BLIPOUT" "$BLIPOUT.stop"; BLIPPID=
  [ -n "$u" ] || return 0
  ( n=0; bad=0; slow=0; while [ ! -e "$BLIPOUT.stop" ]; do
      t=$(curl -sS -f -o /dev/null -m 2 -w '%{time_total}' "$u" 2>/dev/null) || { bad=$((bad+1)); t=0; }
      n=$((n+1)); numck "$t" "<=" 1 || slow=$((slow+1)); sleep 0.2; done
    echo "$n $bad $slow" > "$BLIPOUT" ) & BLIPPID=$!; }
blip_stop() { [ -n "${BLIPPID:-}" ] || { BLIP="no probe"; return; }
  touch "$BLIPOUT.stop"; wait "$BLIPPID" 2>/dev/null; set -- $(cat "$BLIPOUT" 2>/dev/null || echo 0 0 0)
  BLIP="$2 failed, $3 slow of $1 probes"; BLIPPID=; }

shipped_today() { grep -c "^$(date +%F) " "$CD_STATE/$P.ships" 2>/dev/null || echo 0; }
rolled_today() { grep -c "^$(date +%F) " "$CD_STATE/$P.rollbacks" 2>/dev/null || echo 0; }
stopped() { [ -e "$CD_HOME/STOP" ] || [ -e "$CD_HOME/STOP.$P" ]; }

# ── one product, one tick ──
tick() {
  P=$1; DIR=$(awk -v p="$P" '$1==p {print $2}' "$CD_HOME/repos")
  [ -n "$DIR" ] || { say "$P: not in $CD_HOME/repos"; return 1; }
  LIVE=$(st shipped); [ -n "$LIVE" ] || { say "$P: no baseline; run: cdp init $P <sha>"; return 1; }
  FLOOR=$(st floor); FLOOR=${FLOOR:-$LIVE}
  lock || return 0
  git -C "$DIR" fetch -q origin main || { say "$P: fetch failed (GitHub unreachable?); unknown, not acting"; unlock; return 0; }
  TARGET=$(git -C "$DIR" rev-parse origin/main)
  git -C "$DIR" merge-base --is-ancestor "$FLOOR" "$TARGET" || { ntfy "$P: floor $FLOOR is not an ancestor of main; holding"; unlock; return 0; }
  CDTXT=$(git -C "$DIR" show "$LIVE:.cd" 2>/dev/null) || { say "$P: the live commit has no .cd; nothing ships"; unlock; return 0; }
  OFFS=$(cfgall off | tr '\n' ' ')
  # the work tree: the candidate is built, gated and shipped from here. A
  # repo whose build names siblings by relative path (Everyday Vet:
  # ../../kita2) sets `workdir` to a path at the same depth as its checkout.
  W=$(cfg workdir | sed "s#^~#$HOME#"); W=${W:-$CD_STATE/work/$P}
  [ -d "$W" ] || git -C "$DIR" worktree add -q --detach "$W" "$TARGET" || { unlock; return 1; }
  [ "$TARGET" != "$LIVE" ] || { watch_between; unlock; return 0; }      # nothing new on main

  judge; key="$TARGET$WITHHELD"
  if [ "$(st candkey)" = "$key" ] && [ -n "$(st cand)" ]; then CAND=$(st cand); else
    CAND=$(candidate); printf '%s' "$WAITS"
    [ -n "$CAND" ] || { watch_between; unlock; return 0; }
    put cand "$CAND"; put candkey "$key"; fi
  if [ "$CAND" = "$LIVE" ] || [ "$(git -C "$DIR" rev-parse "$CAND^{tree}")" = "$(git -C "$DIR" rev-parse "$LIVE^{tree}")" ]; then
    watch_between; unlock; return 0; fi                      # everything new waits
  [ "$(st failed)" != "$CAND" ] || { watch_between; unlock; return 0; }   # already failed: wait for a new commit
  short=$(short "$CAND"); export CD_SHA=$CAND CD_DIR=$W
  git -C "$W" checkout -q --detach "$CAND" || { unlock; return 1; }
  # off main: the desk's release path fetches origin, so the candidate must be there
  [ "$CAND" = "$TARGET" ] || git -C "$W" push -q -f origin "$CAND:refs/cd/$P" 2>/dev/null || { ntfy "$P $short: cannot push the candidate to origin refs/cd/$P"; unlock; return 1; }
  if [ "$(st proven)" != "$CAND" ]; then
    for step in gate stage smoke; do
      c=$(cfg $step); [ -n "$c" ] || continue
      say "$P $short: $step"
      ( cd "$W" && sh -c "$(sub "$c" "$CAND")" ) > "$CD_STATE/$P.$step.log" 2>&1 || {
        put failed "$CAND"; ntfy "$P $short: $step failed, nothing shipped (log: $CD_STATE/$P.$step.log)"; unlock; return 1; }
    done
    put proven "$CAND"
  fi
  if stopped; then ntfy "$P $short proven on staging; not shipped: kill switch is on"; unlock; return 0; fi

  # ── prod, then watch ──
  say "$P $short: prod"
  sample baseline; clear_bad; blip_start
  if ! ( cd "$W" && sh -c "$(sub "$(cfg prod)" "$CAND")" ) > "$CD_STATE/$P.prod.log" 2>&1; then
    blip_stop; roll "prod step failed" "$LIVE"; unlock; return 1
  fi
  blip_stop
  for_s=$(cfg watch); for_s=${for_s:-300}; elapsed=0
  while [ "$elapsed" -lt "$for_s" ]; do
    sleep "$WATCH_EVERY"; elapsed=$((elapsed+WATCH_EVERY))
    a=$(sample watch); [ -z "$a" ] || { roll "$a" "$LIVE"; unlock; return 1; }
  done
  record_ship; unlock
}
# Between ships every tick samples the signals once. An anomaly inside `undo`
# seconds (default 3600) of the last ship rolls that ship back: it is still
# the suspect. Later, the code has earned its place: one alert and a pause,
# so nothing new lands on a box that is unwell.
watch_between() {
  [ -n "$(cfg health)$(cfgall signal)" ] || return 0
  a=$(sample watch); [ -n "$a" ] || return 0
  last=$(tail -1 "$CD_STATE/$P.ships" 2>/dev/null | awk '{print $4}'); u=$(cfg undo); u=${u:-3600}
  prev=$(st previous)
  if [ -n "$last" ] && [ -n "$prev" ] && [ $(( $(now_s) - last )) -lt "$u" ] && [ "$prev" != "$LIVE" ]; then
    short=$(short "$LIVE"); CAND=$LIVE
    roll "$a, $(( $(now_s) - last ))s after the ship" "$prev" && { put shipped "$prev"; put floor "$(st pfloor)"; }
  else
    has alerted "${a%% *}" || { printf '%s\n' "${a%% *}" >> "$CD_STATE/$P.alerted"; touch "$CD_HOME/STOP.$P"
      ntfy "PAUSED $P: $a, and the last ship is older than ${u}s, so it is not the suspect. Nothing new ships until: cdp go $P"; }
  fi
}
# the prod step, the watch or a later signal failed: put $2 back, and say how long it took
roll() { put failed "$CAND"; rb=$(cfg rollback); t0=$(now_s)
  if [ -n "$rb" ] && ( cd "$W" && sh -c "$(sub "$rb" "$2")" ) > "$CD_STATE/$P.rollback.log" 2>&1; then
    i=0; while [ "$i" -lt 60 ]; do clear_bad; [ -n "$(sample once)" ] || break; sleep 1; i=$((i+1)); done
    t=$(( $(now_s) - t0 )); date +"%F %T $CAND ${t}s" >> "$CD_STATE/$P.rollbacks"
    ntfy "ROLLED BACK $P $short in ${t}s: $1. Back on $(short "$2")."
    [ "$(rolled_today)" -lt 2 ] || [ -e "$CD_HOME/STOP.$P" ] || { touch "$CD_HOME/STOP.$P"; ntfy "PAUSED $P: $(rolled_today) rollbacks today. Nothing ships until: cdp go $P"; }
    clear_bad; return 0
  else
    touch "$CD_HOME/STOP.$P"; date +"%F %T $CAND failed" >> "$CD_STATE/$P.rollbacks"
    ntfy "ROLLBACK FAILED $P $short: $1. $P is paused. Needs a person (log: $CD_STATE/$P.rollback.log)"; return 1
  fi
}
record_ship() {
  n=0; tags=""; untagged=""; srcs=""; was=" $(st included | tr '\n' ' ') "
  for c in $(git -C "$DIR" rev-list --first-parent "$FLOOR..$TARGET"); do
    case " $WITHHELD " in *" $c "*) continue ;; esac
    srcs="$srcs $c"; case "$was" in *" $c "*) continue ;; esac     # shipped past a hold earlier
    n=$((n+1))
    t=$(git -C "$DIR" log -1 --format=%B "$c" | sed -n 's/^Ready: *//p' | head -1)
    if [ -n "$t" ]; then tags="$tags $t"; else untagged="$untagged $(short "$c")"; fi
  done
  if [ -n "$untagged" ]; then
    d=${CD_OFFLIST:-$HOME/Projects/head-of-product/offlist}; mkdir -p "$d"
    printf -- '- %s %s: shipped without a Ready tag:%s\n' "$(date +%F)" "$P" "$untagged" >> "$d/$(date +%F).md"
  fi
  # the org kita log: one entry per release, each commit an openable source
  if [ -x "${CD_ORG:-$HOME/Projects/dorky_robot/org-kita/target/debug/org}" ]; then
    set -- add "shipped $P $short (tags:${tags:- none})" --by a:cdp --known agent-said --as a:cdp --area deploys
    for c in $srcs; do set -- "$@" --source "commit $P@$c"; done
    "${CD_ORG:-$HOME/Projects/dorky_robot/org-kita/target/debug/org}" "$@" >/dev/null 2>&1 || say "org log write failed (not blocking)"
  fi
  put previous "$LIVE"; put pfloor "$FLOOR"; put included "$(echo "$srcs" | tr ' ' '\n')"
  date +"%F %T $CAND $(now_s)" >> "$CD_STATE/$P.ships"; put shipped "$CAND"; put floor "$(new_floor)"
  w=$(echo "$WITHHELD" | wc -w | tr -d ' '); behind=; [ "$w" = 0 ] || behind=" · $w wait behind $(short "$FIRSTHELD")"
  ntfy "shipped $P $short ($n commits) · blip: $BLIP$behind · rollback: $(cfg rollback | head -c 60)"
}

case "${1:-tick}" in
  init)    P=$2; put shipped "$3"; put floor "$3"; say "$P baseline $3" ;;
  ship)    P=$2; has released "$3" || printf '%s\n' "$3" >> "$CD_STATE/$P.released"; tick "$2" ;;
  stop)    mkdir -p "$CD_HOME"; touch "$CD_HOME/STOP${2:+.$2}"; say "kill switch ON${2:+ for $2}" ;;
  go)      rm -f "$CD_HOME/STOP${2:+.$2}"; say "kill switch off${2:+ for $2}" ;;
  status)  for f in "$CD_STATE"/*.shipped; do P=$(basename "$f" .shipped)
             printf '%s live=%s floor=%s held=%s failed=%s today=%s rollbacks=%s%s\n' "$P" "$(short "$(st shipped)")" "$(short "$(st floor)")" "$(st held | grep -c .)" "$(short "$(st failed)")" "$(shipped_today)" "$(rolled_today)" "$([ -e "$CD_HOME/STOP.$P" ] && echo ' PAUSED')"; done
           [ ! -e "$CD_HOME/STOP" ] || echo "kill switch: ON" ;;
  tick)    if [ -n "${2:-}" ]; then tick "$2"; else awk '!/^#/ && NF {print $1}' "$CD_HOME/repos" | while read -r p; do tick "$p"; done; fi ;;
  *) sed -n '2,10p' "$0"; exit 64 ;;
esac
