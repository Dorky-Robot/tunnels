#!/bin/bash
# Keep a machine's Colima VM up, the drives it mounts mounted, and the
# containers that must run, running. Every 120 s, as a LaunchAgent.
#
# WHY (2026-09-23): Doug's mini rebooted at 01:46 and every hostname on its
# tunnel was 502 for fourteen and a half hours, while the tunnel itself was
# healthy and ssh through it worked — nothing looked wrong from outside. The
# chain was one unmounted volume, two hops from the symptom: the T7 SSD was
# attached but its volume did not mount after the reboot; Colima's config
# mounts /Volumes/T7 for Nextcloud, so `colima start` died at
# `mkdir /Volumes/T7: permission denied`; with no VM there was no Pocket ID;
# and the site refuses to start when it cannot reach its identity provider.
#
# What was there could not have caught it. The Colima agent was RunAtLoad
# only: one attempt per login, never a retry. The tunnel watchdog was alive
# and did kickstart the site, but it only knows launchd jobs — not a VM, and
# not a mount. This fills that gap: it retries, and it mounts first.
#
# Mounting only ever mounts a volume that is attached and not mounted. If the
# drive is gone, the VM stays down and this says so every run, which is right:
# starting Nextcloud over an empty /Volumes/T7 would look healthy and serve
# nothing (nextcloud/replicate.sh guards against the same thing).
#
# Configured per machine, in the plist's environment:
#   COLIMA_VOLUMES     volume names the VM mounts, e.g. "T7"
#   COLIMA_CONTAINERS  containers to start if stopped, e.g. "pocket-id nextcloud"
#   COLIMA_PROFILE     colima profile (default: default)
#   COLIMA_REQUIRE     paths that must be readable before the VM starts, e.g.
#                      "/Volumes/T7/nextcloud/data/.ncdata"
#   COLIMA_HOP_KEY     an ssh key; when set, the mount and `colima start` run
#                      through `ssh localhost` instead of in this job
#
# WHY THE HOP (2026-09-29): a launchd job may not read an external volume
# (TCC), and neither may anything it starts. doug-mini's nightly replica
# failed on exactly that for twelve days. A VM started by this job mounts
# /Volumes/T7 with this job's rights, so after a reboot Nextcloud could come
# up over a drive it cannot read. Anything started over ssh inherits sshd's
# Full Disk Access, which is how the VM that has been working was started.
# Granting FDA to limactl instead breaks on every brew upgrade (the Homebrew
# binaries are ad-hoc signed, so each version is a new identity to TCC).
#
# The key's line in ~/.ssh/authorized_keys forces the one thing it may do,
# and has to carry the settings itself, since the job's environment does not
# cross ssh:
#
#   command="COLIMA_VOLUMES=T7 COLIMA_REQUIRE=/Volumes/T7/nextcloud/data/.ncdata /Users/<you>/.local/bin/colima-supervisor.sh --up",restrict,from="127.0.0.1,::1" ssh-ed25519 AAAA… colima-hop
#
# The plist sets COLIMA_HOP_KEY and the same COLIMA_VOLUMES; this job then
# only looks (mount, colima status: neither reads the drive), hops when
# something needs doing, and starts containers itself (docker talks to the VM
# over a socket and never touches the drive).
#
# The everyday-vet copy (consulting/everyday-vet-admin/colima-supervisor.sh)
# is this without the volumes, from the day before; this is the general one.

set -uo pipefail
export PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin

PROFILE=${COLIMA_PROFILE:-default}
VOLUMES=${COLIMA_VOLUMES:-}
CONTAINERS=${COLIMA_CONTAINERS:-}
REQUIRE=${COLIMA_REQUIRE:-}
HOP_KEY=${COLIMA_HOP_KEY:-}
MODE=${1:-}

# Pin the Docker endpoint. Bare `docker` resolves through currentContext in
# ~/.docker/config.json, which colima rewrites on every start/stop for any
# profile — a supervisor that healed the wrong daemon would report success
# while the real one stayed down.
export DOCKER_HOST="unix://$HOME/.colima/$PROFILE/docker.sock"

STATE_DIR="$HOME/Library/Application Support/colima-supervisor"
LOG="$HOME/Library/Logs/colima-supervisor.log"
mkdir -p "$STATE_DIR" "$(dirname "$LOG")"
if [ -f "$LOG" ] && [ "$(stat -f%z "$LOG" 2>/dev/null || echo 0)" -gt 1048576 ]; then
  tail -n 500 "$LOG" > "$LOG.tmp" && mv "$LOG.tmp" "$LOG"
fi
exec >>"$LOG" 2>&1
log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*"; }

# One at a time: a colima start can take minutes, the timer is two. The --up
# side of the hop runs while the job that called it holds the lock.
LOCK="$STATE_DIR/lock.d"
if [ "$MODE" != --up ] && ! mkdir "$LOCK" 2>/dev/null; then
  if [ -n "$(find "$LOCK" -maxdepth 0 -mmin +30 2>/dev/null)" ]; then
    log "reclaiming stale lock"
    rmdir "$LOCK" 2>/dev/null && mkdir "$LOCK" 2>/dev/null || exit 0
  else
    exit 0
  fi
fi
[ "$MODE" = --up ] || trap 'rmdir "$LOCK" 2>/dev/null' EXIT

mounted() { mount | grep -q " on /Volumes/$1 "; }

# Steps 1 and 2, the ones that touch the drive. Run here, or on the far side
# of the hop.
up() {
  # 1. The drives the VM mounts. `mount` is the truth, not the directory: an
  #    unplugged drive can leave /Volumes/<name> behind as an empty folder.
  for v in $VOLUMES; do
    if ! mounted "$v"; then
      if diskutil mount "$v" >/dev/null 2>&1; then
        log "$v mounted"
      else
        log "WARN: $v is not mounted and could not be (unplugged?) — the VM will not start without it"
      fi
    fi
  done

  # 2. The VM. Not over a drive this process cannot read: a VM started
  #    without the right would serve an empty or unreadable data dir.
  if ! colima status -p "$PROFILE" >/dev/null 2>&1; then
    for r in $REQUIRE; do
      if [ ! -r "$r" ]; then
        log "ERROR: $r is not readable from here (unmounted, or TCC) — not starting VM $PROFILE"
        return 1
      fi
    done
    log "VM $PROFILE not running — starting"
    if ! colima start -p "$PROFILE" >/dev/null 2>&1; then
      log "ERROR: colima start failed; see ~/.colima/_lima/colima/ha.stderr.log"
      return 1
    fi
    log "VM $PROFILE up"
  fi
}

if [ "$MODE" = --up ]; then
  up
  exit $?
fi

need_up=
for v in $VOLUMES; do mounted "$v" || need_up=1; done
colima status -p "$PROFILE" >/dev/null 2>&1 || need_up=1
if [ -n "$need_up" ]; then
  if [ -n "$HOP_KEY" ]; then
    # The far side logs to the same file. A colima start takes a minute or two.
    /usr/bin/ssh -i "$HOP_KEY" -o BatchMode=yes -o IdentitiesOnly=yes \
      -o ConnectTimeout=20 -o ServerAliveInterval=30 localhost </dev/null \
      || log "the hop returned $? (an ssh failure, or the far side refused; see above)"
  else
    up
  fi
  colima status -p "$PROFILE" >/dev/null 2>&1 || exit 0
fi

# 3. Containers stopped by hand or left Exited by an odd shutdown. Restart
#    policies cover crashes; nothing inside the VM covers this.
for c in $CONTAINERS; do
  docker inspect "$c" >/dev/null 2>&1 || continue
  if [ "$(docker inspect -f '{{.State.Running}}' "$c" 2>/dev/null)" != "true" ]; then
    if docker start "$c" >/dev/null 2>&1; then log "$c started"; else log "WARN: $c start failed"; fi
  fi
done
