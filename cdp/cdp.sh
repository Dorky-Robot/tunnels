#!/bin/sh
# Continuous deployment: one tick ships what a merge to main brought, if the
# policy (mesh-lead/cd/policy.md) allows. Plain sh, no server; run by hand
# or by a launchd job every 5 minutes.
#
#   cdp init <product> <sha>   say which commit is live now (the baseline)
#   cdp.sh tick [product]         look at origin/main and ship what is allowed
#   cdp.sh ship <product> <sha>     Felix's yes to a held commit: skip the hold test once
#   cdp.sh now <product>           Felix's urgent override of clinic hours, once
#   cdp.sh stop | go               the kill switch on | off
#   cdp.sh status                 what each product has shipped, held, failed
#
# Files (CD_HOME, default ~/.config/cdp):
#   STOP     the kill switch. While it exists nothing reaches prod; the gate,
#            staging and smoke still run, so a yes finds a proven commit.
#   repos    one line per product: <product> <path to a checkout>
# State (CD_STATE, default ~/.local/state/cdp): per product, the last shipped
# sha, the held and failed sha, and a line per prod deploy (for the daily cap).
# Each repo's own <checkout>/.cd says how to gate, stage, smoke, ship, roll
# back and watch it. It is read from the LIVE commit, so a commit cannot
# loosen the rules it is itself judged by.
set -u
CD_HOME=${CD_HOME:-$HOME/.config/cdp}
CD_STATE=${CD_STATE:-$HOME/.local/state/cdp}
WATCH_FOR=${CD_WATCH_FOR:-300}; WATCH_EVERY=${CD_WATCH_EVERY:-30}
mkdir -p "$CD_STATE"

say() { printf '%s cdp: %s\n' "$(date +%H:%M:%S)" "$*"; }
# ntfy: the same topic mesh-watch uses. Without one, the line is only printed.
ntfy() { say "$*"
  url=${CD_NTFY:-${MESH_WATCH_NTFY:-$(cat "$CD_HOME/ntfy" 2>/dev/null)}}
  [ -z "$url" ] || curl -sS -f -m 20 -H "Title: cdp" --data-binary "$*" "$url" >/dev/null 2>&1 || true; }
st() { cat "$CD_STATE/$P.$1" 2>/dev/null || true; }          # read state
put() { printf '%s\n' "$2" > "$CD_STATE/$P.$1"; }            # write state
# key from the live commit's .cd: first word is the key, the rest the value
cfg() { git -C "$DIR" show "$LIVE:.cd" 2>/dev/null | sed -n "s/^$1[[:space:]][[:space:]]*//p" | head -1; }
cfgall() { git -C "$DIR" show "$LIVE:.cd" 2>/dev/null | sed -n "s/^$1[[:space:]][[:space:]]*//p"; }
sub() { printf '%s' "$1" | sed "s/{sha}/$2/g"; }

# ── policy checks ──
# a path or a commit message that needs Felix's yes; empty when nothing does
hold_reason() {
  files=$(git -C "$DIR" diff --name-only "$LIVE" "$TARGET" 2>/dev/null) || { echo "cannot read the diff"; return; }
  [ -n "$files" ] || { echo "empty diff"; return; }
  # sh `case` globs, which let * cross a /; the defaults hold for everything
  globs="$(cfgall hold) .cd *.plist *migrat* *schema* *consent* *optout* *opt-out* *stop_gate* *tcpa* *dns* *billing* *payment*"
  for f in $files; do for g in $globs; do
    # shellcheck disable=SC2254
    case "$f" in $g|$g/*|*/$g|*/$g/*) echo "$f touches '$g'"; return ;; esac
  done; done
  git -C "$DIR" log --format=%B "$LIVE..$TARGET" | grep -qi '^Hold:' && { echo "a commit says Hold:"; return; }
  # only the LIVE commit must have a .cd at all: no rules, no shipping
  cfg prod >/dev/null || true
}
# clinic hours: `hours deny Mon-Fri 07:30-19:30 America/New_York` refuses inside
# that window. CD_NOW="<weekday 1-7> <HHMM>" stands in for the clock in tests.
in_denied_hours() {
  h=$(cfg hours); [ -n "$h" ] || return 1
  # `hours cmd <command>` asks the product for its own hours at ship time
  # (Sara can change Everyday Vet's in Settings); the command prints the same line
  case "$h" in "cmd "*) h=$(sh -c "${h#cmd }" 2>/dev/null) || return 0 ;; esac   # cannot ask: wait
  [ -z "${CD_FORCE_HOURS:-}" ] || return 1
  set -- $h; days=$2; win=$3; tz=${4:-UTC}
  now=${CD_NOW:-$(TZ=$tz date '+%u %H%M')}; d=${now% *}; t=${now#* }
  case "$days" in Mon-Fri) [ "$d" -le 5 ] || return 1 ;; Mon-Sun|Everyday) : ;; *) return 0 ;; esac
  from=$(echo "${win%-*}" | tr -d :); to=$(echo "${win#*-}" | tr -d :)
  [ "$t" -ge "$from" ] && [ "$t" -lt "$to" ]
}
shipped_today() { grep -c "^$(date +%F) " "$CD_STATE/$P.ships" 2>/dev/null || echo 0; }

# ── one product, one tick ──
tick() {
  P=$1; DIR=$(awk -v p="$P" '$1==p {print $2}' "$CD_HOME/repos")
  [ -n "$DIR" ] || { say "$P: not in $CD_HOME/repos"; return 1; }
  LIVE=$(st shipped); [ -n "$LIVE" ] || { say "$P: no baseline; run: cdp init $P <sha>"; return 1; }
  git -C "$DIR" fetch -q origin main || { say "$P: fetch failed (GitHub unreachable?); unknown, not acting"; return 0; }
  TARGET=$(git -C "$DIR" rev-parse origin/main)
  [ "$TARGET" != "$LIVE" ] || return 0
  git -C "$DIR" merge-base --is-ancestor "$LIVE" "$TARGET" || { ntfy "$P: live $LIVE is not an ancestor of main; holding"; return 0; }
  [ "$(st failed)" != "$TARGET" ] || return 0        # already failed this sha: wait for a new commit
  short=$(echo "$TARGET" | cut -c1-8)

  if [ -z "${CD_RELEASE:-}" ]; then
    r=$(hold_reason)
    if [ -n "$r" ]; then
      [ "$(st held)" = "$TARGET" ] || { put held "$TARGET"; ntfy "held $P $short: $r. Release: cdp ship $P $TARGET"; }
      return 0
    fi
  fi
  # hours and cap wait quietly; the next tick tries again
  in_denied_hours && { say "$P: $short waits, clinic hours"; return 0; }
  cap=$(cfg cap); [ "$(shipped_today)" -lt "${cap:-3}" ] || { say "$P: $short waits, ${cap:-3} ships today"; return 0; }

  # ── the commit, alone, in a work tree ──
  # a repo whose build names siblings by relative path (Everyday Vet: ../../kita2)
  # sets `workdir` to a path at the same depth as its checkout
  W=$(cfg workdir | sed "s#^~#$HOME#"); W=${W:-$CD_STATE/work/$P}; export CD_SHA=$TARGET CD_DIR=$W
  [ -d "$W" ] || git -C "$DIR" worktree add -q --detach "$W" "$TARGET" || return 1
  git -C "$W" checkout -q --detach "$TARGET" || return 1
  if [ "$(st proven)" != "$TARGET" ]; then
    for step in gate stage smoke; do
      c=$(cfg $step); [ -n "$c" ] || continue
      say "$P $short: $step"
      ( cd "$W" && sh -c "$(sub "$c" "$TARGET")" ) > "$CD_STATE/$P.$step.log" 2>&1 || {
        put failed "$TARGET"; ntfy "$P $short: $step failed, nothing shipped (log: $CD_STATE/$P.$step.log)"; return 1; }
    done
    put proven "$TARGET"
  fi
  if [ -e "$CD_HOME/STOP" ]; then ntfy "$P $short proven on staging; not shipped: kill switch is on"; return 0; fi

  # ── prod, then watch ──
  say "$P $short: prod"
  if ! ( cd "$W" && sh -c "$(sub "$(cfg prod)" "$TARGET")" ) > "$CD_STATE/$P.prod.log" 2>&1; then
    roll "prod step failed"; return 1
  fi
  urls=$(cfg health); elapsed=0; bad=0
  while [ "$elapsed" -lt "$WATCH_FOR" ]; do
    for u in $urls; do
      curl -sS -f -m 10 "$u" 2>/dev/null | grep -q '"ok":true' || bad=$((bad+1))
    done
    [ "$bad" -lt 2 ] || { roll "health failed at $u"; return 1; }   # two bad answers in a row-ish
    sleep "$WATCH_EVERY"; elapsed=$((elapsed+WATCH_EVERY))
  done
  record_ship
}
roll() {  # the prod step or the watch failed
  put failed "$TARGET"
  rb=$(cfg rollback)
  if [ -n "$rb" ] && ( cd "$W" && sh -c "$(sub "$rb" "$LIVE")" ) > "$CD_STATE/$P.rollback.log" 2>&1; then
    ntfy "ROLLED BACK $P $short: $1. Back on $(echo "$LIVE" | cut -c1-8)."
  else
    touch "$CD_HOME/STOP"
    ntfy "ROLLBACK FAILED $P $short: $1. Kill switch is ON. Needs a person (log: $CD_STATE/$P.rollback.log)"
  fi
}
record_ship() {
  commits=$(git -C "$DIR" log --format='%h %s' "$LIVE..$TARGET")
  tags=""; untagged=""
  for c in $(git -C "$DIR" log --format=%H "$LIVE..$TARGET"); do
    t=$(git -C "$DIR" log -1 --format=%B "$c" | sed -n 's/^Ready: *//p' | head -1)
    if [ -n "$t" ]; then tags="$tags $t"; else untagged="$untagged $(echo "$c" | cut -c1-8)"; fi
  done
  if [ -n "$untagged" ]; then
    d=${CD_OFFLIST:-$HOME/Projects/head-of-product/offlist}; mkdir -p "$d"
    printf -- '- %s %s: shipped without a Ready tag:%s\n' "$(date +%F)" "$P" "$untagged" >> "$d/$(date +%F).md"
  fi
  # the org kita log: one entry per release, each commit an openable source
  if [ -x "${CD_ORG:-$HOME/Projects/dorky_robot/org-kita/target/debug/org}" ]; then
    set -- add "shipped $P $short (tags:${tags:- none})" --by a:cdp --known agent-said --as a:cdp --area deploys
    for c in $(git -C "$DIR" log --format=%H "$LIVE..$TARGET"); do set -- "$@" --source "commit $P@$c"; done
    "${CD_ORG:-$HOME/Projects/dorky_robot/org-kita/target/debug/org}" "$@" >/dev/null 2>&1 || say "org log write failed (not blocking)"
  fi
  date +"%F %T $TARGET" >> "$CD_STATE/$P.ships"; put shipped "$TARGET"
  ntfy "shipped $P $short ($(echo "$commits" | wc -l | tr -d ' ') commits) · rollback: $(cfg rollback | head -c 60)"
}

case "${1:-tick}" in
  init)    P=$2; put shipped "$3"; say "$P baseline $3" ;;
  ship)    CD_RELEASE=1 tick "$2" ;;
  now)     CD_FORCE_HOURS=1 tick "$2" ;;
  stop)    mkdir -p "$CD_HOME"; touch "$CD_HOME/STOP"; say "kill switch ON" ;;
  go)      rm -f "$CD_HOME/STOP"; say "kill switch off" ;;
  status)  for f in "$CD_STATE"/*.shipped; do P=$(basename "$f" .shipped); printf '%s live=%s held=%s failed=%s today=%s\n' "$P" "$(st shipped | cut -c1-8)" "$(st held | cut -c1-8)" "$(st failed | cut -c1-8)" "$(shipped_today)"; done
           [ ! -e "$CD_HOME/STOP" ] || echo "kill switch: ON" ;;
  tick)    if [ -n "${2:-}" ]; then tick "$2"; else awk '!/^#/ && NF {print $1}' "$CD_HOME/repos" | while read -r p; do tick "$p"; done; fi ;;
  *) sed -n '2,10p' "$0"; exit 64 ;;
esac
