#!/usr/bin/python3
"""Update Claude Code on this Mac once a week, on purpose, and make sure the
background sessions come back from it. docs/lead-keeper.md.

    claude-update.py              update; if the version changed, probe and respawn
    claude-update.py --dry-run    say what it would do; update nothing, restart nothing

Auto-update is off (DISABLE_AUTOUPDATER=1, Felix's call), because on
2026-09-29 an auto-update restarted the background service from a binary
macOS had never granted Full Disk Access. Every new session hung on a
privacy prompt nobody was there to answer, and the sessions it respawned
lost Remote Control. So the update happens here, at a quiet hour, in this
order:

  1. `claude update`. Same version as before: one log line, done.
  2. Probe the new binary before any real session depends on it: a
     promptless `claude --bg --remote-control` session (no model turn) must
     show its claude.ai/code link within PROBE_WAIT seconds, and `lsof` must
     show it running the new version. A script cannot grant or read Full
     Disk Access, so this is the check. If the probe hangs or runs the old
     binary, the grant cannot be verified: Felix gets one alert naming the
     exact path to grant, and nothing is respawned.
  3. `claude respawn <id>` for every background session that is not busy,
     one at a time; a busy one gets up to BUSY_WAIT to finish its turn.
  4. Each respawned session must be alive again, on the new version, and
     show its RC link if it had one before. Any that do not are one alert.

lead-keeper.py pauses while this holds its lock, so a lead in the middle
of a respawn is not mistaken for a dead one.
"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
from claude_bg import (CLAUDE, HOME, agents, box, flush, load_env, load_json, rc_link,  # noqa: E402
                       run, save_json, stamp, BG_ID, plain)

STATE = os.environ.get("CLAUDE_UPDATE_STATE", os.path.join(HOME, ".local/state/claude-update"))
LOCK = os.environ.get("CLAUDE_UPDATE_LOCK", os.path.join(STATE, "running"))
JOBS = os.environ.get("CLAUDE_JOBS", os.path.join(HOME, ".claude/jobs"))
BUNDLE = os.environ.get("CLAUDE_BUNDLE", os.path.join(HOME, ".local/share/claude/ClaudeCode.app"))
PROBE_CWD = os.path.expanduser(os.environ.get("CLAUDE_UPDATE_PROBE_CWD", "~/Projects"))
PROBE_NAME = "claude-update probe"
PROBE_WAIT = float(os.environ.get("CLAUDE_UPDATE_PROBE_WAIT", "60"))
RC_WAIT = float(os.environ.get("CLAUDE_UPDATE_RC_WAIT", "90"))
BUSY_WAIT = float(os.environ.get("CLAUDE_UPDATE_BUSY_WAIT", "1800"))
LSOF = os.environ.get("CLAUDE_UPDATE_LSOF", "/usr/sbin/lsof")
TAG = "claude-update"


def log(msg):
    print("%s %s" % (stamp(), msg), flush=True)


def binary():
    """(version, the real file ~/.local/bin/claude points at)."""
    real = os.path.realpath(CLAUDE)
    rc, out, _ = run([CLAUDE, "--version"], 60)
    return (out.split()[0] if rc == 0 and out.split() else "?"), real


def grant_path(real):
    """What Full Disk Access has to be granted to for this binary. On
    dorkyrobot2 the installer keeps an app bundle whose executable is a
    hard link to the current version, and the grant belongs to the bundle
    (bundle id plus signature), so it carries across versions. Without that
    link (the mini) the process runs the version file itself, a path macOS
    has never seen."""
    exe = os.path.join(BUNDLE, "Contents/MacOS/claude")
    try:
        if os.stat(exe).st_ino == os.stat(real).st_ino:
            return BUNDLE, True
    except OSError:
        pass
    return real, False


def job(id_):
    return load_json(os.path.join(JOBS, id_, "state.json"), {})


def image(pid):
    """The executable a running process was started from."""
    rc, out, _ = run([LSOF, "-a", "-p", str(pid), "-d", "txt", "-Fn"], 20)
    for line in out.splitlines():
        if line.startswith("n") and "claude" in line:
            return line[1:]
    return None


def same_file(a, b):
    try:
        return os.stat(a).st_ino == os.stat(b).st_ino
    except (OSError, TypeError):
        return False


def probe(real, dry):
    """Start a throwaway session on the new binary and see it reach Remote
    Control. (ok, why). Always cleans up after itself."""
    if dry:
        print("would probe: from %s, %s --bg --remote-control -n %r (no prompt, no model turn);\n"
              "  it must show a claude.ai/code link within %ds and run %s" % (PROBE_CWD, CLAUDE, PROBE_NAME, PROBE_WAIT, real))
        return True, ""
    rc, out, err = run([CLAUDE, "--bg", "--remote-control", "-n", PROBE_NAME], 120, cwd=PROBE_CWD)
    m = BG_ID.search(plain(out + err))
    if rc != 0 or not m:
        return False, "the probe did not start: %s" % plain(out + err).strip()[-200:]
    pid_ = m.group(1)
    try:
        link = rc_link(pid_, PROBE_WAIT)
        e = [a for a in (agents() or []) if a.get("id") == pid_]
        img = image(e[0]["pid"]) if e and e[0].get("pid") else None
        if not link:
            return False, ("a new session on it sat for %ds without reaching Remote Control, the way "
                           "sessions hang on a macOS privacy prompt" % PROBE_WAIT)
        if not same_file(img, real):
            return False, ("the probe ran %s, not the new binary, so it proves nothing about the new one"
                           % (img or "an unknown binary"))
        return True, ""
    finally:
        run([CLAUDE, "stop", pid_], 60)
        run([CLAUDE, "rm", pid_], 60)


def sessions():
    """The background sessions that are alive now, with whether each had
    Remote Control: its saved flags say so, and its logs show the link."""
    out = []
    for e in agents() or []:
        if e.get("kind") != "background" or not e.get("pid") or not e.get("id"):
            continue
        if e.get("name") == PROBE_NAME:
            continue
        flags = job(e["id"]).get("respawnFlags") or []
        out.append({"id": e["id"], "name": e.get("name"), "status": e.get("status"),
                    "rc": "--remote-control" in flags or bool(rc_link(e["id"], 0))})
    return out


def respawn_all(before, version, dry):
    """Respawn each one, idle ones first; give busy ones time to finish.
    Returns the sessions that did not come back whole."""
    todo = sorted(before, key=lambda s: s["status"] == "busy")
    bad = []
    for s in todo:
        if dry:
            print("would respawn %s (%s, %s, RC %s)" % (s["id"], s["name"], s["status"], "yes" if s["rc"] else "no"))
            continue
        end = time.monotonic() + BUSY_WAIT
        while True:
            now_ = {a.get("id"): a for a in agents() or []}.get(s["id"])
            if not now_ or not now_.get("pid"):
                break                    # it died on its own; lead-keeper's business
            if now_.get("status") != "busy" or time.monotonic() >= end:
                break
            time.sleep(30)
        if now_ and now_.get("status") == "busy":
            log("left %s (%s) on the old version: still busy after %ds" % (s["id"], s["name"], BUSY_WAIT))
            continue
        if not now_ or not now_.get("pid"):
            continue
        rc, out, err = run([CLAUDE, "respawn", s["id"]], 120)
        if rc != 0:
            bad.append("%s (%s): respawn refused: %s" % (s["name"], s["id"], plain(out + err).strip()[-150:]))
            continue
        link = rc_link(s["id"], RC_WAIT) if s["rc"] else "n/a"
        after = {a.get("id"): a for a in agents() or []}.get(s["id"]) or {}
        v = job(s["id"]).get("cliVersion")
        if not after.get("pid"):
            bad.append("%s (%s): not running after respawn" % (s["name"], s["id"]))
        elif not link:
            bad.append("%s (%s): back without Remote Control" % (s["name"], s["id"]))
        elif v and v != version:
            bad.append("%s (%s): still on %s" % (s["name"], s["id"], v))
    return bad


def main():
    dry = "--dry-run" in sys.argv[1:] or "--dry" in sys.argv[1:]
    load_env()
    path = os.path.join(STATE, "state.json")
    st = load_json(path, {})
    st.setdefault("pending", [])         # alerts that could not be sent last time go with this run's
    old, old_real = binary()
    if dry:
        g, carries = grant_path(old_real)
        print("now: Claude Code %s at %s" % (old, old_real))
        print("Full Disk Access belongs to %s (%s)" % (
            g, "an app bundle hard-linked to the version, so the grant carries" if carries
            else "the version file itself, so every version needs its own grant"))
        print("would run: %s update" % CLAUDE)
        print("if the version changes:")
        probe(old_real, True)
        respawn_all(sessions(), old, True)
        print("then check each is running, on the new version, with its RC link if it had one")
        return 0
    os.makedirs(STATE, exist_ok=True)
    with open(LOCK, "w") as f:
        f.write("%d\n" % os.getpid())
    try:
        rc, out, err = run([CLAUDE, "update"], 900)
        new, real = binary()
        if rc != 0:
            log("claude update failed (%s): %s" % (rc, plain(out + err).strip()[-200:]))
            return 0                     # nothing changed; next week tries again
        if new == old and real == old_real:
            log("up to date: %s" % new)
            return 0
        log("updated %s -> %s (%s)" % (old, new, real))
        g, carries = grant_path(real)
        ok, why = probe(real, False)
        if not ok:
            log("NOT respawning: %s" % why)
            st["pending"].append({"at": stamp(), "title": "%s: grant Full Disk Access to Claude %s" % (box(), new),
                                  "body": "Claude Code updated %s -> %s, and %s.\n"
                                          "Nothing was respawned. On %s, in System Settings > Privacy & "
                                          "Security > Full Disk Access, add:\n%s\nthen run: claude respawn --all"
                                          % (old, new, why, box(), g)})
            return 0
        before = sessions()
        bad = respawn_all(before, new, False)
        log("respawned %d of %d background sessions onto %s%s" % (
            len(before) - len(bad), len(before), new, "; not whole: " + "; ".join(bad) if bad else ""))
        if bad:
            st["pending"].append({"at": stamp(), "title": "%s: sessions not back after Claude %s" % (box(), new),
                                  "body": "After updating to %s and respawning:\n%s\n"
                                          "To bring one back with Remote Control: claude stop <id>, then from its "
                                          "cwd claude --bg --resume <sessionId> --remote-control -n \"<name>\" \"<note>\""
                                          % (new, "\n".join(bad))})
        return 0
    finally:
        try:
            os.remove(LOCK)
        except OSError:
            pass
        st["pending"], _ = flush(st["pending"], False, TAG)
        st["last_run"] = stamp()
        save_json(path, st)


if __name__ == "__main__":
    sys.exit(main())
