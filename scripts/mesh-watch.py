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
    the machine checks below go through them for real. If the fleet cannot
    be read, the last copy that could is used, and that is itself a finding.
  - every machine in mesh/machines: `tailscale ping`, `ssh <name> true`, and
    `ssh cloudflare-<name> true` when it has a tunnel. Not the agent's :7630,
    which mac2019 and doug-mini's firewalls do not answer.
  - with MESH_WATCH_PEER=<machine>, that the heartbeat there is running: the
    two watchers watch each other.
mesh/watch.conf adds what the fleet cannot know: hosts to skip and why, sites
outside the fleet, and text a page must contain.

A check that fails is retried within the run. An incident opens after
FAIL_RUNS failing runs in a row and is alerted once; it closes after OK_RUNS
passing runs in a row, alerted once, so a flaky host does not flap. What
opens in one run goes out as one message, grouped by the machine that serves
it. If the monitor cannot reach the internet at all, the run judges nothing:
a dead uplink here is not forty dead sites there. It says so in its
heartbeat, and the heartbeat says so to Felix.

History, for an uptime board later, under $MESH_WATCH_STATE
(default ~/.local/state/mesh-watch):
  checks-YYYY-MM.jsonl   one line per check per run; KEEP_MONTHS are kept
  incidents.jsonl        one line per opened / recovered / retired incident
  state.json             open incidents and streaks
  fleet.json             the last fleet that could be read
  last-run               the heartbeat: "<UTC time> ok" or "<time> offline <since>"

Alerts go to $MESH_WATCH_NTFY (a full ntfy URL, topic included), read from
~/.config/mesh-watch/env. The topic is the secret: it never goes in git. With
none set, alerts are logged as unsent and nothing leaves the box.
"""
import concurrent.futures as cf
import datetime as dt
import glob
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
REMOTE_STATE = os.environ.get("MESH_WATCH_REMOTE_STATE", ".local/state/mesh-watch")
FAIL_RUNS = int(os.environ.get("MESH_WATCH_FAIL_RUNS", "2"))
OK_RUNS = int(os.environ.get("MESH_WATCH_OK_RUNS", "2"))
RETRIES = int(os.environ.get("MESH_WATCH_RETRIES", "2"))
RETRY_GAP = float(os.environ.get("MESH_WATCH_RETRY_GAP", "15"))
# Both jobs run every 5 minutes, so 11 minutes is two missed runs.
STALE_MIN = int(os.environ.get("MESH_WATCH_STALE_MIN", "11"))
KEEP_MONTHS = int(os.environ.get("MESH_WATCH_KEEP_MONTHS", "6"))
# How often to ask Cloudflare what it really serves; 0 turns it off.
DRIFT_MIN = int(os.environ.get("MESH_WATCH_DRIFT_MIN", "60"))
# Somewhere that is up whenever the internet is. If it does not answer, the
# problem is this box's uplink and the run judges nothing.
CANARY = os.environ.get("MESH_WATCH_CANARY", "https://www.cloudflare.com/cdn-cgi/trace")
PATH = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
os.environ["PATH"] = os.environ.get("PATH", "") + ":" + PATH  # launchd gives /usr/bin:/bin only

# Cloudflare's own error pages: the edge answered, the origin did not.
CF_ERROR = re.compile(r"cf-error-details|Error 10(16|33)|cloudflare-nginx|<title>[^<]*\| Cloudflare</title>", re.I)

# A page that answers 200 but is an error or a placeholder: a proxy with no
# app behind it, a web server's default page, an app's crash screen. Judged
# on the <title>, and on the whole text only when the page is tiny, because a
# real page can mention "not found" anywhere in its scripts.
BROKEN = (r"^\s*(\d{3}\b|bad gateway|internal server error|service unavailable|gateway time-?out|"
          r"application error|welcome to nginx|it works!?|index of /|test page|page not found|"
          r"not found|no healthy upstream|upstream connect error|cannot (get|post) /|error\b)")
BROKEN_TITLE = re.compile(r"<title>" + BROKEN, re.I)
BROKEN_TINY = re.compile(BROKEN, re.I)


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


def run(argv, timeout):
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=timeout,
                           stdin=subprocess.DEVNULL)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return 124, "", "timed out after %ss" % timeout
    except OSError as e:
        return 127, "", str(e)


def last_line(s, default):
    lines = s.strip().splitlines()
    return (lines[-1] if lines else default)[:200]


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


def read_fleet():
    """The fleet, and None; or the last copy that could be read, and why."""
    cache = os.path.join(STATE, "fleet.json")
    try:
        src = os.environ.get("MESH_WATCH_FLEET")
        if src:
            data = json.load(open(src))
        else:
            rc, out, err = run(["tunnels", "fleet", "show", "--json"], 30)
            if rc != 0:
                raise ValueError(last_line(err or out, "tunnels exited %d" % rc))
            data = json.loads(out)
        if not data.get("routes"):
            raise ValueError("the fleet has no routes")
    except (OSError, ValueError) as e:
        try:
            data = json.load(open(cache))
        except (OSError, ValueError):
            raise SystemExit("mesh-watch: cannot read the fleet (%s), and there is no earlier copy" % e)
        return data, "cannot read the fleet (%s); checking the copy of %s" % (e, data.get("updated_at", "?"))
    try:
        save_json(data, "fleet.json")
    except OSError:
        pass
    return data, None


def cf_get(path):
    rc, out, err = run(["tunnels", "cf", "get", "--json", path], 30)
    if rc != 0:
        raise ValueError("tunnels cf get %s: %s" % (path.split("?")[0], last_line(err or out, str(rc))))
    r = json.loads(out).get("response") or {}
    if not r.get("success", True):
        raise ValueError("Cloudflare refused %s: %s" % (path.split("?")[0], r.get("errors")))
    return r["result"]


def live_routes(fleet):
    """Every hostname Cloudflare sends to a tunnel, by ingress rule or by
    tunnel CNAME, whether or not the fleet file knows it: {host: tunnel}."""
    src = os.environ.get("MESH_WATCH_LIVE")
    if src:
        return json.load(open(src))
    accounts = {a: v["id"] for a, v in fleet.get("accounts", {}).items()}
    alias = {v["id"]: k for k, v in fleet.get("tunnels", {}).items()}
    live = {}
    for a, aid in accounts.items():
        for t in cf_get("/accounts/%s/cfd_tunnel?is_deleted=false&per_page=100" % aid):
            conf = cf_get("/accounts/%s/cfd_tunnel/%s/configurations" % (aid, t["id"])) or {}
            for r in (conf.get("config") or {}).get("ingress") or []:
                if r.get("hostname"):
                    live[r["hostname"]] = alias.get(t["id"], "unlisted tunnel " + t.get("name", t["id"][:8]))
    for a, v in fleet.get("accounts", {}).items():
        for z in v.get("zones", []):
            for r in cf_get("/zones/{zone:%s}/dns_records?type=CNAME&per_page=500" % z):
                if r.get("content", "").endswith(".cfargotunnel.com"):
                    tid = r["content"].split(".")[0]
                    live.setdefault(r["name"], alias.get(tid, "unlisted tunnel " + tid[:8]))
    return live


def drift(fleet, t):
    """What Cloudflare serves that the fleet does not say, and the reverse.
    Asked every DRIFT_MIN minutes; a difference counts only when two fetches
    in a row see it, so a `route add` caught halfway is not drift. Returns
    ({host: tunnel} live but not in the fleet, finding or None, error or None)."""
    if DRIFT_MIN <= 0:
        return {}, None, None
    try:
        cache = json.load(open(os.path.join(STATE, "drift.json")))
    except (OSError, ValueError):
        cache = {}
    err = None
    if not cache.get("fetched") or (t - parse(cache["fetched"])).total_seconds() >= DRIFT_MIN * 60 - 30:
        try:
            live = live_routes(fleet)
            cache = {"fetched": stamp(t), "live": live, "prev": cache.get("live")}
        except (OSError, ValueError, KeyError, TypeError) as e:
            # Unknown is not missing: a failed fetch reports nothing.
            err = str(e)[:200]
            cache["error"] = err
        save_json(cache, "drift.json")
    fleet_hosts = {r["host"] for r in fleet.get("routes", [])}
    live, prev = cache.get("live") or {}, cache.get("prev")
    unlisted = {h: v for h, v in live.items() if h not in fleet_hosts}
    if prev is None:
        return unlisted, None, err
    extra = sorted(h for h in unlisted if h in prev)
    gone = sorted(h for h in fleet_hosts if h not in live and h not in prev)
    parts = []
    if extra:
        parts.append("live but not in the fleet file: " + ", ".join("%s (%s)" % (h, live[h]) for h in extra))
    if gone:
        parts.append("in the fleet file but not in Cloudflare: " + ", ".join(gone))
    return unlisted, "; ".join(parts) or None, err


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


def targets(conf, fleet, machines=True, unlisted=None):
    """Every check, and every skip with its reason."""
    out, skipped = [], []
    tunnels = fleet.get("tunnels", {})
    # Hosts Cloudflare serves that the fleet does not know are checked too:
    # the list comes from what is live, not only from what was written down.
    for h, tun in sorted((unlisted or {}).items()):
        if h in conf["skip"] or h.startswith("ssh-"):
            continue
        m = tunnels.get(tun, {}).get("machine", "?")
        out.append({"id": "http:" + h, "kind": "http", "url": "https://%s/" % h, "label": h, "machine": m,
                    "where": "live on %s but not in the fleet file" % tun, "expect": conf["expect"].get(h)})
    for r in fleet.get("routes", []):
        h = r["host"]
        if h in conf["skip"]:
            skipped.append((h, conf["skip"][h]))
        elif r.get("service", "").startswith("ssh://"):
            skipped.append((h, "ssh route: checked by `ssh cloudflare-<machine>`"))
        else:
            m = tunnels.get(r.get("tunnel"), {}).get("machine", "?")
            where = "served by %s (tunnel %s)" % (m, r.get("tunnel"))
            if r.get("standby"):
                where += "; standby %s" % tunnels.get(r["standby"], {}).get("machine", r["standby"])
            out.append({"id": "http:" + h, "kind": "http", "url": "https://%s/" % h, "label": h,
                        "machine": m, "where": where, "expect": conf["expect"].get(h)})
    for url, why in conf["extra"]:
        h = re.sub(r"^https?://([^/]+).*", r"\1", url)
        name = re.sub(r"^https?://", "", url).rstrip("/")
        # Named by host and path, so two pages on one host are two checks.
        out.append({"id": "http:" + name, "kind": "http", "url": url, "label": name, "machine": None,
                    "where": why or "outside the fleet", "expect": conf["expect"].get(h)})
    if machines:
        for m in read_machines():
            n = m["name"]
            if n in conf["skip"]:
                skipped.append((n, conf["skip"][n]))
                continue
            out.append({"id": "tailnet:" + n, "kind": "tailnet", "host": m["host"], "machine": n,
                        "label": "%s: tailscale ping" % n, "where": "tailnet name " + m["host"]})
            out.append({"id": "ssh:" + n, "kind": "ssh", "alias": n, "machine": n,
                        "label": "%s: ssh over the tailnet" % n, "where": "ssh " + n})
            if m["tunnel"] != "-":
                out.append({"id": "ssh-cf:" + n, "kind": "ssh", "alias": "cloudflare-" + n, "machine": n,
                            "label": "%s: ssh through Cloudflare" % n, "where": m["tunnel"]})
    peer = os.environ.get("MESH_WATCH_PEER")
    if peer and machines:
        out.append({"id": "heartbeat:" + peer, "kind": "peer", "alias": peer, "machine": peer,
                    "label": "%s: the heartbeat that watches this monitor" % peer,
                    "where": "com.dorkyrobot.mesh-watch-heartbeat on " + peer})
    return out, skipped


# ---- one check -------------------------------------------------------------

# Sign-in lives at id.<domain> (Pocket ID), which answers 429 to one address
# polling it every few seconds. Following every app's sign-in redirect would
# hit it several times a run at once, so a redirect there ends the check: the
# app answered, and the id host has a check of its own.
IDP = re.compile(r"^https?://id(-dev)?\.[^/]+/", re.I)


def fetch(url):
    """(rc, code, location, body, err) for one request, redirects not followed."""
    rc, out, err = run(["curl", "-sS", "-m", "20", "-A", "mesh-watch/1",
                        "-w", "\n%{http_code} %{redirect_url}", url], 25)
    if rc != 0:
        return rc, None, "", "", last_line(err, "curl %d" % rc)
    body, _, tail = out.rpartition("\n")
    code, _, loc = tail.partition(" ")
    return 0, int(code or 0), loc.strip(), body, ""


def check_http(t):
    url = t["url"]
    for _ in range(8):
        rc, code, loc, body, err = fetch(url)
        if rc != 0:
            return False, None, err
        if 300 <= code < 400 and loc:
            if IDP.match(loc) and not IDP.match(t["url"]):
                return True, code, "sign-in redirect to " + loc.split("?")[0]
            url = loc
            continue
        break
    else:
        return False, code, "more than 8 redirects"
    at = "" if url == t["url"] else " at " + url.split("?")[0]
    if code == 429:
        return True, code, "rate-limited (429), so it is up" + at
    if not 200 <= code < 300:
        return False, code, "HTTP %d%s" % (code, at)
    if not body.strip():
        return False, code, "empty page" + at
    if CF_ERROR.search(body):
        return False, code, "Cloudflare error page" + at
    m = BROKEN_TITLE.search(body) or (len(body) < 1000 and BROKEN_TINY.search(re.sub(r"<[^>]*>", " ", body)))
    if m:
        return False, code, "error page (%r)%s" % (m.group(0).replace("<title>", "").strip()[:60], at)
    if t.get("expect") and t["expect"] not in body:
        return False, code, "page lacks %r%s" % (t["expect"], at)
    return True, code, at.strip()


def check_tailnet(t):
    rc, out, err = run(["tailscale", "ping", "-c", "3", "--timeout", "5s", t["host"]], 25)
    return rc == 0, None, last_line(out + err, "")


def check_ssh(t):
    rc, out, err = run(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=12",
                        "-o", "ServerAliveInterval=5", "-o", "ServerAliveCountMax=2",
                        t["alias"], "true"], 30)
    return rc == 0, None, "" if rc == 0 else last_line(err, "ssh %d" % rc)


def check_peer(t):
    rc, out, err = run(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=12", t["alias"],
                        "cat %s/heartbeat-last" % REMOTE_STATE], 30)
    if rc == 255:
        # The machine checks already say it is unreachable; this is about the job.
        return True, None, "unreachable; see the machine checks"
    if rc != 0:
        return False, None, "no heartbeat has run there (%s)" % last_line(err, "?")
    try:
        age = (now() - parse(out.split()[0])).total_seconds()
    except (ValueError, IndexError):
        return False, None, "heartbeat-last unreadable"
    if age > STALE_MIN * 60:
        return False, None, "the heartbeat last ran %s ago" % ago(out.split()[0], now())
    return True, None, ""


KINDS = {"http": check_http, "tailnet": check_tailnet, "ssh": check_ssh, "peer": check_peer}


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
        s = json.load(open(os.path.join(STATE, name)))
        if not isinstance(s, dict):
            raise ValueError
    except (OSError, ValueError):
        s = {}
    for k, v in (("streak", {}), ("open", {}), ("pending", [])):
        s.setdefault(k, v)
    return s


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


def prune(t):
    """Per-check history is kept KEEP_MONTHS; incidents.jsonl is kept for good."""
    keep = {(t.year * 12 + t.month - 1 - i) for i in range(KEEP_MONTHS)}
    for p in glob.glob(os.path.join(STATE, "*-[0-9][0-9][0-9][0-9]-[0-9][0-9].jsonl")):
        m = re.search(r"-(\d{4})-(\d{2})\.jsonl$", p)
        if m and int(m.group(1)) * 12 + int(m.group(2)) - 1 not in keep:
            os.remove(p)


def send(msg, dry):
    """Send one alert. True if it left the box (or was printed, when dry)."""
    if dry:
        print("ALERT %s\n%s\n" % (msg["title"], msg["body"]))
        return True
    url = os.environ.get("MESH_WATCH_NTFY")
    if not url:
        return False
    rc, out, err = run(["curl", "-sS", "-f", "-m", "20", "-H", "Title: " + msg["title"],
                        "-H", "Tags: " + msg.get("tags", "rotating_light"),
                        "-H", "Priority: " + msg.get("priority", "high"),
                        "--data-binary", msg["body"], url], 25)
    return rc == 0


def flush(state, dry):
    """Send what is waiting, oldest first; keep whatever did not go."""
    left = []
    for a in state["pending"]:
        if left or not send(a, dry):
            left.append(a)
    state["pending"] = left


def judge(state, ts, results, t, fail_runs=FAIL_RUNS, ok_runs=OK_RUNS):
    """Fold one run into the state. Returns (opened, recovered) incident ids."""
    by_id = {x["id"]: x for x in ts}
    opened, recovered, rows = [], [], []
    for tid, r in results.items():
        inc = state["open"].get(tid)
        if r["ok"]:
            state["streak"].pop(tid, None)
            if inc:
                inc["oks"] = inc.get("oks", 0) + 1
                if inc["oks"] >= ok_runs:
                    del state["open"][tid]
                    rows.append({"ts": stamp(t), "event": "recovered", "id": tid, "since": inc["since"]})
                    recovered.append((tid, inc))
            continue
        if inc:
            inc["oks"] = 0
            inc["detail"] = r["detail"]
            continue
        s = state["streak"].setdefault(tid, {"since": stamp(t), "runs": 0})
        s["runs"] += 1
        s["detail"] = r["detail"]
        need = r.get("fail_runs", fail_runs)
        if s["runs"] >= need:
            x = by_id.get(tid, {})
            state["open"][tid] = {"since": s["since"], "detail": r["detail"], "oks": 0,
                                  "label": x.get("label", tid), "where": x.get("where", ""),
                                  "machine": x.get("machine"), "priority": x.get("priority", "high")}
            del state["streak"][tid]
            rows.append({"ts": stamp(t), "event": "opened", "id": tid, "since": s["since"], "detail": r["detail"]})
            opened.append(tid)
    # A check that is no longer derived (route removed, host skipped) closes quietly.
    for tid in list(state["open"]):
        if tid not in by_id:
            del state["open"][tid]
            rows.append({"ts": stamp(t), "event": "retired", "id": tid})
    for tid in list(state["streak"]):
        if tid not in by_id:
            del state["streak"][tid]
    append("incidents.jsonl", rows)
    return opened, recovered


def ago(since, t):
    mins = int((t - parse(since)).total_seconds() // 60)
    return "%dm" % mins if mins < 120 else "%dh%02dm" % (mins // 60, mins % 60)


def clock(s):
    """UTC stamp to this box's wall clock, which is Felix's."""
    return time.strftime("%H:%M", time.localtime(parse(s).timestamp()))


def ways_in(m):
    return "ssh %s · ssh cloudflare-%s · ssh %s-lan" % (m, m, m)


def compose(state, ts, opened, recovered, t, box):
    """One message for everything that opened, one for everything that closed."""
    msgs = []
    low = [i for i in opened if state["open"][i].get("priority") == "low"]
    opened = [i for i in opened if i not in low]
    for tid in low:
        inc = state["open"][tid]
        msgs.append({"title": "note: " + inc["label"], "body": "%s.\nNothing is down because of it; %s.\n— mesh-watch on %s"
                     % (inc["detail"][0].upper() + inc["detail"][1:], inc["where"], box),
                     "tags": "memo", "priority": "low"})
    per_machine = {}
    for x in ts:
        per_machine.setdefault(x.get("machine"), []).append(x["id"])
    if opened:
        groups = {}
        for tid in opened:
            groups.setdefault(state["open"][tid].get("machine"), []).append(tid)
        lines, heads = [], []
        for m in sorted(groups, key=lambda m: (m is None, m or "")):
            ids = groups[m]
            down_all = m and set(per_machine.get(m, [])) <= set(state["open"])
            if m is None:
                lines.append("Outside the tunnels:")
            elif down_all:
                lines.append("%s looks down: all %d of its checks fail." % (m, len(per_machine[m])))
                heads.append(m)
            else:
                lines.append("On %s:" % m)
                heads.extend(state["open"][i]["label"] for i in ids)
            for tid in ids:
                inc = state["open"][tid]
                lines.append("  %s — %s, since %s" % (inc["label"], inc["detail"] or "failed", clock(inc["since"])))
                if inc.get("where") and not inc["label"].startswith((m or "") + ":"):
                    lines.append("    %s" % inc["where"])
            if m:
                lines.append("  get in: " + ways_in(m))
        still = len(state["open"]) - len(opened)
        if still:
            lines.append("(%d earlier incident%s still open)" % (still, "" if still == 1 else "s"))
        title = "down: " + (heads[0] if len(heads) == 1 else ", ".join(heads[:3]) +
                            (" +%d" % (len(heads) - 3) if len(heads) > 3 else ""))
        if not heads:
            title = "down: " + ", ".join(state["open"][i]["label"] for i in opened)
        msgs.append({"title": title[:120], "body": "\n".join(lines) + "\n— mesh-watch on " + box,
                     "tags": "rotating_light", "priority": "high"})
    quiet = [r for r in recovered if r[1].get("priority") == "low"]
    recovered = [r for r in recovered if r not in quiet]
    for tid, inc in quiet:
        msgs.append({"title": "cleared: " + inc["label"], "body": "Open %s.\n— mesh-watch on %s" % (ago(inc["since"], t), box),
                     "tags": "white_check_mark", "priority": "low"})
    if recovered:
        lines = ["%s — back; down %s (%s–%s)" % (inc["label"], ago(inc["since"], t), clock(inc["since"]),
                                                 clock(stamp(t))) for tid, inc in recovered]
        if state["open"]:
            lines.append("(%d still down)" % len(state["open"]))
        title = "back: " + ", ".join(inc["label"] for _, inc in recovered)
        msgs.append({"title": title[:120], "body": "\n".join(lines) + "\n— mesh-watch on " + box,
                     "tags": "white_check_mark", "priority": "default"})
    return msgs


def write_heartbeat(name, text):
    with open(os.path.join(STATE, name), "w") as f:
        f.write(text + "\n")


# ---- modes -----------------------------------------------------------------

def watch(dry, machines=True):
    state = load_state()
    t = now()
    rc, _, err = run(["curl", "-sS", "-f", "-o", "/dev/null", "-m", "15", CANARY], 20)
    if rc != 0:
        state.setdefault("offline_since", stamp(t))
        append("checks-%s.jsonl" % t.strftime("%Y-%m"),
               [{"ts": stamp(t), "id": "canary", "ok": False, "detail": last_line(err, "canary %d" % rc)}])
        save_json(state, "state.json")
        write_heartbeat("last-run", "%s offline %s" % (stamp(), state["offline_since"]))
        print("mesh-watch: the canary did not answer; this box looks offline, judging nothing", file=sys.stderr)
        return 3
    state.pop("offline_since", None)
    fleet, fleet_note = read_fleet()
    unlisted, drift_note, drift_err = drift(fleet, t)
    ts, _ = targets(read_conf(), fleet, machines, unlisted)
    results = check_all(ts)
    ts.append({"id": "monitor:fleet", "label": "the fleet file on " + box(), "machine": None,
               "where": "`tunnels fleet show --json` on " + box()})
    results["monitor:fleet"] = {"id": "monitor:fleet", "ok": fleet_note is None, "detail": fleet_note or ""}
    if DRIFT_MIN > 0:
        ts.append({"id": "monitor:drift", "label": "the fleet file and Cloudflare disagree", "machine": None,
                   "priority": "low", "where": "`tunnels plan` shows the fix; hosts live in Cloudflare are checked anyway"})
        results["monitor:drift"] = {"id": "monitor:drift", "ok": drift_note is None,
                                    "detail": drift_note or drift_err or ""}
    append("checks-%s.jsonl" % t.strftime("%Y-%m"), [dict(r, ts=stamp(t)) for r in results.values()])
    opened, recovered = judge(state, ts, results, t)
    state["pending"].extend(compose(state, ts, opened, recovered, t, box()))
    flush(state, dry)
    state["last_run"] = stamp()
    save_json(state, "state.json")
    write_heartbeat("last-run", "%s ok" % stamp())
    prune(t)
    bad = [r for r in results.values() if not r["ok"]]
    print("mesh-watch: %d checks, %d failing, %d open, %d alert(s) waiting" %
          (len(results), len(bad), len(state["open"]), len(state["pending"])))
    for r in sorted(bad, key=lambda r: r["id"]):
        print("  FAIL %-40s %s" % (r["id"], r["detail"]))
    return 0


def heartbeat(machine, dry):
    """From a second box: has the monitor on <machine> finished a run lately?"""
    state = load_state("heartbeat.json")
    t = now()
    last, how, reached = None, [], False
    # Tailnet first, then the Cloudflare path, so one path down is not "the
    # monitor is down". ssh exits 255 only when it could not get in.
    for alias in (machine, "cloudflare-" + machine):
        for attempt in range(RETRIES + 1):
            if attempt:
                time.sleep(RETRY_GAP)
            rc, out, err = run(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=12", alias,
                                "cat %s/last-run" % REMOTE_STATE], 30)
            if rc != 255:
                break
        reached = rc != 255
        if rc == 0:
            last = out.strip()
        how.append("%s: %s" % (alias, "ok" if rc == 0 else last_line(err, "?")[:120]))
        if reached:
            break
    # Unreachable is judged over two runs, since the fault may be this box's;
    # a stale or missing heartbeat already means two missed runs, so at once.
    ok, detail, fail_runs = False, "", 1
    if not reached:
        detail, fail_runs = "cannot reach %s (%s)" % (machine, "; ".join(how)), 2
    elif last is None:
        detail = "%s answers, but mesh-watch has never finished a run there (%s)" % (machine, how[-1])
    else:
        f = last.split()
        try:
            age = (t - parse(f[0])).total_seconds()
            if age > STALE_MIN * 60:
                detail = "its last run was %s ago, at %s" % (ago(f[0], t), clock(f[0]))
            elif len(f) >= 3 and f[1] == "offline" and (t - parse(f[2])).total_seconds() > STALE_MIN * 60:
                detail = ("it is running, but %s has had no internet since %s, so it cannot see the "
                          "sites or send alerts" % (machine, clock(f[2])))
            else:
                ok, detail = True, "last run %s ago" % ago(f[0], t)
        except (ValueError, IndexError):
            detail = "last-run unreadable: %r" % last[:40]
    tid = "monitor:" + machine
    append("heartbeat-%s.jsonl" % t.strftime("%Y-%m"), [{"ts": stamp(t), "id": tid, "ok": ok, "detail": detail}])
    fake = [{"id": tid, "label": "mesh-watch on " + machine, "machine": machine}]
    opened, recovered = judge(state, fake, {tid: {"ok": ok, "detail": detail, "fail_runs": fail_runs}}, t,
                              ok_runs=1)
    if opened:
        state["pending"].append({
            "title": "mesh-watch on %s is not watching" % machine,
            "body": "%s. Nothing is checking the mesh until it is back; since %s.\n  get in: %s\n"
                    "— heartbeat on %s" % (detail[0].upper() + detail[1:], clock(state["open"][tid]["since"]),
                                           ways_in(machine), box()),
            "tags": "warning", "priority": "high"})
    if recovered:
        state["pending"].append({
            "title": "mesh-watch on %s is watching again" % machine,
            "body": "Out %s (%s–%s); %s.\n— heartbeat on %s" % (
                ago(recovered[0][1]["since"], t), clock(recovered[0][1]["since"]), clock(stamp(t)), detail, box()),
            "tags": "white_check_mark", "priority": "default"})
    flush(state, dry)
    save_json(state, "heartbeat.json")
    write_heartbeat("heartbeat-last", "%s %s" % (stamp(), "ok" if ok else "fail"))
    prune(t)
    print("heartbeat %s: %s — %s" % (machine, "ok" if ok else "FAIL", detail))
    return 0


def listing():
    fleet, note = read_fleet()
    if note:
        print("note   " + note)
    unlisted, note2, err = drift(fleet, now())
    if note2 or err:
        print("note   " + (note2 or "drift unknown: " + err))
    ts, skipped = targets(read_conf(), fleet, unlisted=unlisted)
    for t in ts:
        print("check  %-44s %s%s" % (t["id"], t.get("url") or t.get("alias") or t.get("host"),
                                     "  expect %r" % t["expect"] if t.get("expect") else ""))
    for h, why in skipped:
        print("skip   %-44s %s" % (h, why))
    print("%d checks, %d skipped" % (len(ts), len(skipped)))
    return 0


def main(argv):
    load_env()
    os.makedirs(STATE, exist_ok=True)
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
