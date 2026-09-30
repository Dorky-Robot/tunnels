"""What lead-keeper.py and claude-update.py both need from Claude Code's
background sessions, and the one way both of them tell Felix something.

Everything here shells out to the `claude` CLI by full path: launchd gives a
job /usr/bin:/bin and nothing else, and on 2026-09-29 "command not found"
over a non-login shell was PATH, not a missing install.

Learned on throwaway sessions (2026-09-30, Claude Code 2.1.285):
  - A session is dead when its `claude agents --json --all` entry has no pid.
  - `claude --bg --resume <sessionId> "<note>"`, with NO other flags, wakes a
    dead background session in place: same id, same conversation, its saved
    --remote-control and -n, and the same claude.ai/code link. The note
    arrives as its next prompt.
  - Pass flags to that command and you get a copy with a new id instead
    ("keeps its own saved options, so the flags you passed started a copy").
  - `claude rm <id>` keeps the transcript; after it, the flagged --resume
    brings the conversation back (and may even keep the id). Plain
    `claude respawn <id>` also wakes a dead entry, but carries no note.
  - `claude logs <id>` on a dead session says "job not found", so an RC link
    in the logs is always from the live process.
  - A promptless `claude --bg --remote-control -n <name>` shows its RC link
    within seconds and spends no tokens: a free probe of a binary.
"""
import datetime as dt
import json
import os
import re
import subprocess
import time

HOME = os.path.expanduser("~")
CLAUDE = os.environ.get("CLAUDE_BIN", os.path.join(HOME, ".local/bin/claude"))
KAPWA = os.environ.get("KAPWA_BIN", os.path.join(HOME, ".local/bin/kapwa"))
# mesh-watch's alert channel: one topic, one env file, the topic never in git.
ENVFILE = os.environ.get("MESH_WATCH_ENV", os.path.join(HOME, ".config/mesh-watch/env"))
PATH = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
os.environ["PATH"] = os.path.join(HOME, ".local/bin") + ":" + os.environ.get("PATH", "") + ":" + PATH

ANSI = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07")
RC_LINK = re.compile(r"claude\.ai/code/session_[A-Za-z0-9]+")
BG_ID = re.compile(r"backgrounded\s*·\s*([0-9a-f]{8})")


def now():
    return dt.datetime.now(dt.timezone.utc)


def stamp(t=None):
    return (t or now()).strftime("%Y-%m-%dT%H:%M:%SZ")


def parse(s):
    return dt.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)


def box():
    return os.uname().nodename.split(".")[0]


def load_env():
    if os.path.exists(ENVFILE):
        for line in open(ENVFILE):
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                os.environ.setdefault(k.strip(), v.strip().strip("'\""))


def run(argv, timeout, cwd=None):
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout,
                           stdin=subprocess.DEVNULL, cwd=cwd)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 124, "", "timed out after %ss" % timeout
    except OSError as e:
        return 127, "", str(e)


def plain(s):
    return ANSI.sub("", s or "")


def agents():
    """Every session, live and dead, or None when the list cannot be read.
    None is unknown, not empty: nothing is revived on the strength of it."""
    rc, out, err = run([CLAUDE, "agents", "--json", "--all"], 60)
    if rc != 0:
        return None
    try:
        data = json.loads(out)
    except ValueError:
        return None
    return data if isinstance(data, list) else None


def pid_alive(pid):
    """Is there a process with this pid, and is it Claude? A pid reused by
    something else is not a lead."""
    if not pid:
        return False
    try:
        os.kill(int(pid), 0)
    except (OSError, ValueError):
        return False
    rc, out, _ = run(["/bin/ps", "-o", "command=", "-p", str(pid)], 10)
    return rc == 0 and "claude" in out


def rc_link(short_id, wait, gap=2.0):
    """Poll `claude logs` until the session shows its Remote Control link.
    The link, or None if none appeared within `wait` seconds."""
    end = time.monotonic() + wait
    while True:
        rc, out, _ = run([CLAUDE, "logs", short_id], 20)
        m = RC_LINK.search(plain(out)) if rc == 0 else None
        if m:
            return "https://" + m.group(0)
        if time.monotonic() >= end:
            return None
        time.sleep(gap)


BUNDLE = os.environ.get("CLAUDE_BUNDLE", os.path.join(HOME, ".local/share/claude/ClaudeCode.app"))
JOBS = os.environ.get("CLAUDE_JOBS", os.path.join(HOME, ".claude/jobs"))
DAEMON_LOG = os.environ.get("CLAUDE_DAEMON_LOG", os.path.join(HOME, ".claude/daemon.log"))


def job(id_):
    """The supervisor's saved state for a background session: its
    respawnFlags say whether it comes back with --remote-control."""
    try:
        return json.load(open(os.path.join(JOBS, id_, "state.json")))
    except (OSError, ValueError, TypeError):
        return {}


def grant_path(real):
    """What Full Disk Access has to be granted to for this binary. On
    dorkyrobot2 the installer keeps an app bundle whose executable is a
    hard link to the current version, and the grant belongs to the bundle
    (bundle id plus signature), so it carries across versions. Without that
    link (the mini) the process runs the version file itself, a path macOS
    has never seen. (path, whether it is the bundle)."""
    exe = os.path.join(BUNDLE, "Contents/MacOS/claude")
    try:
        if os.stat(exe).st_ino == os.stat(real).st_ino:
            return BUNDLE, True
    except OSError:
        pass
    return real, False


def retired(short_id, since_ms):
    """Did the supervisor retire this session for sitting idle, after it
    started? ("[bg] bg retire <id>: idle-prompt, idle 8h"). Nothing was
    in flight then, so it can come back without a word."""
    try:
        with open(DAEMON_LOG, "rb") as f:
            f.seek(0, 2)
            f.seek(max(0, f.tell() - 512 * 1024))
            tail = f.read().decode("utf-8", "replace")
    except OSError:
        return False
    for m in re.finditer(r"^\[([0-9T:.\-]+)Z\] \[bg\] bg retire %s: (idle-prompt|settled)" % re.escape(short_id),
                         tail, re.M):
        try:
            t = dt.datetime.strptime(m.group(1)[:19], "%Y-%m-%dT%H:%M:%S").replace(tzinfo=dt.timezone.utc)
        except ValueError:
            continue
        if t.timestamp() * 1000 >= (since_ms or 0):
            return True
    return False


# Lines a session appends without taking a turn: a message queued to it,
# a reminder, bookkeeping. None of them means it moved on.
IDLE_LINES = ("queue-operation", "attachment", "bridge-session", "cost-state", "last-prompt",
              "summary", "system")


def trust_stall(path, min_age):
    """Is this session stuck on the invisible trust prompt a new worktree
    raises? (2026-09-30, e961533d.) Its transcript ends at the tool_result
    of the step that moved it into a worktree (EnterWorktree, or a Bash
    command about a worktree) and has grown nothing but queued messages and
    bookkeeping for `min_age` seconds. A long tool call does not match: it
    ends at the tool_use, since the result is not back yet. Returns
    {since, tool, what, queued} or None."""
    try:
        with open(path, "rb") as f:
            f.seek(0, 2)
            f.seek(max(0, f.tell() - 512 * 1024))
            lines = f.read().decode("utf-8", "replace").splitlines()
    except OSError:
        return None
    rows = []
    for l in lines:
        try:
            rows.append(json.loads(l))
        except ValueError:
            continue
    queued, last = 0, None
    for i in range(len(rows) - 1, -1, -1):
        r = rows[i]
        if r.get("type") in ("user", "assistant") and r.get("message"):
            last = i
            break
        if r.get("type") == "queue-operation" and r.get("operation") == "enqueue":
            queued += 1
    if last is None or rows[last]["type"] != "user":
        return None
    content = rows[last]["message"].get("content")
    ids = [b.get("tool_use_id") for b in content if isinstance(b, dict) and b.get("type") == "tool_result"] \
        if isinstance(content, list) else []
    if not ids:
        return None
    use = None
    for r in reversed(rows[:last]):
        c = (r.get("message") or {}).get("content") if r.get("type") == "assistant" else None
        for b in c if isinstance(c, list) else []:
            if isinstance(b, dict) and b.get("type") == "tool_use" and b.get("id") in ids:
                use = b
        if use:
            break
    if not use:
        return None
    inp = use.get("input") or {}
    what = inp.get("command") or inp.get("name") or inp.get("path") or ""
    if not (use.get("name") == "EnterWorktree" or (use.get("name") == "Bash" and "worktree" in what)):
        return None
    try:
        since = dt.datetime.strptime(rows[last]["timestamp"][:19], "%Y-%m-%dT%H:%M:%S").replace(tzinfo=dt.timezone.utc)
    except (KeyError, ValueError):
        return None
    if (now() - since).total_seconds() < min_age:
        return None
    return {"since": stamp(since), "tool": use.get("name"), "what": str(what)[:160], "queued": queued}


def memory_tight():
    """Is the machine short of memory? The kernel's own pressure level:
    1 normal, 2 warning, 4 critical."""
    rc, out, _ = run(["/usr/sbin/sysctl", "-n", "kern.memorystatus_vm_pressure_level"], 10)
    try:
        return int(out.strip()) >= int(os.environ.get("LEAD_KEEPER_PRESSURE_LEVEL", "2"))
    except ValueError:
        return False


LEADS = os.environ.get("LEAD_KEEPER_CONF",
                       os.path.join(os.path.dirname(os.path.dirname(os.path.realpath(__file__))), "mesh/leads.conf"))


def read_leads(path=None):
    """mesh/leads.conf: ([{name, cwd, note}], {excluded names})."""
    leads, excludes = [], set()
    for line in open(path or LEADS):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = [p.strip() for p in line.split(" | ")]
        if parts[0] == "exclude" and len(parts) == 2:
            excludes.add(parts[1])
            continue
        if len(parts) < 2 or not parts[0]:
            raise SystemExit("%s: bad line: %s" % (path or LEADS, line))
        leads.append({"name": parts[0], "cwd": os.path.expanduser(parts[1]),
                      "note": parts[2] if len(parts) > 2 and parts[2] else ""})
    return leads, excludes


def kept(name):
    """Is this a session lead-keeper looks after: a lead, or '<lead> · <task>'?"""
    try:
        leads, excludes = read_leads()
    except (OSError, SystemExit):
        return False
    return name not in excludes and any(name == l["name"] or name.startswith(l["name"] + " · ") for l in leads)


def trusted(cwd):
    """Has the trust dialog been accepted for cwd or a folder above it? An
    untrusted cwd makes a background session refuse to start."""
    try:
        projects = json.load(open(os.path.join(HOME, ".claude.json"))).get("projects", {})
    except (OSError, ValueError):
        return None
    d = os.path.realpath(cwd)
    while True:
        if (projects.get(d) or {}).get("hasTrustDialogAccepted"):
            return True
        up = os.path.dirname(d)
        if up == d:
            return False
        d = up


def transcript(cwd, session_id):
    """Where Claude keeps a session's conversation: the file --resume reads."""
    return os.path.join(HOME, ".claude/projects", re.sub(r"[^A-Za-z0-9]", "-", cwd),
                        session_id + ".jsonl")


# ---- telling Felix ----------------------------------------------------------

def ntfy(title, body, dry, priority="high", tags="rotating_light"):
    """True if it left the box (or was printed, when dry)."""
    if dry:
        print("ALERT %s\n%s\n" % (title, body))
        return True
    url = os.environ.get("MESH_WATCH_NTFY")
    if not url:
        return False
    # The title is an HTTP header, read as Latin-1: a worker's "·" arrives as
    # "Â·". Plain ASCII there; the body is UTF-8 and keeps the exact name.
    title = title.replace("\u00b7", "-").encode("ascii", "replace").decode()
    rc, _, _ = run(["curl", "-sS", "-f", "-m", "20", "-H", "Title: " + title,
                    "-H", "Tags: " + tags, "-H", "Priority: " + priority,
                    "--data-binary", body, url], 25)
    return rc == 0


def kapwa(headline, detail, tag, dry, done=None):
    """One item on #mesh: the headline is its name forever, so it is short
    and the detail goes in a note on it. With done=<id>, close that item
    instead of opening one: the recovery answers the alert where it was
    raised. The item's id (or True) once it is on the board, else None."""
    if dry:
        print("KAPWA #mesh %s%s\n  %s\n" % ("done %s: " % done if done else "", headline, detail))
        return True
    if not os.path.exists(KAPWA):
        return None
    if done:
        rc, _, _ = run([KAPWA, "done", done, headline + ". " + detail, "--tag", tag], 60)
        return done if rc == 0 else None
    rc, out, _ = run([KAPWA, "say", headline, "--t", "mesh", "--tag", tag], 60)
    if rc != 0:
        return None
    # kapwa ids are short hex; one with a digit in it, so a word like
    # "added" in the output is not mistaken for one.
    m = re.search(r"\b((?=[0-9a-f]*[0-9])[0-9a-f]{5,12})\b", plain(out))
    if not m:
        return True
    if detail:
        run([KAPWA, "say", m.group(1), detail, "--tag", tag], 60)
    return m.group(1)


def flush(pending, dry, tag):
    """Send what is waiting, oldest first. Returns (what did not go, what
    did). An alert is ntfy and kapwa both, each half retried on its own;
    a sent alert carries its kapwa item id in "kapwa_sent"."""
    left, sent = [], []
    for a in pending:
        if "at" in a and (now() - parse(a["at"])).total_seconds() > 86400:
            continue                     # a day late is news nobody can use
        if not a.get("ntfy_sent"):
            a["ntfy_sent"] = ntfy(a["title"], a["body"], dry, a.get("priority", "high"),
                                  a.get("tags", "rotating_light"))
        if not a.get("kapwa_sent"):
            a["kapwa_sent"] = kapwa(a["title"], a["body"], tag, dry, a.get("kapwa_done"))
        (sent if a["ntfy_sent"] and a["kapwa_sent"] else left).append(a)
    return left, sent


def load_json(path, default):
    try:
        return json.load(open(path))
    except (OSError, ValueError):
        return default


def save_json(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    tmp = path + ".tmp"
    with open(tmp, "w") as f:
        json.dump(data, f, indent=1, sort_keys=True)
    os.replace(tmp, path)
