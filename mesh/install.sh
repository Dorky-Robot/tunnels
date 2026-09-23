#!/bin/sh
# Put the mesh's shared setup on this machine, identical to every other one.
#
#   sh mesh/install.sh           install, or bring this machine up to date
#   sh mesh/install.sh --tidy    also remove old Host blocks in ~/.ssh/config
#                                that mesh.conf now answers for (listed first)
#
# Idempotent: run it again and nothing changes. Anything of yours it edits is
# copied to ~/.ssh/backups/ first.
#
# What it installs — the same bytes on every machine:
#   ~/.ssh/config.d/mesh.conf           every box, three ways in (see its header)
#   ~/.ssh/config.d/mesh_known_hosts    host keys taken from each machine itself
#   ~/.ssh/config.d/github.conf         every GitHub account, by alias
#   ~/.ssh/config.d/github_known_hosts  GitHub's published host keys
#   ~/.ssh/config.d/machines, accounts  the inventory, for desktop and github-key-setup
#   ~/.ssh/authorized_keys              the mesh block: every machine's mesh key
#   ~/.local/bin/github-key-setup       this machine's own GitHub key, and git set up for it
#   ~/.local/bin/desktop + symlinks     desktop-<machine>, a screen in one word
#   the tunnels agent                   `tunnels agent install`: keeps this machine's
#                                       tunnels running and in line with the fleet file,
#                                       and brings back one that was booted out. With a
#                                       tunnels too old to have an agent, the old
#                                       tunnel-watchdog.sh does that last part instead.
#
# What it does NOT do, because it cannot be done from one machine:
#   - make this machine's mesh key or GitHub key (docs/remote-access.md,
#     "Adding a machine"; `github-key-setup`)
#   - install Tailscale (needs a password and a browser)
set -eu
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/.." && pwd)
H=$(hostname -s)
S=$HOME/.ssh D=$HOME/.ssh/config.d BK=$HOME/.ssh/backups
stamp=$(date +%Y%m%d-%H%M%S)
TIDY=; [ "${1:-}" = --tidy ] && TIDY=1
die() { echo "install: $*" >&2; exit 1; }
backup() { mkdir -p "$BK"; chmod 700 "$BK"; cp "$1" "$BK/$(basename "$1").$stamp"; }

sh "$here/build" --check >/dev/null || die "mesh/ is out of date with its inventory; run: sh mesh/build, and commit"
grep -v '^[[:space:]]*#' "$here/machines" | awk -v h="$H" '$2 == h {f=1} END {exit !f}' \
  || echo "install: $H is not in mesh/machines — installing anyway, but no other box knows this one" >&2

# ---- 1. the shared files ----------------------------------------------------
mkdir -p "$D"; chmod 700 "$S" "$D"
for f in mesh.conf mesh_known_hosts github.conf github_known_hosts machines accounts; do
  cp "$here/$f" "$D/$f"; chmod 644 "$D/$f"
done

# ---- 2. ~/.ssh/config: one Include for all of config.d --------------------
# It goes after any Includes already at the top: an Include written below a
# Host line is scoped to that Host, and the mesh would silently only apply to it.
c=$S/config; [ -f "$c" ] || : > "$c"
inc='Include ~/.ssh/config.d/*.conf'
if ! grep -qxF "$inc" "$c"; then
  backup "$c"
  awk -v inc="$inc" '
    $0 == "Include ~/.ssh/config.d/mesh.conf" { if (!done) print inc; done=1; next }
    !done && !/^Include/ { print inc; done=1 }
    { print }
    END { if (!done) print inc }' "$c" > "$c.new"
  mv "$c.new" "$c"
  echo "· ~/.ssh/config: $inc"
fi
chmod 600 "$c"

# ---- 3. Host blocks in ~/.ssh/config that config.d now answers for --------
# ssh takes the first value of each option, and config.d is included first,
# so these are shadowed — but IdentityFile and LocalForward add up across
# every matching block, which is how a stray `Host *` key got offered to
# GitHub ahead of the right one. GitHub blocks are always removed (github.conf
# names the same keys); mesh blocks only with --tidy, since some still carry a
# LocalForward somebody may use.
covered=$(awk '/^Host[[:space:]]/ { for (i = 2; i <= NF; i++) print $i }' "$D"/*.conf | sort -u)
gh_covered=$(awk '/^Host[[:space:]]/ { for (i = 2; i <= NF; i++) print $i }' "$D/github.conf" | sort -u)
# a block: its Host line and the indented or blank lines under it; a line at
# column 0 (a comment, another Host, a Match) ends it
blocks() {  # blocks <covered names> → "startline endline Host …" per block wholly covered
  printf '%s\n' "$1" > "$S/.covered.$$"
  awk -v cf="$S/.covered.$$" '
    BEGIN { while ((getline n < cf) > 0) cov[n] = 1 }
    function flush() { if (start && all) print start, last, hl; start = 0 }
    /^[Hh]ost[[:space:]]/ { flush(); start = NR; last = NR; hl = $0; all = 1
                            for (i = 2; i <= NF; i++) if (!($i in cov)) all = 0; next }
    start && (/^[[:space:]]/ || !NF) { if (NF) last = NR; next }
    { flush() }
    END { flush() }' "$c"
  rm -f "$S/.covered.$$"
}
drop() {  # drop <"start end …" lines>
  printf '%s\n' "$1" | awk 'NF {print $1, $2}' > "$S/.drop.$$"
  awk -v df="$S/.drop.$$" 'BEGIN { while ((getline l < df) > 0) { split(l, r, " "); for (i = r[1]; i <= r[2]; i++) gone[i] = 1 } }
    !(NR in gone)' "$c" | cat -s > "$c.new"
  rm -f "$S/.drop.$$"; mv "$c.new" "$c"; chmod 600 "$c"
}
gh=$(blocks "$gh_covered")
if [ -n "$gh" ]; then
  backup "$c"; drop "$gh"
  printf '%s\n' "$gh" | cut -d' ' -f3- | sed 's/^/· removed from ~\/.ssh\/config (github.conf has it): /'
fi
old=$(blocks "$covered")
if [ -n "$old" ]; then
  if [ -n "$TIDY" ]; then
    backup "$c"
    printf '%s\n' "$old" | while read -r a b hl; do
      echo "· removed from ~/.ssh/config (mesh.conf has it): $hl"
      sed -n "${a},${b}p" "$c" | grep -E '^[[:space:]]+(LocalForward|RemoteForward|DynamicForward)' | sed 's/^[[:space:]]*/    and with it: /' || true
    done
    drop "$old"
  else
    echo "· ~/.ssh/config still has blocks mesh.conf answers for (shadowed; --tidy removes them):"
    printf '%s\n' "$old" | cut -d' ' -f3- | sed 's/^/    /'
  fi
fi

# ---- 4. authorized_keys: the mesh block --------------------------------------
# Everything between the markers is mesh/authorized_keys, and any older line
# holding one of those keys or signed "mesh:" is folded into it — so a machine
# taken out of mesh/keys stops getting in at the next install everywhere.
ak=$S/authorized_keys; [ -f "$ak" ] || : > "$ak"
begin='# >>> mesh — managed by tunnels/mesh/install.sh; edit mesh/keys, not this >>>'
end='# <<< mesh <<<'
[ "$(grep -c '^ssh-' "$here/authorized_keys")" -ge 2 ] || die "mesh/authorized_keys has fewer than two keys; refusing to write it"
awk '/^ssh-/ {print $2}' "$here/authorized_keys" > "$S/.meshblobs.$$"
{
  awk -v b="$begin" -v e="$end" -v bf="$S/.meshblobs.$$" '
    BEGIN { while ((getline k < bf) > 0) mine[k] = 1 }
    $0 == b { skip = 1; next }  $0 == e { skip = 0; next }  skip { next }
    { for (i = 1; i <= NF; i++) if ($i in mine) next }
    / mesh:[^ ]*$/ { print "· no longer let in (not in mesh/keys): " $NF > "/dev/stderr"; next }
    { print }' "$ak"
  echo "$begin"; grep '^ssh-' "$here/authorized_keys"; echo "$end"
} > "$ak.new"
rm -f "$S/.meshblobs.$$"
if cmp -s "$ak.new" "$ak"; then rm -f "$ak.new"
else backup "$ak"; mv "$ak.new" "$ak"; echo "· ~/.ssh/authorized_keys: mesh block, $(grep -c '^ssh-' "$here/authorized_keys") keys"; fi
chmod 600 "$ak"

# ---- 5. scripts, and git set up for this machine's GitHub account ---------
mkdir -p "$HOME/.local/bin" "$HOME/Library/Logs"
cp "$here/github-key-setup" "$HOME/.local/bin/github-key-setup"
chmod +x "$HOME/.local/bin/github-key-setup"
"$HOME/.local/bin/github-key-setup" --git || echo "install: github-key-setup --git failed; git is as it was" >&2

# desktop-<machine>. --install makes its own symlinks beside itself, from the
# machines file this installed above.
cp "$here/desktop" "$HOME/.local/bin/desktop"
chmod +x "$HOME/.local/bin/desktop"
"$HOME/.local/bin/desktop" --install >/dev/null
mkdir -p "$HOME/Library/LaunchAgents"
TUNNELS=$(command -v tunnels || ls /opt/homebrew/bin/tunnels /usr/local/bin/tunnels 2>/dev/null | head -1 || true)
if [ -n "$TUNNELS" ] && "$TUNNELS" agent --help >/dev/null 2>&1; then
  # the agent does what the watchdog did, and more; it removes the watchdog itself
  "$TUNNELS" agent install >/dev/null
  keeper="agent $("$TUNNELS" --version | cut -d' ' -f2)"
else
  cp "$repo/scripts/tunnel-watchdog.sh" "$HOME/.local/bin/tunnel-watchdog.sh"
  chmod +x "$HOME/.local/bin/tunnel-watchdog.sh"
  P="$HOME/Library/LaunchAgents/com.dorkyrobot.tunnel-watchdog.plist"
  if ! cmp -s "$here/com.dorkyrobot.tunnel-watchdog.plist" "$P" || ! launchctl print "gui/$(id -u)/com.dorkyrobot.tunnel-watchdog" >/dev/null 2>&1; then
    launchctl bootout "gui/$(id -u)/com.dorkyrobot.tunnel-watchdog" 2>/dev/null || true
    cp "$here/com.dorkyrobot.tunnel-watchdog.plist" "$P"
    launchctl bootstrap "gui/$(id -u)" "$P"
  fi
  keeper="watchdog $(shasum -a 256 "$HOME/.local/bin/tunnel-watchdog.sh" | cut -c1-12)"
fi

h() { shasum -a 256 "$1" | cut -c1-12; }
printf '%s  mesh.conf %s  known_hosts %s  github.conf %s  %s\n' "$H" \
  "$(h "$D/mesh.conf")" "$(h "$D/mesh_known_hosts")" "$(h "$D/github.conf")" "$keeper"
