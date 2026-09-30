# lead-keeper and claude-update: Claude sessions that come back

The leads on dorkyrobot2 (CTO, Mesh, Everyday Vet, Agent tooling,
DorkyRobot, Monica) and their workers have to be there when Felix or CTO
talks to them. Claude Code's background supervisor retires `--bg` sessions
that sit idle, after 8h with Remote Control and 1h without
(`[bg] bg retire <id>: idle-prompt, idle 8h` in `~/.claude/daemon.log`).
Pinning a session in `claude agents` (Ctrl+T) exempts it from that, but a
pinned session can still be retired under low memory, can crash, and on
2026-09-29 respawned sessions hung on a macOS privacy prompt after an
auto-update. Two jobs cover this:

| job | what | when |
|---|---|---|
| `scripts/lead-keeper.py` | brings back dead leads and workers, with their conversation and Remote Control | every 5 min, `mesh/com.dorkyrobot.lead-keeper.plist` |
| `scripts/claude-update.py` | updates Claude Code, proves the new binary, respawns onto it | Sunday 04:31, `mesh/com.dorkyrobot.claude-update.plist` |

Both are user LaunchAgents, because the supervisor, the install and the
sessions all belong to dorkyrobot2's user and its login session. Both only
speak through mesh-watch's channel (below). mesh-watch watches both: once a
plist is in `~/Library/LaunchAgents`, a `last-run` older than 11 minutes
(lead-keeper) or 8 days (claude-update) is an incident like a dead site.

## What the CLI does (Claude Code 2.1.285, tried on throwaway sessions)

- A session is dead when its `claude agents --json --all` entry has no pid,
  or has a pid whose process is gone.
- `claude --bg --resume <sessionId> "<note>"`, with **no other flags**,
  wakes a dead background session in place: the same id, the same
  conversation, its saved `--remote-control` and `-n`, the same claude.ai
  link, and the note as its next prompt ("woke session … with its saved
  options").
- The same command **with** flags starts a copy with a new id, and leaves
  the old entry behind as a dead duplicate ("keeps its own saved options,
  so the flags you passed started a copy").
- `claude respawn <id>` also wakes a dead entry in place, with no prompt,
  so no model turn.
- `claude rm <id>` keeps the transcript. After it, `respawn` says "No job
  matching", and the flagged `--resume` brings the conversation back.
- `claude logs <id>` on a dead session says "job not found", so an RC link
  in the logs always comes from the live process. In a long session the
  link scrolls out of the logs, so it is only a test for a fresh start.
- A promptless `claude --bg --remote-control -n <name>` shows its RC link
  in seconds and spends no tokens, which makes it a free probe of a binary.
- There is no CLI way to send a prompt to a live session.

## lead-keeper

**Who.** Every session named `<Lead>` or `<Lead> · <task>` for a lead in
`mesh/leads.conf`, as long as `claude agents --all` lists it. `claude rm`
is how a session is finished on purpose, so a removed session is never
brought back. Per name only the newest entry counts: if any entry by that
name is alive, the name is alive, and an older duplicate is never revived.
`exclude | <name>` in the config leaves a session alone.

**When.** A name is revived when two passes in a row find either:

- no entry with a live process, or
- a live, idle session whose saved flags have lost `--remote-control`, as
  the 09-29 respawn did. A busy session, or one waiting on a permission
  prompt, is left until it is idle.

It is never revived while the process it was last seen with is still alive.
It also does nothing while claude-update holds its lock.

**How.** From the session's own cwd, or its lead's if that cwd is gone:

| found | run | note |
|---|---|---|
| retired for idling (the daemon log says so) | `claude respawn <id>` | none: nothing was in flight, so no model turn |
| died | `claude --bg --resume <sessionId> "<note>"` | tells a worker to SendMessage its lead, and a lead to tell CTO, that it is back |
| lost RC, or saved without it | `claude stop <id>`, then `claude --bg --resume <sessionId> --remote-control -n <name> "<note>"` | a copy with a new id and the same conversation |
| an interactive lead (CTO) gone from the list | the same flagged `--resume`, from its last session id | |

Leads come first. While the kernel reports memory pressure
(`kern.memorystatus_vm_pressure_level` of 2 or higher), only leads are
revived and workers wait.

**Did it work.** A revival counts once `claude logs <id>` shows a
claude.ai/code link within 60 s. If the process starts and no link ever
appears, that is the privacy-prompt hang. The attempt is stopped (a stuck
copy would look alive forever), Felix is told once which path needs Full
Disk Access, and nothing more is revived until the Claude binary changes or
someone runs `scripts/lead-keeper.py --clear`. If the command itself fails,
that one session is retried every 6h.

**Cost.** While everything is up, one pass is one `claude agents` call:
0.11 s, and no model turns. A revival costs one turn (the note), except an
idle retire, which costs none.

### What Felix hears

A revival is one line in `~/Library/Logs/lead-keeper.log` and nothing else.
Felix gets an ntfy alert, plus one kapwa item on `#mesh` signed
`lead-keeper`, only when:

- a revival hung without Remote Control (once, with the path to grant),
- a lead cannot be revived (untrusted cwd, missing cwd), a lead was
  removed, or a lead's process is alive but unlisted for three passes,
- a lead died twice in 24 hours, not counting idle retires (at most once a
  day), or
- `claude agents` has been unreadable for 30 minutes.

Each of these is said once. When a lead that was reported down comes back,
that is one low-priority ntfy, and the kapwa item is closed with `done`, not
answered with a new one. An alert that cannot be sent waits for the next
pass, for up to a day.

Alerts use mesh-watch's channel: `MESH_WATCH_NTFY` in
`~/.config/mesh-watch/env`. If that is not set, the ntfy half waits.

### State

`~/.local/state/lead-keeper/`: `state.json` holds, per name, the last id,
session, pid and kind, the pass counts, failures, what has been said, and
any hold. `last-run` is the heartbeat. A name that is no longer listed is
dropped from the state.

## claude-update

Auto-update is off (`DISABLE_AUTOUPDATER=1`, Felix's decision, set by hand
in settings). Instead, once a week:

1. `claude update`. If the version is unchanged, that is one log line and
   the run is over. If it fails two weeks in a row, Felix hears once.
2. Probe the new binary before anything depends on it. A promptless
   `claude --bg --remote-control -n "claude-update probe"` must show its RC
   link within 60 s, and `lsof` must show it running the new file. The
   probe is always stopped and removed afterwards. A script can neither
   grant nor read Full Disk Access, so this is the check. If the probe
   hangs, or runs the old binary and so proves nothing, Felix gets one
   alert naming the exact path to grant, and **nothing is respawned**.
   - On dorkyrobot2 the path is `~/.local/share/claude/ClaudeCode.app`.
     The installer keeps that bundle's executable hard-linked to the
     current version, and the grant belongs to the bundle, so it should
     carry over. The probe is what confirms it.
   - Where there is no such link (the mini), the path is
     `~/.local/share/claude/versions/<new>`, and each version needs its
     own grant.
3. `claude respawn <id>`, one session at a time, idle ones first. A busy
   session gets up to 30 minutes to finish its turn, and is otherwise left
   on the old version until next week.
4. Each one must be running again, on the new version, and back on RC if it
   had RC before. A lead or worker that comes back without RC is left to
   lead-keeper, which repairs it within ten minutes. Anything else is one
   alert.

lead-keeper pauses while claude-update holds
`~/.local/state/claude-update/running`. CTO is an interactive session run
by the Remote Control daemon, not by the supervisor, so `respawn` does not
move it to the new version; it keeps its version until it restarts.

## Install (not done)

On dorkyrobot2, as dorkyrobot2, once Felix says so:

    cp mesh/com.dorkyrobot.lead-keeper.plist mesh/com.dorkyrobot.claude-update.plist ~/Library/LaunchAgents/
    launchctl bootstrap gui/$UID ~/Library/LaunchAgents/com.dorkyrobot.lead-keeper.plist
    launchctl bootstrap gui/$UID ~/Library/LaunchAgents/com.dorkyrobot.claude-update.plist

Before that:

- `scripts/lead-keeper.py --dry-run` and `scripts/claude-update.py
  --dry-run` should both read sensibly.
- `~/.config/mesh-watch/env` should hold `MESH_WATCH_NTFY`.
- Auto-update should be off.

Both jobs run the scripts from the main checkout, so landing on main
updates them. To run one now:
`launchctl kickstart gui/$UID/com.dorkyrobot.lead-keeper`. This does not
restart anything that carries ssh.

## Test

    /usr/bin/python3 tests/lead_keeper_test.py
    /usr/bin/python3 tests/claude_update_test.py

Both run against a fake `claude`, a fake `kapwa` and a local ntfy, and
start, stop and resume nothing real.
