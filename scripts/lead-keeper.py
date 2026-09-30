#!/usr/bin/python3
"""Keep the lead sessions on this Mac alive: when one dies, bring it back
with its conversation and its Remote Control link. docs/lead-keeper.md.

    lead-keeper.py              one pass: revive any lead that is down
    lead-keeper.py --dry-run    say what it would do, change nothing

Claude Code's background supervisor retires --bg sessions that sit idle (8h
with Remote Control, 1h without), and pinning a session does not save it
from a low-memory retire, a crash, or a respawn that hangs on a macOS
privacy prompt. The leads in mesh/leads.conf have to be there when Felix or
CTO talks to them, so this looks every five minutes. While every lead is up
it runs one `claude agents --json --all` and nothing else: no model turns.

A lead is dead when no entry with its exact name has a pid. It is revived
only after two passes in a row have seen it dead (a `claude respawn --all`
or a supervisor restart leaves entries without a pid for a moment), and
never while a process it was last seen with is still alive, because that
is how you get two copies of one lead. Then, from the lead's cwd:

  1. wake in place:  claude --bg --resume <sessionId> "<note>"
     No other flags, so the supervisor wakes the same entry with its saved
     options: same id, same conversation, same Remote Control link.
  2. if its entry is gone (someone ran `claude rm`), start it again from the
     transcript: claude --bg --resume <sessionId> --remote-control -n <name> "<note>"

Either way the revival counts only once `claude logs <id>` shows a
claude.ai/code link within 60 s. If it does not, the new process may be
stuck on a privacy prompt: that is alerted, and the lead is not tried again
for RETRY_HOURS, so a stuck box does not fill up with stuck copies.

Quiet by design. A revival is one line in the log and nothing else. Felix
hears (ntfy, and one kapwa item on #mesh) only when a lead is down for
real: a revival failed, a lead cannot be revived at all, or the same lead
died twice in 24 hours. Each of those is said once, not every five minutes.
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
from claude_bg import (CLAUDE, HOME, agents, box, flush, load_env, load_json, now, parse,  # noqa: E402
                       pid_alive, plain, rc_link, run, save_json, stamp, transcript, trusted, BG_ID)

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
CONF = os.environ.get("LEAD_KEEPER_CONF", os.path.join(REPO, "mesh/leads.conf"))
STATE = os.environ.get("LEAD_KEEPER_STATE", os.path.join(HOME, ".local/state/lead-keeper"))
# claude-update.py holds this while it updates and respawns; leads it is
# restarting must not look dead to us.
UPDATE_LOCK = os.environ.get("CLAUDE_UPDATE_LOCK",
                             os.path.join(HOME, ".local/state/claude-update/running"))
CONFIRM_RUNS = int(os.environ.get("LEAD_KEEPER_CONFIRM_RUNS", "2"))
RC_WAIT = float(os.environ.get("LEAD_KEEPER_RC_WAIT", "60"))
RETRY_HOURS = float(os.environ.get("LEAD_KEEPER_RETRY_HOURS", "6"))
# A process still alive for a lead the list calls dead: say so after this
# many passes (it may be a respawn that is taking its time).
HUNG_RUNS = int(os.environ.get("LEAD_KEEPER_HUNG_RUNS", "3"))
UNREADABLE_RUNS = int(os.environ.get("LEAD_KEEPER_UNREADABLE_RUNS", "6"))
TAG = "lead-keeper"

NOTE = ("You were restarted by lead-keeper after your process died ({why}). "
        "Check your ledger and any workers you had, then SendMessage CTO: \"{name} back\".")


def log(msg):
    print("%s %s" % (stamp(), msg), flush=True)


def leads():
    out = []
    for line in open(CONF):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = [p.strip() for p in line.split(" | ")]
        if len(parts) < 2 or not parts[0]:
            raise SystemExit("%s: bad line: %s" % (CONF, line))
        out.append({"name": parts[0], "cwd": os.path.expanduser(parts[1]),
                    "note": parts[2] if len(parts) > 2 and parts[2] else ""})
    return out


def update_running():
    """Is claude-update.py mid-update? Its lock holds its pid."""
    try:
        pid = int(open(UPDATE_LOCK).read().split()[0])
    except (OSError, ValueError, IndexError):
        return False
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


DOWN = ("failed", "cannot", "hung")   # alerts that say a lead is down, answered by "back"


def alert(st, ls, key, title, body):
    """Queue one alert, unless this one (key) was already said for this lead
    since it was last seen alive."""
    said = ls.setdefault("said", [])
    if key in said:
        return
    said.append(key)
    st["pending"].append({"at": stamp(), "title": title, "body": body, "lead": ls["name"],
                          "down": key in DOWN})
    log("ALERT %s" % title)


def back(st, ls, e):
    """A lead Felix was told is down is up again: say so once, on ntfy, and
    close the kapwa item that said it was down rather than opening another."""
    item = ls.pop("kapwa_item", None)
    st["pending"].append({"at": stamp(), "title": "%s: lead %s is back" % (box(), ls["name"]),
                          "body": "%s is up again as %s (pid %s)." % (ls["name"], e.get("id") or e.get("kind"), e["pid"]),
                          "priority": "default", "tags": "white_check_mark",
                          "kapwa_done": item, "kapwa_sent": None if item else True})
    log("BACK %s" % ls["name"])


def newest(entries):
    return max(entries, key=lambda e: e.get("startedAt") or 0) if entries else None


def plan(lead, entries, ls):
    """How this dead lead would be revived: (method, argv, session id) or
    (None, why, None) when it cannot be."""
    dead = newest(entries)
    sid = (dead or {}).get("sessionId") or ls.get("session_id")
    if not sid:
        return None, "it has never been seen, so there is no conversation to resume", None
    cwd = lead["cwd"]
    if not os.path.isdir(cwd):
        return None, "its cwd %s does not exist" % cwd, sid
    if trusted(cwd) is False:
        return None, "Claude does not trust %s, so a session there refuses to start" % cwd, sid
    note = lead["note"] or NOTE.format(name=lead["name"], why="found dead by lead-keeper at %s" % stamp())
    if dead and dead.get("kind") == "background" and dead.get("id"):
        return "wake", [CLAUDE, "--bg", "--resume", sid, note], sid
    if not os.path.exists(transcript(cwd, sid)):
        return None, "its transcript is not at %s" % transcript(cwd, sid), sid
    return "resume", [CLAUDE, "--bg", "--resume", sid, "--remote-control", "-n", lead["name"], note], sid


def revive(lead, method, argv, sid, st, ls, dry):
    name = lead["name"]
    if dry:
        print("  would run, from %s:\n    %s" % (lead["cwd"], " ".join(
            a if " " not in a else repr(a) for a in argv)))
        return
    rc, out, err = run(argv, 120, cwd=lead["cwd"])
    text = plain(out + err)
    m = BG_ID.search(text) or re.search(r"woke session ([0-9a-f]{8})", text)
    new_id = m.group(1) if m else None
    if not ls.get("failed_at"):           # a retry is the same death, not another
        ls.setdefault("deaths", []).append(stamp())
    link = rc_link(new_id, RC_WAIT) if (rc == 0 and new_id) else None
    if link:
        log("revived %s by %s: %s -> %s %s" % (name, method, ls.get("id") or "-", new_id, link))
        ls.update({"id": new_id, "failed_at": None, "dead_runs": 0, "revived_at": stamp()})
        if method == "resume":
            ls["session_id"] = None  # a copy has its own; read it next pass
        return
    why = ("the command failed (%s): %s" % (rc, text.strip().splitlines()[-1][:200] if text.strip() else "no output")
           if rc != 0 or not new_id else
           "%s started but showed no Remote Control link in %ds; it may be stuck on a macOS "
           "privacy prompt (Full Disk Access for the Claude binary)" % (new_id, RC_WAIT))
    if new_id:
        # This copy is ours, and a copy stuck on a privacy prompt would look
        # alive forever and never be retried. Stopped, it is a dead entry
        # again: its conversation is kept, and the retry wakes it in place.
        run([CLAUDE, "stop", new_id], 60)
    log("revival FAILED %s by %s: %s" % (name, method, why))
    ls.update({"failed_at": stamp(), "dead_runs": 0})
    alert(st, ls, "failed", "%s: lead %s did not come back" % (box(), name),
          "lead-keeper tried to revive %s (%s, session %s) and %s.\n"
          "It stopped that attempt and tries again every %gh; you hear again only when it is back. "
          "Look with: claude agents; claude logs %s" % (name, method, sid, why, RETRY_HOURS, new_id or "<id>"))


def main():
    dry = "--dry-run" in sys.argv[1:] or "--dry" in sys.argv[1:]
    load_env()
    path = os.path.join(STATE, "state.json")
    st = load_json(path, {})
    st.setdefault("leads", {})
    st.setdefault("pending", [])
    if update_running():
        if dry:
            print("claude-update is running; this pass would do nothing")
        return 0
    listing = agents()
    if listing is None:
        st["unreadable_runs"] = st.get("unreadable_runs", 0) + 1
        if st["unreadable_runs"] == UNREADABLE_RUNS:
            st["pending"].append({"at": stamp(), "title": "%s: lead-keeper is blind" % box(),
                                  "body": "`%s agents --json --all` has failed %d passes in a row, "
                                          "so no lead is being watched." % (CLAUDE, UNREADABLE_RUNS)})
        if dry:
            print("claude agents --json --all could not be read; nothing judged")
    else:
        st["unreadable_runs"] = 0
        for lead in leads():
            name = lead["name"]
            ls = st["leads"].setdefault(name, {})
            ls["name"] = name
            mine = [e for e in listing if e.get("name") == name]
            live = [e for e in mine if e.get("pid")]
            if live:
                e = newest(live)
                if dry:
                    print("%-14s up    %s pid %s (%s)" % (name, e.get("id") or e.get("kind"), e["pid"], e.get("status")))
                elif set(ls.get("said", [])) & set(DOWN):
                    back(st, ls, e)
                ls.update({"session_id": e.get("sessionId"), "id": e.get("id"), "pid": e["pid"],
                           "seen": stamp(), "dead_runs": 0, "hung_runs": 0, "said": []})
                continue
            if pid_alive(ls.get("pid")):
                # The list has no pid, but the process it last had is alive:
                # a respawn under way, or a hung one. Either way a revival
                # now would make a second copy.
                ls["hung_runs"] = ls.get("hung_runs", 0) + 1
                if dry:
                    print("%-14s ?     unlisted, but its last process %s is alive: would wait" % (name, ls["pid"]))
                if ls["hung_runs"] >= HUNG_RUNS:
                    alert(st, ls, "hung", "%s: lead %s is not answering" % (box(), name),
                          "claude agents lists no process for %s, but pid %s from it is still alive "
                          "(%d passes). lead-keeper will not start a second copy. "
                          "Look with: ps -p %s; claude agents" % (name, ls["pid"], ls["hung_runs"], ls["pid"]))
                continue
            ls["dead_runs"] = ls.get("dead_runs", 0) + 1
            method, argv, sid = plan(lead, mine, ls)
            if dry:
                print("%-14s DEAD  (%d of %d passes)%s" % (name, ls["dead_runs"], CONFIRM_RUNS,
                      "" if method else ": cannot revive: " + argv))
            if method is None:
                if ls["dead_runs"] >= CONFIRM_RUNS:
                    alert(st, ls, "cannot", "%s: lead %s is down" % (box(), name),
                          "%s is dead and lead-keeper cannot revive it: %s." % (name, argv))
                continue
            if ls["dead_runs"] < CONFIRM_RUNS and not dry:
                continue
            failed = ls.get("failed_at")
            if failed and (now() - parse(failed)).total_seconds() < RETRY_HOURS * 3600:
                if dry:
                    print("  last revival failed at %s; would wait until %gh after it" % (failed, RETRY_HOURS))
                continue
            revive(lead, method, argv, sid, st, ls, dry)
            day = [d for d in ls.get("deaths", []) if (now() - parse(d)).total_seconds() < 86400]
            ls["deaths"] = day
            last = ls.get("twice_at")
            if len(day) >= 2 and not dry and not (last and (now() - parse(last)).total_seconds() < 86400):
                ls["twice_at"] = stamp()   # once a day, however often it dies
                alert(st, ls, "twice:" + stamp(), "%s: lead %s died twice in 24h" % (box(), name),
                      "%s has been revived %d times since %s (last: %s). Something keeps killing it; "
                      "see ~/.claude/daemon.log for 'bg retire' and 'settled' lines."
                      % (name, len(day), day[0], ls.get("revived_at") or ls.get("failed_at")))
    if dry:
        for a in st["pending"]:
            print("waiting to be sent: %s" % a["title"])
        return 0
    st["pending"], sent = flush(st["pending"], False, TAG)
    for a in sent:  # remember where each lead's trouble was raised, to close it there
        if a.get("down") and a.get("lead") in st["leads"] and isinstance(a["kapwa_sent"], str):
            st["leads"][a["lead"]]["kapwa_item"] = a["kapwa_sent"]
    save_json(path, st)
    with open(os.path.join(STATE, "last-run"), "w") as f:
        f.write(stamp() + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
