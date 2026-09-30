#!/usr/bin/python3
"""The mesh, seen from outside: does every public hostname answer, and does
every machine answer over the tailnet and over ssh? Reports; fixes nothing.

    mesh-watch.py                 check everything once, alert on news
    mesh-watch.py --dry           the same, but print alerts instead of sending
    mesh-watch.py --list          what it would check, and what it skips
    mesh-watch.py heartbeat <m>   is the monitor on machine <m> still running?

The per-box tunnel-watchdogs ask launchd whether a job is loaded. That is the
inside view: a tunnel can be loaded, running, and serving 530s. This asks the
question a visitor asks, from one box, every five minutes.

What it checks is derived, not listed:
  - every route in the fleet file (`tunnels fleet show --json`), fetched as
    https://<host>/ with redirects followed. ssh-* routes are skipped here;
    the machine checks below go through them for real.
  - every machine in mesh/machines: `tailscale ping`, `ssh <name> true`, and
    `ssh cloudflare-<name> true` when it has a tunnel. Not the agent's :7630,
    which mac2019 and doug-mini's firewalls do not answer.
mesh/watch.conf adds what the fleet cannot know: hosts to skip and why, sites
outside the fleet, and text a page must contain.

A check that fails is retried within the run. An incident opens after
FAIL_RUNS failing runs in a row and is alerted once; its recovery is alerted
once. Everything that opens in one run goes out as one message. If the
monitor cannot reach the internet at all, the run judges nothing: a dead
uplink here is not forty dead sites there.

History, for an uptime board later, under $MESH_WATCH_STATE
(default ~/.local/state/mesh-watch):
  checks-YYYY-MM.jsonl   one line per check per run
  incidents.jsonl        one line per opened / alerted / recovered incident
  state.json             open incidents and streaks
  last-run               the heartbeat: UTC time of the last finished run

Alerts go to $MESH_WATCH_NTFY (a full ntfy URL, topic included), read from
~/.config/mesh-watch/env. The topic is the secret: it never goes in git. With
none set, alerts are logged as unsent and nothing leaves the box.
"""
import concurrent.futures as cf
import datetime as dt
import json
import os
import re
import subprocess
import sys
import time

HOME = os.path.expanduser("~")
HERE = os.path.dirname(os.path.realpath(__file__))
REPO = os.path.dirname(HERE)
STATE = os.environ.get("MESH_WATCH_STATE", os.path.join(HOME, ".local/state/mesh-watch"))
ENVFILE = os.environ.get("MESH_WATCH_ENV", os.path.join(HOME, ".config/mesh-watch/env"))
CONF = os.environ.get("MESH_WATCH_CONF", os.path.join(REPO, "mesh/watch.conf"))
MACHINES = os.environ.get("MESH_WATCH_MACHINES", os.path.join(REPO, "mesh/machines"))
FAIL_RUNS = int(os.environ.get("MESH_WATCH_FAIL_RUNS", "2"))
RETRIES = int(os.environ.get("MESH_WATCH_RETRIES", "2"))
RETRY_GAP = float(os.environ.get("MESH_WATCH_RETRY_GAP", "15"))
STALE_MIN = int(os.environ.get("MESH_WATCH_STALE_MIN", "15"))
# Somewhere that is up whenever the internet is. If it does not answer, the
# problem is this box's uplink and the run judges nothing.
CANARY = os.environ.get("MESH_WATCH_CANARY", "https://www.cloudflare.com/cdn-cgi/trace")
PATH = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
os.environ["PATH"] = os.environ.get("PATH", "") + ":" + PATH  # launchd gives /usr/bin:/bin only

# Cloudflare's own error pages: the edge answered, the origin did not.
CF_ERROR = re.compile(r"cf-error-details|Error 10(16|33)|cloudflare-nginx|<title>[^<]*\| Cloudflare</title>", re.I)


def now():
    return dt.datetime.now(dt.timezone.utc)


def stamp(t=None):
    return (t or now()).strftime("%Y-%m-%dT%H:%M:%SZ")


def load_env():
    if os.path.exists(ENVFILE):
        for line in open(ENVFILE):
            line = line.strip()
            if line and not line.startswith("#") and "=" in line:
                k, v = line.split("=", 1)
                os.environ.setdefault(k.strip(), v.strip().strip("'\""))


def run(argv, timeout):
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout,
                           stdin=subprocess.DEVNULL)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 124, "", "timed out after %ss" % timeout
    except OSError as e:
        return 127, "", str(e)


# ---- what to check ---------------------------------------------------------

def read_conf():
    """skip <name> <why…> · extra <url> <why…> · expect <host> <text…>"""
    conf = {"skip": {}, "extra": [], "expect": {}}
    if not os.path.exists(CONF):
        return conf
    for line in open(CONF):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        verb, _, rest = line.partition(" ")
        key, _, val = rest.strip().partition(" ")
        if verb == "skip":
            conf["skip"][key] = val.strip() or "skipped"
        elif verb == "extra":
            conf["extra"].append((key, val.strip()))
        elif verb == "expect":
            conf["expect"][key] = val.strip()
    return conf


def fleet_routes():
    fleet = os.environ.get("MESH_WATCH_FLEET")
    if fleet:
        data = json.load(open(fleet))
    else:
        rc, out, err = run(["tunnels", "fleet", "show", "--json"], 30)
        if rc != 0:
            raise SystemExit("mesh-watch: cannot read the fleet: " + (err or out).strip())
        data = json.loads(out)
    return data.get("routes", [])


def read_machines():
    ms = []
    if not os.path.exists(MACHINES):
        return ms
    for line in open(MACHINES):
        if line.lstrip().startswith("#") or not line.strip():
            continue
        f = line.split()
        ms.append({"name": f[0], "host": f[1], "tunnel": f[4]})
    return ms


def targets(conf, machines=True):
    """Every check, and every skip with its reason."""
    out, skipped = [], []
    for r in fleet_routes():
        h = r["host"]
        if h in conf["skip"]:
            skipped.append((h, conf["skip"][h]))
        elif r.get("service", "").startswith("ssh://"):
            skipped.append((h, "ssh route: checked by `ssh cloudflare-<machine>`"))
        else:
            out.append({"id": "http:" + h, "kind": "http", "url": "https://%s/" % h,
                        "where": r.get("tunnel", ""), "expect": conf["expect"].get(h)})
    for url, why in conf["extra"]:
        h = re.sub(r"^https?://([^/]+).*", r"\1", url)
        # Named by host and path, so two pages on one host are two checks.
        out.append({"id": "http:" + re.sub(r"^https?://", "", url).rstrip("/"), "kind": "http", "url": url, "where": why or "outside the fleet",
                    "expect": conf["expect"].get(h)})
    if machines:
        for m in read_machines():
            if m["name"] in conf["skip"]:
                skipped.append((m["name"], conf["skip"][m["name"]]))
                continue
            out.append({"id": "tailnet:" + m["name"], "kind": "tailnet", "host": m["host"], "where": m["name"]})
            out.append({"id": "ssh:" + m["name"], "kind": "ssh", "alias": m["name"], "where": m["name"]})
            if m["tunnel"] != "-":
                out.append({"id": "ssh-cf:" + m["name"], "kind": "ssh", "alias": "cloudflare-" + m["name"],
                            "where": m["tunnel"]})
    return out, skipped


# ---- one check -------------------------------------------------------------

def check_http(t):
    rc, out, err = run(["curl", "-sS", "-L", "--max-redirs", "8", "-m", "20", "-A", "mesh-watch/1",
                        "-w", "\n%{http_code} %{url_effective}", t["url"]], 25)
    if rc != 0:
        return False, None, (err.strip().splitlines() or ["curl %d" % rc])[-1][:200]
    body, _, tail = out.rpartition("\n")
    code, _, final = tail.partition(" ")
    code = int(code or 0)
    if not 200 <= code < 300:
        return False, code, "HTTP %d at %s" % (code, final)
    if not body.strip():
        return False, code, "empty page at %s" % final
    if CF_ERROR.search(body):
        return False, code, "Cloudflare error page at %s" % final
    if t.get("expect") and t["expect"] not in body:
        return False, code, "page lacks %r" % t["expect"]
    return True, code, final if final != t["url"] else ""


def check_tailnet(t):
    rc, out, err = run(["tailscale", "ping", "-c", "3", "--timeout", "5s", t["host"]], 25)
    msg = (out + err).strip().splitlines()
    return rc == 0, None, (msg[-1] if msg else "")[:200]


def check_ssh(t):
    rc, out, err = run(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=12",
                        "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=2",
                        t["alias"], "true"], 30)
    return rc == 0, None, "" if rc == 0 else ((err.strip().splitlines() or ["ssh %d" % rc])[-1])[:200]


KINDS = {"http": check_http, "tailnet": check_tailnet, "ssh": check_ssh}


def check(t):
    t0 = time.time()
    ok, code, detail = KINDS[t["kind"]](t)
    return {"id": t["id"], "ok": ok, "code": code, "ms": int((time.time() - t0) * 1000), "detail": detail}


def check_all(ts, retries=RETRIES, gap=RETRY_GAP):
    results = {}
    todo = ts
    for attempt in range(retries + 1):
        if attempt:
            time.sleep(gap)
        with cf.ThreadPoolExecutor(8) as pool:
            for t, r in zip(todo, pool.map(check, todo)):
                r["tries"] = attempt + 1
                results[t["id"]] = r
        todo = [t for t in todo if not results[t["id"]]["ok"]]
        if not todo:
            break
    return results


# ---- state, history, alerts -------------------------------------------------

def load_state(name="state.json"):
    try:
        return json.load(open(os.path.join(STATE, name)))
    except (OSError, ValueError):
        return {"streak": {}, "open": {}, "pending": []}


def save_json(obj, name):
    p = os.path.join(STATE, name)
    tmp = p + ".tmp"
    with open(tmp, "w") as f:
        json.dump(obj, f, indent=1, sort_keys=True)
    os.replace(tmp, p)


def append(name, rows):
    with open(os.path.join(STATE, name), "a") as f:
        for r in rows:
            f.write(json.dumps(r, sort_keys=True) + "\n")


def send(title, body, dry, tags="rotating_light"):
    """Send one alert. True if it left the box (or was printed, when dry)."""
    if dry:
        print("ALERT %s\n%s\n" % (title, body))
        return True
    url = os.environ.get("MESH_WATCH_NTFY")
    if not url:
        return False
    rc, out, err = run(["curl", "-sS", "-f", "-m", "20", "-H", "Title: " + title, "-H", "Tags: " + tags,
                        "-H", "Priority: high", "--data-binary", body, url], 25)
    return rc == 0


def flush(state, dry):
    """Send what is waiting, oldest first; keep whatever did not go."""
    left = []
    for a in state["pending"]:
        if left or not send(a["title"], a["body"], dry, a.get("tags", "rotating_light")):
            left.append(a)
    state["pending"] = left


def judge(state, ts, results, t):
    """Fold one run into the state. Returns (opened, recovered) incident ids."""
    by_id = {x["id"]: x for x in ts}
    opened, recovered = [], []
    rows = []
    for tid, r in results.items():
        if r["ok"]:
            state["streak"].pop(tid, None)
            inc = state["open"].pop(tid, None)
            if inc:
                inc["recovered"] = stamp(t)
                rows.append({"ts": stamp(t), "event": "recovered", "id": tid, "since": inc["since"],
                             "alerted": inc["alerted"]})
                if inc["alerted"]:
                    recovered.append((tid, inc))
            continue
        s = state["streak"].setdefault(tid, {"since": stamp(t), "runs": 0})
        s["runs"] += 1
        s["detail"] = r["detail"]
        if tid not in state["open"] and s["runs"] >= FAIL_RUNS:
            state["open"][tid] = {"since": s["since"], "alerted": True, "detail": r["detail"],
                                  "where": by_id.get(tid, {}).get("where", "")}
            rows.append({"ts": stamp(t), "event": "opened", "id": tid, "since": s["since"], "detail": r["detail"]})
            opened.append(tid)
    # A check that is no longer derived (route removed, host skipped) closes quietly.
    for tid in list(state["open"]):
        if tid not in by_id:
            state["open"].pop(tid)
            rows.append({"ts": stamp(t), "event": "retired", "id": tid})
    for tid in list(state["streak"]):
        if tid not in by_id:
            state["streak"].pop(tid)
    append("incidents.jsonl", rows)
    return opened, recovered


def ago(since, t):
    mins = int((t - dt.datetime.strptime(since, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)).total_seconds() // 60)
    return "%dm" % mins if mins < 120 else "%dh%02dm" % (mins // 60, mins % 60)


def compose(state, opened, recovered, t, box):
    msgs = []
    if opened:
        lines = ["%s — %s (%s)" % (tid, state["open"][tid]["detail"] or "failed", state["open"][tid]["where"])
                 for tid in opened]
        still = len(state["open"]) - len(opened)
        if still:
            lines.append("(%d other incident%s still open)" % (still, "" if still == 1 else "s"))
        title = "mesh: %s down" % (opened[0] if len(opened) == 1 else "%d checks" % len(opened))
        msgs.append({"title": title, "body": "\n".join(lines) + "\n— mesh-watch on " + box, "tags": "rotating_light"})
    if recovered:
        lines = ["%s — back after %s" % (tid, ago(inc["since"], t)) for tid, inc in recovered]
        if state["open"]:
            lines.append("(%d still down)" % len(state["open"]))
        title = "mesh: %s back" % (recovered[0][0] if len(recovered) == 1 else "%d checks" % len(recovered))
        msgs.append({"title": title, "body": "\n".join(lines) + "\n— mesh-watch on " + box, "tags": "white_check_mark"})
    return msgs


# ---- modes -----------------------------------------------------------------

def watch(dry, machines=True):
    os.makedirs(STATE, exist_ok=True)
    conf = read_conf()
    ts, _ = targets(conf, machines)
    box = os.uname().nodename.split(".")[0]
    state = load_state()
    t = now()
    rc, _, _ = run(["curl", "-sS", "-f", "-o", "/dev/null", "-m", "15", CANARY], 20)
    if rc != 0:
        append("checks-%s.jsonl" % t.strftime("%Y-%m"),
               [{"ts": stamp(t), "id": "canary", "ok": False, "detail": "this box is offline; nothing judged"}])
        print("mesh-watch: the canary did not answer; this box looks offline, judging nothing", file=sys.stderr)
        return 3
    results = check_all(ts)
    append("checks-%s.jsonl" % t.strftime("%Y-%m"),
           [dict(r, ts=stamp(t)) for r in results.values()])
    opened, recovered = judge(state, ts, results, t)
    state["pending"].extend(compose(state, opened, recovered, t, box))
    flush(state, dry)
    state["last_run"] = stamp()
    save_json(state, "state.json")
    with open(os.path.join(STATE, "last-run"), "w") as f:
        f.write(stamp() + "\n")
    bad = [r for r in results.values() if not r["ok"]]
    print("mesh-watch: %d checks, %d failing, %d open, %d alert(s) waiting" %
          (len(results), len(bad), len(state["open"]), len(state["pending"])))
    for r in sorted(bad, key=lambda r: r["id"]):
        print("  FAIL %-40s %s" % (r["id"], r["detail"]))
    return 0


def heartbeat(machine, dry):
    """From a second box: has the monitor on <machine> finished a run lately?"""
    os.makedirs(STATE, exist_ok=True)
    state = load_state("heartbeat.json")
    t = now()
    path = os.environ.get("MESH_WATCH_REMOTE_STATE", ".local/state/mesh-watch") + "/last-run"
    last, how, reached = None, [], False
    # Tailnet first, then the Cloudflare path, so one path down is not "the
    # monitor is down". ssh exits 255 only when it could not get in.
    for alias in (machine, "cloudflare-" + machine):
        for attempt in range(RETRIES + 1):
            if attempt:
                time.sleep(RETRY_GAP)
            rc, out, err = run(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=12", alias,
                                "cat " + path], 30)
            if rc != 255:
                break
        reached = rc != 255
        if rc == 0:
            last = out.strip()
        how.append("%s: %s" % (alias, "ok" if rc == 0 else (err.strip().splitlines() or ["?"])[-1][:120]))
        if reached:
            break
    ok, detail = False, ""
    if not reached:
        detail = "cannot reach %s (%s)" % (machine, "; ".join(how))
    elif last is None:
        detail = "%s answers but has no heartbeat (%s)" % (machine, how[-1])
    else:
        try:
            age = (t - dt.datetime.strptime(last, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.timezone.utc)).total_seconds()
            ok = age < STALE_MIN * 60
            detail = "last run %s ago" % ago(last, t)
        except ValueError:
            detail = "last-run unreadable: %r" % last[:40]
    tid = "monitor:" + machine
    append("heartbeat-%s.jsonl" % t.strftime("%Y-%m"), [{"ts": stamp(t), "id": tid, "ok": ok, "detail": detail}])
    fake = [{"id": tid, "where": "mesh-watch on " + machine}]
    opened, recovered = judge(state, fake, {tid: {"ok": ok, "detail": detail}}, t)
    msgs = []
    if opened:
        msgs.append({"title": "mesh-watch on %s has stopped" % machine,
                     "body": "%s. Nothing is watching the mesh until it is back.\n— heartbeat on %s"
                             % (detail, os.uname().nodename.split(".")[0]), "tags": "warning"})
    if recovered:
        msgs.append({"title": "mesh-watch on %s is back" % machine,
                     "body": "%s; it was out for %s." % (detail, ago(recovered[0][1]["since"], t)),
                     "tags": "white_check_mark"})
    state["pending"].extend(msgs)
    flush(state, dry)
    save_json(state, "heartbeat.json")
    print("heartbeat %s: %s — %s" % (machine, "ok" if ok else "FAIL", detail))
    return 0


def listing():
    conf = read_conf()
    ts, skipped = targets(conf)
    for t in ts:
        print("check  %-44s %s%s" % (t["id"], t.get("url") or t.get("alias") or t.get("host"),
                                     "  expect %r" % t["expect"] if t.get("expect") else ""))
    for h, why in skipped:
        print("skip   %-44s %s" % (h, why))
    print("%d checks, %d skipped" % (len(ts), len(skipped)))
    return 0


def main(argv):
    load_env()
    dry = "--dry" in argv
    args = [a for a in argv if not a.startswith("--")]
    if "--list" in argv:
        return listing()
    if args and args[0] == "heartbeat":
        if len(args) != 2:
            raise SystemExit("usage: mesh-watch.py heartbeat <machine>")
        return heartbeat(args[1], dry)
    if args:
        raise SystemExit(__doc__)
    return watch(dry, machines="--no-machines" not in argv)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
