#!/usr/bin/python3
"""Keep the leads on this Mac, and their workers, alive and on Remote
Control. docs/lead-keeper.md.

    lead-keeper.py                one pass: revive what is down
    lead-keeper.py --dry-run      say what it would do, change nothing
    lead-keeper.py --clear [name] forget a hold or a failure, so it is tried again

Claude Code's background supervisor retires --bg sessions that sit idle (8h
with Remote Control, 1h without), and pinning does not save a session from
a low-memory retire, a crash, or a respawn that hangs on a macOS privacy
prompt. So every five minutes this reads `claude agents --json --all` and
nothing else while all is well: no model turns.

Who it keeps: every session named "<Lead>" or "<Lead> · <task>", for the
leads in mesh/leads.conf, that is still listed. `claude rm` is how a
session is finished on purpose, so a removed session never comes back; the
set is what is listed, not a list of names. Per name only the newest entry
counts: if any entry by that name is alive the name is alive, and an older
duplicate is never revived.

A name needs reviving when, two passes in a row:
  - no entry has a live process (a respawn leaves entries without a pid for
    a moment, and a listed pid can be stale), or
  - it is alive and not busy, but its saved flags have lost
    --remote-control, as the 2026-09-29 respawn did to three leads.
Never while a process it was last seen with is still alive: that is how
you get two copies.

How, from the session's own cwd (else its lead's):
  retired idle   claude respawn <id>
                 nothing was in flight, so no note and no model turn; the
                 same id and RC link.
  died           claude --bg --resume <sessionId> "<note>"
                 no other flags, so the same entry wakes with its saved
                 options, and the note tells it to SendMessage its lead
                 (a lead tells CTO) that it is back.
  lost RC        claude stop <id>, then
                 claude --bg --resume <sessionId> --remote-control -n <name> "<note>"
                 a copy with the same conversation and a new id.
Leads first, then workers; while memory is tight only the leads.

A revival counts once `claude logs <id>` shows a claude.ai/code link within
60 s. One that never shows it is the privacy-prompt hang: the attempt is
stopped, Felix hears once (ntfy, and one kapwa item on #mesh) which path
needs Full Disk Access, and nothing is revived again until the Claude
binary changes or someone runs --clear. Otherwise it is quiet: a revival is
one log line. Felix also hears when a lead cannot be revived, or dies twice
in 24 hours other than by idling, and once when it is back.
"""
import os
import re
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.realpath(__file__)))
from claude_bg import (CLAUDE, HOME, agents, box, flush, grant_path, job, load_env, load_json,  # noqa: E402
                       memory_tight, now, parse, pid_alive, plain, rc_link, read_leads, retired, run,
                       save_json, stamp, trusted, BG_ID)

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
CONF = os.environ.get("LEAD_KEEPER_CONF", os.path.join(REPO, "mesh/leads.conf"))
STATE = os.environ.get("LEAD_KEEPER_STATE", os.path.join(HOME, ".local/state/lead-keeper"))
# claude-update.py holds this while it updates and respawns; sessions it is
# restarting must not look dead to us.
UPDATE_LOCK = os.environ.get("CLAUDE_UPDATE_LOCK",
                             os.path.join(HOME, ".local/state/claude-update/running"))
CONFIRM_RUNS = int(os.environ.get("LEAD_KEEPER_CONFIRM_RUNS", "2"))
RC_WAIT = float(os.environ.get("LEAD_KEEPER_RC_WAIT", "60"))
RETRY_HOURS = float(os.environ.get("LEAD_KEEPER_RETRY_HOURS", "6"))
# A process still alive for a name the list calls dead: say so after this
# many passes (it may be a respawn that is taking its time).
HUNG_RUNS = int(os.environ.get("LEAD_KEEPER_HUNG_RUNS", "3"))
UNREADABLE_RUNS = int(os.environ.get("LEAD_KEEPER_UNREADABLE_RUNS", "6"))
DOT = " · "
TAG = "lead-keeper"

NOTE_LEAD = ("You were restarted by lead-keeper ({why}). Check your ledger and any workers "
             "you had, then SendMessage CTO: \"{name} back\".")
NOTE_WORKER = ("You were restarted by lead-keeper ({why}). SendMessage \"{lead}\" that you "
               "are back and what you were doing, then carry on.")
DOWN = ("failed", "cannot", "hung", "removed")   # alerts that say it is down, answered by "back"


def log(msg):
    print("%s %s" % (stamp(), msg), flush=True)


def read_conf():
    return read_leads(CONF)


def update_running():
    """Is claude-update.py mid-update? Its lock holds its pid."""
    try:
        pid = int(open(UPDATE_LOCK).read().split()[0])
        os.kill(pid, 0)
        return True
    except (OSError, ValueError, IndexError):
        return False


def newest(entries):
    return max(entries, key=lambda e: e.get("startedAt") or 0) if entries else None


def targets(listing, leads, excludes):
    """Every name to keep, leads first: (name, lead, entries). A configured
    lead with no entries is included, so its absence can be noticed."""
    by_name = {}
    for e in listing:
        n = e.get("name")
        if n:
            by_name.setdefault(n, []).append(e)
    out = [(lead["name"], lead, by_name.get(lead["name"], [])) for lead in leads
           if lead["name"] not in excludes]
    workers = []
    for n, es in by_name.items():
        for lead in leads:
            if n.startswith(lead["name"] + DOT) and n not in excludes:
                workers.append((n, lead, es))
                break
    return out + sorted(workers, key=lambda t: t[0])


def alert(st, ss, key, title, body, name=None):
    """Queue one alert, unless this one (key) was already said for this
    session since it was last seen well."""
    said = ss.setdefault("said", [])
    if key in said:
        return
    said.append(key)
    st["pending"].append({"at": stamp(), "title": title, "body": body, "lead": name, "down": key in DOWN})
    log("ALERT %s" % title)


def back(st, ss, name, e):
    """Something Felix was told is down is up again: say so once, and close
    the kapwa item that said it was down rather than opening another."""
    item = ss.pop("kapwa_item", None)
    st["pending"].append({"at": stamp(), "title": "%s: %s is back" % (box(), name),
                          "body": "%s is up again as %s (pid %s)." % (name, e.get("id") or e.get("kind"), e.get("pid")),
                          "priority": "default", "tags": "white_check_mark",
                          "kapwa_done": item, "kapwa_sent": None if item else True})
    log("BACK %s" % name)


def plan(name, lead, e, ss, why, alive):
    """How to bring back `name`, whose newest entry is e (None if it is no
    longer listed): (method, [argvs], cwd) or (None, reason, None)."""
    is_lead = name == lead["name"]
    if e is None:
        # Only an interactive lead (CTO, run by Remote Control rather than
        # the supervisor) can vanish from the list without being removed.
        if not (is_lead and ss.get("kind") == "interactive" and ss.get("session_id")):
            return None, "removed", None
        sid, cwd, flags = ss["session_id"], lead["cwd"], []
    else:
        sid = e.get("sessionId")
        cwd = e.get("cwd") or lead["cwd"]
        flags = (job(e["id"]).get("respawnFlags") or []) if e.get("id") else []
    if not os.path.isdir(cwd):
        cwd = lead["cwd"]
    if not os.path.isdir(cwd):
        return None, "its cwd %s does not exist" % cwd, None
    if trusted(cwd) is False:
        return None, "Claude does not trust %s, so a session there refuses to start" % cwd, None
    if is_lead and lead["note"]:
        note = lead["note"]
    else:
        note = (NOTE_LEAD if is_lead else NOTE_WORKER).format(name=name, lead=lead["name"], why=why)
    copy = [CLAUDE, "--bg", "--resume", sid, "--remote-control", "-n", name, note]
    if alive:
        return "rc", [[CLAUDE, "stop", e["id"]], copy], cwd
    if e is None or e.get("kind") != "background" or not e.get("id") or "--remote-control" not in flags:
        return "copy", [copy], cwd
    if retired(e["id"], e.get("startedAt")):
        return "respawn", [[CLAUDE, "respawn", e["id"]]], cwd
    return "wake", [[CLAUDE, "--bg", "--resume", sid, note]], cwd


def gone(short_id):
    """Wait (up to 30 s) for a stopped session's process to go."""
    for _ in range(15):
        listed = [a for a in agents() or [] if a.get("id") == short_id]
        if not (listed and listed[0].get("pid") and pid_alive(listed[0]["pid"])):
            return True
        time.sleep(2)
    return False


def revive(st, ss, name, is_lead, method, argvs, cwd, dry):
    """Run the plan. True if it came back with Remote Control."""
    if dry:
        for argv in argvs:
            print("  would run, from %s:\n    %s" % (cwd, " ".join(a if " " not in a else repr(a) for a in argv)))
        return True
    new_id, text, rc = None, "", 0
    for argv in argvs:
        rc, out, err = run(argv, 120, cwd=cwd)
        text = plain(out + err)
        if rc != 0:
            break
        if argv[1] == "stop":
            if not gone(argv[2]):
                rc, text = 1, "%s did not stop within 30 s" % argv[2]
                break
            continue
        m = BG_ID.search(text) or re.search(r"(?:woke session|respawned) ([0-9a-f]{8})", text)
        new_id = m.group(1) if m else (argv[2] if argv[1] == "respawn" else None)
    if method in ("wake", "copy") and is_lead and not ss.get("failed_at"):
        ss.setdefault("deaths", []).append(stamp())   # idling and lost RC are not deaths
    link = rc_link(new_id, RC_WAIT) if (rc == 0 and new_id) else None
    if link:
        log("revived %s by %s: %s -> %s %s" % (name, method, ss.get("id") or "-", new_id, link))
        ss.update({"id": new_id, "failed_at": None, "runs": 0, "revived_at": stamp()})
        return True
    if rc != 0 or not new_id:
        why = "the command failed (%s): %s" % (rc, text.strip().splitlines()[-1][:200] if text.strip() else "no output")
        log("revival FAILED %s by %s: %s" % (name, method, why))
        ss.update({"failed_at": stamp(), "runs": 0})
        if is_lead:
            alert(st, ss, "failed", "%s: %s did not come back" % (box(), name),
                  "lead-keeper tried to revive %s (%s) and %s. It tries again every %gh; you hear again only "
                  "when it is back. Look with: claude agents" % (name, method, why, RETRY_HOURS), name)
        return False
    # Started, but no Remote Control: the privacy-prompt hang. This copy is
    # ours, and stuck it would look alive for ever, so stop it; it is a dead
    # entry again with its conversation kept.
    run([CLAUDE, "stop", new_id], 60)
    real = os.path.realpath(CLAUDE)
    grant, _ = grant_path(real)
    log("revival FAILED %s by %s: %s showed no Remote Control link in %ds; holding every revival "
        "until the Claude binary changes" % (name, method, new_id, RC_WAIT))
    ss.update({"runs": 0})
    st["hold"] = {"binary": real, "at": stamp(), "name": name}
    alert(st, st.setdefault("box", {}), "tcc", "%s: grant Full Disk Access to Claude" % box(),
          "lead-keeper revived %s (%s) as %s and it sat %ds without reaching Remote Control: the "
          "macOS privacy-prompt hang. It stopped that attempt and will revive nothing until the Claude "
          "binary changes. On %s, System Settings > Privacy & Security > Full Disk Access, add:\n%s\n"
          "then run: %s --clear" % (name, method, new_id, RC_WAIT, box(), grant,
                                    os.path.join(REPO, "scripts/lead-keeper.py")))
    return False


def main():
    args = sys.argv[1:]
    dry = "--dry-run" in args or "--dry" in args
    load_env()
    path = os.path.join(STATE, "state.json")
    st = load_json(path, {})
    st.setdefault("sessions", {})
    st.setdefault("pending", [])
    if "--clear" in args:
        rest = [a for a in args if not a.startswith("--")]
        if not rest:
            st.pop("hold", None)
            st.pop("box", None)
        for n, ss in st["sessions"].items():
            if not rest or n in rest:
                ss.update({"failed_at": None, "runs": 0})
        save_json(path, st)
        print("cleared %s" % (", ".join(rest) if rest else "everything"))
        return 0
    if update_running():
        if dry:
            print("claude-update is running; this pass would do nothing")
        return 0
    hold = st.get("hold")
    if hold and hold.get("binary") != os.path.realpath(CLAUDE):
        log("hold lifted: Claude is now %s" % os.path.realpath(CLAUDE))
        st.pop("hold")
        st.pop("box", None)
        hold = None
    listing = agents()
    if listing is None:
        st["unreadable_runs"] = st.get("unreadable_runs", 0) + 1
        if st["unreadable_runs"] == UNREADABLE_RUNS:
            st["pending"].append({"at": stamp(), "title": "%s: lead-keeper is blind" % box(),
                                  "body": "`%s agents --json --all` has failed %d passes in a row, "
                                          "so nothing is being watched." % (CLAUDE, UNREADABLE_RUNS)})
        if dry:
            print("claude agents --json --all could not be read; nothing judged")
    else:
        st["unreadable_runs"] = 0
        leads, excludes = read_conf()
        tight = None
        seen = set()
        for name, lead, entries in targets(listing, leads, excludes):
            seen.add(name)
            is_lead = name == lead["name"]
            ss = st["sessions"].setdefault(name, {})
            live = [e for e in entries if e.get("pid") and pid_alive(e["pid"])]
            e = newest(live) or newest(entries)
            if live:
                j = job(e["id"]) if e.get("kind") == "background" and e.get("id") else None
                lost_rc = bool(j) and "--remote-control" not in (j.get("respawnFlags") or [])
                if not lost_rc or e.get("status") == "busy":
                    if dry:
                        print("%-30s up      %s pid %s (%s)%s" % (name, e.get("id") or e.get("kind"), e["pid"],
                              e.get("status"), "; RC lost, waits until it is not busy" if lost_rc else ""))
                    elif set(ss.get("said", [])) & set(DOWN):
                        back(st, ss, name, e)
                    ss.update({"session_id": e.get("sessionId"), "id": e.get("id"), "pid": e["pid"],
                               "kind": e.get("kind"), "seen": stamp(), "runs": 0, "hung_runs": 0, "said": []})
                    continue
                why = "your Remote Control had dropped, so it stopped you and started you again with it"
            elif pid_alive(ss.get("pid")):
                # Nothing listed is alive, but the process it last had is: a
                # respawn under way, or a hung one. A revival now would
                # make a second copy.
                ss["hung_runs"] = ss.get("hung_runs", 0) + 1
                if dry:
                    print("%-30s ?       not listed alive, but its last process %s is: would wait" % (name, ss["pid"]))
                if ss["hung_runs"] >= HUNG_RUNS and is_lead:
                    alert(st, ss, "hung", "%s: lead %s is not answering" % (box(), name),
                          "claude agents shows no live process for %s, but pid %s from it is still alive "
                          "(%d passes). lead-keeper will not start a second copy. "
                          "Look with: ps -p %s; claude agents" % (name, ss["pid"], ss["hung_runs"], ss["pid"]), name)
                continue
            else:
                why = "your process had died"
            if e is None and not ss.get("seen"):
                if dry:
                    print("%-30s -       not listed and never seen: nothing to revive" % name)
                continue
            ss["runs"] = ss.get("runs", 0) + 1
            method, argvs, cwd = plan(name, lead, e, ss, why, bool(live))
            if dry:
                print("%-30s %s (%d of %d passes)%s" % (name, "RC LOST" if live else "DEAD   ", ss["runs"], CONFIRM_RUNS,
                      "" if method else ": will not revive: " + ("removed with claude rm" if argvs == "removed" else argvs)))
            if method is None:
                if is_lead and ss["runs"] >= CONFIRM_RUNS:
                    if argvs == "removed":
                        alert(st, ss, "removed", "%s: lead %s is gone" % (box(), name),
                              "%s is no longer in `claude agents --all`: it was removed, and removed sessions "
                              "are not brought back. If it was retired on purpose, take it out of "
                              "mesh/leads.conf." % name, name)
                    else:
                        alert(st, ss, "cannot", "%s: lead %s is down" % (box(), name),
                              "%s is down and lead-keeper cannot revive it: %s." % (name, argvs), name)
                continue
            if ss["runs"] < CONFIRM_RUNS and not dry:
                continue
            if hold:
                if dry:
                    print("  held: a revival sat on a privacy prompt at %s; nothing is revived until "
                          "Claude changes from %s, or --clear" % (hold["at"], hold["binary"]))
                continue
            failed = ss.get("failed_at")
            if failed and (now() - parse(failed)).total_seconds() < RETRY_HOURS * 3600:
                if dry:
                    print("  last revival failed at %s; would wait until %gh after it" % (failed, RETRY_HOURS))
                continue
            if not is_lead:
                if tight is None:
                    tight = memory_tight()
                if tight:
                    if dry:
                        print("  memory is tight: workers wait")
                    elif not ss.get("deferred"):
                        ss["deferred"] = True
                        log("deferred %s: memory is tight" % name)
                    continue
            ss.pop("deferred", None)
            ok = revive(st, ss, name, is_lead, method, argvs, cwd, dry)
            if not ok and st.get("hold"):
                hold = st["hold"]            # the rest would hang the same way
            day = [d for d in ss.get("deaths", []) if (now() - parse(d)).total_seconds() < 86400]
            ss["deaths"] = day
            last = ss.get("twice_at")
            if len(day) >= 2 and not dry and not (last and (now() - parse(last)).total_seconds() < 86400):
                ss["twice_at"] = stamp()     # once a day, however often it dies
                alert(st, ss, "twice:" + stamp(), "%s: lead %s died twice in 24h" % (box(), name),
                      "%s has been revived %d times since %s, not counting idle retires. Something keeps "
                      "killing it; see ~/.claude/daemon.log." % (name, len(day), day[0]), name)
        # A name no longer listed at all was removed on purpose: forget it.
        for n in [n for n in st["sessions"] if n not in seen]:
            del st["sessions"][n]
    if dry:
        for a in st["pending"]:
            print("waiting to be sent: %s" % a["title"])
        return 0
    st["pending"], sent = flush(st["pending"], False, TAG)
    for a in sent:  # remember where trouble was raised, to close it there
        if a.get("down") and a.get("lead") in st["sessions"] and isinstance(a["kapwa_sent"], str):
            st["sessions"][a["lead"]]["kapwa_item"] = a["kapwa_sent"]
    save_json(path, st)
    with open(os.path.join(STATE, "last-run"), "w") as f:
        f.write(stamp() + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
