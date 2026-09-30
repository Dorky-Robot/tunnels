# Revive a dead Claude lead or worker

**Use when:** a background Claude session (a lead like "Mesh", or a worker
like "Mesh · runbooks") has gone from `claude agents` or ListAgents, has lost
Remote Control, or is stuck on "/rc connecting…".
**Don't use when:** it is running and just slow. `claude logs <id>` shows
what it is doing.
**Scope:** this Mac's Claude background service. No tunnel, fleet or DNS
change.
**Needs:** a shell on the box the session ran on. Leads and workers run only
on dorkyrobot2 and the mini (`ssh mini`, then
`export PATH=$HOME/.local/bin:/opt/homebrew/bin:$PATH`).

## Why sessions die (Claude Code 2.1.285)

| cause | how it shows | fix |
|---|---|---|
| **idle retirement**: the background service retires a session idle 8 h with Remote Control, 1 h without; no setting changes it | gone from `claude agents`; `~/.claude/daemon.log` has `bg retire … idle` | resume it (step 3); pin it (step 4) |
| **auto-update** restarts the service from the new binary | sessions come back without Remote Control | resume with `--remote-control` (step 3) |
| **privacy (TCC) hang**: the new binary has no Full Disk Access, and macOS waits on a dialog nobody sees | new sessions sit on "/rc connecting…" forever | Full Disk Access (step 5) |
| **the box rebooted** | everything gone at once | [after-a-power-cut.md](after-a-power-cut.md) first, then step 3 per lead |

## 1. What is there

```sh
claude agents --json | python3 -c 'import json,sys
for a in json.load(sys.stdin): print(a["status"], a["sessionId"][:8], a.get("name"))'
```

and every background session this box has had, newest first, with its full
id and whether Remote Control was ever set up:

```sh
python3 - <<'EOF'
import json, glob, os
for f in sorted(glob.glob(os.path.expanduser("~/.claude/jobs/*/state.json")), key=os.path.getmtime, reverse=True):
    d = json.load(open(f))
    print(d.get("updatedAt", "")[:16], f"{d.get('state', ''):8}", d.get("sessionId", ""), d.get("name"), "rc" if d.get("bridgeSessionId") else "NO-RC")
EOF
```

Find the lead by name there; its `sessionId` is what `--resume` takes.

## 2. Why it went

```sh
grep -E 'bg retire|respawn' ~/.claude/daemon.log | tail -20
```

A line like `bg retire 1f807b99: idle-prompt, idle 8h` naming the first 8
characters of its id: retired for idleness. Nothing: it may have crashed;
`claude logs <id>` shows its last screen.

## 3. Bring it back, with its conversation

If it is dead (listed with no pid) and was started with `--remote-control`,
wake it in place instead: `cd <its cwd> && claude --bg --resume <sessionId>
"<note>"` with **no other flags** keeps its id, its saved options and its
claude.ai link, and delivers the note. Flags are what make the copy below.
`scripts/lead-keeper.py` does all of this by itself (docs/lead-keeper.md).

If it still shows in `claude agents` but has no Remote Control, stop it
first and wait until `claude agents` no longer lists it:

```sh
claude stop <id>
```

Then:

```sh
cd <its cwd> && claude --bg --resume <sessionId> --remote-control -n "<its exact name>" "<one line: why you revived it>"
```

**Check:** it prints an id; within ~30 s the job lister above shows it
`working` with `rc`, and ListAgents lists it by name, so SendMessage
reaches it. Use the exact old name: SendMessage addresses by name, and a
new name breaks every other session's way of reaching it. If the old copy
was still running, the CLI starts a copy with a new id and says so; stop
the old one.

## 4. Keep a lead alive: pin it

Pinned sessions are exempt from idle retirement. In `claude agents`, select
the session and press **Ctrl+T**. **Check:**

```sh
cat ~/.claude/jobs/pins.json      # its id is listed
```

Workers are meant to finish and be removed (`claude stop <id>`, then
`claude rm <id>`); pin leads, not workers.

## 5. Stuck on "/rc connecting…": the privacy hang

Before changing anything, prove it:

```sh
claude --bg --remote-control --debug-file /tmp/rc-probe.log -n "probe" "say hi"
sleep 20; tail -3 /tmp/rc-probe.log         # log stops half a second in: a hang
pid=$(claude agents --json | python3 -c 'import json,sys; print(next(a["pid"] for a in json.load(sys.stdin) if a.get("name")=="probe"))')
sample $pid 1 2>/dev/null | grep -m3 openat
```

A main thread stuck in `openat` is macOS waiting for a Full Disk Access
answer. Only a person at the screen can grant it (System Settings → Privacy
& Security → Full Disk Access):

- dorkyrobot2: `~/.local/share/claude/ClaudeCode.app`
- the mini: no app bundle; it runs `~/.local/share/claude/versions/<version>`
  directly, so each update needs the grant again.

Then remove the probe (`claude stop`, `claude rm`) and resume the leads
(step 3). **Don't** switch Claude versions to test a theory: `claude
install` restarts the background service and drops Remote Control from every
running lead.

## 6. New models greyed out, or sessions on an old binary

After an update, `claude respawn --all` restarts background sessions on the
current binary. On the mini, sessions started from Felix's app go through
the `com.felixflores.claude-rc` LaunchAgent, which keeps the binary it
started with:

```sh
launchctl kickstart -k gui/$(id -u)/com.felixflores.claude-rc
```

It **kills every session running under it**, so say so first. It is safe
over `ssh mini`: it doesn't carry the ssh.

## 7. Clean up every test session, in both places

A session you start to test something (name it `lk-throwaway-<n>`, which
lead-keeper never revives) lives in two places. Clean up both before you
report.

1. **Locally**: `claude stop <id>`, then `claude rm <id>`. **Check:**
   `claude agents --json --all` no longer lists it.
2. **In the claude.ai sidebar**: any session that ran with
   `--remote-control`, or was resumed with it, has a claude.ai record, and
   `claude rm` leaves that record behind. No CLI command archives it
   (checked in 2.1.285/2.1.286 help and in the Remote Control and agent-view
   docs). Find its link with `claude logs <id>` before you remove it, or
   read the `bridgeSessionId` (`cse_X`, link `claude.ai/code/session_X`)
   from its transcript, then ask Felix to archive it: open the link, and in
   the claude.ai/code session list open that session's menu and choose
   Archive.

Keep sidebar records rare. Test without `--remote-control` when RC is not
what you are testing. When it is, resume one test conversation
(`claude --bg --resume <its sessionId> --remote-control -n <name>`)
instead of starting new ones: a resumed session reattaches to its
existing claude.ai record. claude-update's probe works this way.

## Stop and ask a person if

- the probe shows the `openat` hang (only Felix, at the screen, can grant
  Full Disk Access);
- the session isn't in `~/.claude/jobs` at all: it wasn't a background
  session on this box. Try the other box;
- resuming fails twice the same way: tell CTO rather than start a fresh
  session under the old name, which loses the lead's context.

## Undo

`claude stop <id>` stops a revived copy; its conversation is kept.

## History

- 2026-09-29: an auto-update restarted the background service from a binary
  without Full Disk Access; every new session on dorkyrobot2 and the mini sat
  on "connecting…" for an afternoon, and respawned leads lost Remote
  Control. FDA for `ClaudeCode.app` fixed dorkyrobot2 in seconds.
- 2026-09-30: leads retired overnight after 8 idle hours. Pinning is the
  exemption. Written the same day; the listers, `claude … --help` and the
  claude-rc job were checked on dorkyrobot2 and the mini. Resume and pin
  were not run.
- 2026-09-30: run on throwaway sessions for lead-keeper. With no flags,
  `--resume <sessionId> "<note>"` woke a stopped session in place (same id
  and RC link, note delivered); with flags it made a copy and left a dead
  duplicate. `claude rm` kept the transcript, and the flagged resume
  brought the conversation back.
- 2026-09-30: lead-keeper's throwaways left six records in Felix's claude.ai
  sidebar after `claude rm`. Step 7 added: clean up in both places. A
  resumed test conversation reattached to its own record (session_01XYsUNP…),
  which is how the claude-update probe now stays at one record.
