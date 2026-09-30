#!/usr/bin/python3
"""scripts/claude-update.py against a fake installer, supervisor and ntfy.

    /usr/bin/python3 tests/claude_update_test.py

Nothing is installed or restarted: ~/.local/bin/claude is a symlink in a
temp dir to a fake `claude` that plays `update`, `respawn` and the rest
from a JSON file, and ntfy is a server on 127.0.0.1.
"""
import http.server
import json
import os
import subprocess
import tempfile
import threading
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
SCRIPT = os.path.join(REPO, "scripts/claude-update.py")

# world.json: "next" (what `update` installs, or null), "probe_rc" (does a
# new session reach Remote Control), "probe_old" (does it run the old
# binary), "lose_rc" (ids that come back from respawn without RC).
FAKE_CLAUDE = r'''#!/usr/bin/python3
import json, os, sys
W = os.environ["FAKE_WORLD"]; w = json.load(open(W)); a = sys.argv[1:]
d = os.path.dirname(W)
open(W + ".calls", "a").write(json.dumps({"argv": a, "locked": os.path.exists(os.environ["CLAUDE_UPDATE_LOCK"])}) + "\n")
def save(): json.dump(w, open(W, "w"))
me = os.path.basename(os.path.realpath(sys.argv[0]))
if a == ["--version"]: print(me + " (Claude Code)"); sys.exit(0)
if a == ["update"]:
    if os.path.exists(os.path.join(d, "fail")): print("network error"); sys.exit(1)
    if w.get("next"):
        link = os.environ["CLAUDE_BIN"]; os.remove(link)
        os.symlink(os.path.join(d, "versions", w["next"]), link)
        print("Successfully updated to " + w["next"])
    else:
        print("Claude Code is up to date")
    sys.exit(0)
if a[:1] == ["agents"]: print(json.dumps(w["agents"])); sys.exit(0)
if a[:1] == ["logs"]:
    e = [e for e in w["agents"] if e.get("id") == a[1] and e.get("pid")]
    if not e: print("job not found"); sys.exit(1)
    print("https://claude.ai/code/session_01X" if e[0].get("rc") else "/rc connecting…"); sys.exit(0)
if a[:2] in (["--bg", "--remote-control"], ["--bg", "--resume"]):
    sid = a[2] if a[1] == "--resume" else "b0be0000-new"
    w["agents"].append({"id": "b0be0000", "name": a[-1], "kind": "background", "pid": 4242,
                        "rc": w.get("probe_rc", True), "startedAt": 1, "sessionId": sid})
    save(); print("backgrounded · b0be0000 · " + a[-1]); sys.exit(0)
if a[:1] in (["stop"], ["rm"]):
    w["agents"] = [e for e in w["agents"] if not (a[0] == "rm" and e.get("id") == a[1])]
    for e in w["agents"]:
        if e.get("id") == a[1]: e.pop("pid", None)
    save(); sys.exit(0)
if a[:1] == ["respawn"]:
    for e in w["agents"]:
        if e.get("id") == a[1]:
            e["pid"] = e["pid"] + 1; e["rc"] = a[1] not in w.get("lose_rc", [])
            j = os.path.join(d, "jobs", a[1], "state.json"); s = json.load(open(j))
            s["cliVersion"] = me; json.dump(s, open(j, "w"))
    save(); print("respawned " + a[1]); sys.exit(0)
print("unexpected", a); sys.exit(2)
'''

# lsof -Fn for the probe: the new binary unless the world says old.
FAKE_LSOF = r'''#!/usr/bin/python3
import json, os
w = json.load(open(os.environ["FAKE_WORLD"]))
d = os.path.dirname(os.environ["FAKE_WORLD"])
print("p4242\nn" + os.path.join(d, "versions", w["old"] if w.get("probe_old") else (w.get("next") or w["old"])))
'''


class Ntfy(http.server.BaseHTTPRequestHandler):
    alerts = []

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        Ntfy.alerts.append({"title": self.headers.get("Title"), "body": self.rfile.read(n).decode()})
        self.send_response(200)
        self.end_headers()

    def log_message(self, *a):
        pass


class ClaudeUpdate(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Ntfy)
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()

    def setUp(self):
        Ntfy.alerts.clear()
        self.tmp = tempfile.TemporaryDirectory(prefix="claude-")  # lsof paths name claude
        t = self.d = self.tmp.name
        os.makedirs(os.path.join(t, "versions"))
        os.makedirs(os.path.join(t, "bin"))
        os.makedirs(os.path.join(t, "Projects"))
        for v in ("2.1.285", "2.1.290"):
            p = os.path.join(t, "versions", v)
            open(p, "w").write(FAKE_CLAUDE)
            os.chmod(p, 0o755)
        os.symlink(os.path.join(t, "versions", "2.1.285"), os.path.join(t, "bin", "claude"))
        p = os.path.join(t, "lsof")
        open(p, "w").write(FAKE_LSOF)
        os.chmod(p, 0o755)
        open(os.path.join(t, "env"), "w").write(
            "MESH_WATCH_NTFY=http://127.0.0.1:%d/topic\n" % self.srv.server_address[1])
        agents = [{"id": "aaaa0001", "name": "Mesh", "kind": "background", "pid": 100, "rc": True, "status": "busy"},
                  {"id": "bbbb0002", "name": "Monica", "kind": "background", "pid": 200, "rc": True, "status": "idle"},
                  {"id": "cccc0003", "name": "worker", "kind": "background", "pid": 300, "rc": False, "status": "idle"},
                  {"id": "dddd0004", "name": "gone", "kind": "background", "status": "idle"},
                  {"name": "CTO", "kind": "interactive", "pid": 400}]
        for a in agents:
            if "id" in a:
                os.makedirs(os.path.join(t, "jobs", a["id"]))
                json.dump({"cliVersion": "2.1.285",
                           "respawnFlags": ["--remote-control", "-n", a["name"]] if a["rc"] else ["-n", a["name"]]}
                          if "rc" in a else {}, open(os.path.join(t, "jobs", a["id"], "state.json"), "w"))
        self.world({"old": "2.1.285", "next": "2.1.290", "agents": agents})

    def tearDown(self):
        self.tmp.cleanup()

    def world(self, w=None, **kw):
        p = os.path.join(self.d, "world.json")
        if w is None:
            w = json.load(open(p))
            if not kw:
                return w
        w.update(kw)
        json.dump(w, open(p, "w"))

    def calls(self, verb=None):
        p = os.path.join(self.d, "world.json.calls")
        out = [json.loads(l) for l in open(p)] if os.path.exists(p) else []
        return [c for c in out if verb is None or c["argv"][:len(verb)] == verb]

    def run_(self, *args, **env):
        t = self.d
        e = dict(os.environ, HOME=t, CLAUDE_BIN=os.path.join(t, "bin/claude"), KAPWA_BIN=os.path.join(t, "no-kapwa"),
                 FAKE_WORLD=os.path.join(t, "world.json"), MESH_WATCH_ENV=os.path.join(t, "env"),
                 CLAUDE_UPDATE_STATE=os.path.join(t, "state"), CLAUDE_UPDATE_LOCK=os.path.join(t, "state/running"),
                 CLAUDE_JOBS=os.path.join(t, "jobs"), CLAUDE_BUNDLE=os.path.join(t, "ClaudeCode.app"),
                 CLAUDE_UPDATE_PROBE_CWD=os.path.join(t, "Projects"), CLAUDE_UPDATE_LSOF=os.path.join(t, "lsof"),
                 CLAUDE_UPDATE_PROBE_WAIT="0", LEAD_KEEPER_CONF=os.path.join(t, "no-leads.conf"), CLAUDE_UPDATE_RC_WAIT="0", CLAUDE_UPDATE_BUSY_WAIT="0")
        e.update(env)
        p = subprocess.run(["/usr/bin/python3", SCRIPT] + list(args), env=e, capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def test_no_new_version_is_one_line_and_nothing_else(self):
        self.world(next=None)
        out = self.run_()
        self.assertIn("up to date: 2.1.285", out)
        self.assertEqual(len(out.strip().splitlines()), 1)
        self.assertEqual(self.calls(["--bg"]) + self.calls(["respawn"]), [])
        self.assertFalse(os.path.exists(os.path.join(self.d, "state/running")))
        self.assertEqual(Ntfy.alerts, [])

    def test_new_version_is_probed_then_every_idle_session_respawned_under_the_lock(self):
        out = self.run_()
        self.assertIn("updated 2.1.285 -> 2.1.290", out)
        order = [c["argv"][0] for c in self.calls() if c["argv"][0] in ("--bg", "respawn")]
        self.assertEqual(order[0], "--bg")                       # probe before any respawn
        resp = self.calls(["respawn"])
        # the busy one gets its time and is left; the dead one and CTO are not ours
        self.assertEqual(sorted(c["argv"][1] for c in resp), ["bbbb0002", "cccc0003"])
        self.assertTrue(all(c["locked"] for c in resp))         # lead-keeper stands aside
        self.assertIn("left aaaa0001 (Mesh) on the old version", out)
        self.assertEqual(self.calls(["rm"])[0]["argv"], ["rm", "b0be0000"])  # probe cleaned up
        self.assertFalse(os.path.exists(os.path.join(self.d, "state/running")))
        self.assertEqual(Ntfy.alerts, [])

    def test_the_probe_is_one_claude_ai_record_resumed_each_time(self):
        self.run_()
        first = self.calls(["--bg"])[0]["argv"]
        self.assertNotIn("--resume", first)
        # the probe's conversation exists, so next time it is resumed, not made anew
        tdir = os.path.join(self.d, ".claude/projects", os.path.join(self.d, "Projects").replace("/", "-"))
        os.makedirs(tdir)
        open(os.path.join(tdir, "b0be0000-new.jsonl"), "w").write("{}\n")
        self.world(next="2.1.291")
        p = os.path.join(self.d, "versions", "2.1.291")
        open(p, "w").write(FAKE_CLAUDE)
        os.chmod(p, 0o755)
        self.run_()
        second = self.calls(["--bg"])[1]["argv"]
        self.assertEqual(second[:3], ["--bg", "--resume", "b0be0000-new"])
        self.assertEqual(second[3:], ["--remote-control", "-n", "claude-update probe"])
        self.assertEqual(len(self.calls(["rm"])), 2)            # gone locally both times

    def test_a_probe_that_hangs_stops_everything_and_names_the_path(self):
        self.world(probe_rc=False)
        out = self.run_()
        self.assertIn("NOT respawning", out)
        self.assertEqual(self.calls(["respawn"]), [])
        self.assertEqual(len(Ntfy.alerts), 1)
        self.assertIn("grant Full Disk Access to Claude 2.1.290", Ntfy.alerts[0]["title"])
        # no bundle hard-linked here, so the grant is for the version file
        self.assertIn(os.path.join(self.d, "versions/2.1.290"), Ntfy.alerts[0]["body"])
        self.assertEqual(self.calls(["rm"])[0]["argv"], ["rm", "b0be0000"])

    def test_a_bundle_linked_to_the_version_is_what_needs_the_grant(self):
        exe = os.path.join(self.d, "ClaudeCode.app/Contents/MacOS")
        os.makedirs(exe)
        os.link(os.path.join(self.d, "versions/2.1.290"), os.path.join(exe, "claude"))
        self.world(probe_rc=False)
        self.run_()
        self.assertIn(os.path.join(self.d, "ClaudeCode.app"), Ntfy.alerts[0]["body"])

    def test_a_probe_on_the_old_binary_proves_nothing(self):
        self.world(probe_old=True)
        out = self.run_()
        self.assertIn("proves nothing", out)
        self.assertEqual(self.calls(["respawn"]), [])
        self.assertEqual(len(Ntfy.alerts), 1)

    def test_a_session_back_without_remote_control_is_one_alert(self):
        self.world(lose_rc=["bbbb0002"])
        self.run_()
        self.assertEqual(len(Ntfy.alerts), 1)
        self.assertIn("Monica (bbbb0002): back without Remote Control", Ntfy.alerts[0]["body"])
        self.assertNotIn("worker", Ntfy.alerts[0]["body"])      # never had RC, not missing it

    def test_a_kept_session_back_without_rc_is_left_to_lead_keeper(self):
        open(os.path.join(self.d, "leads.conf"), "w").write("Monica | ~/Projects\n")
        self.world(lose_rc=["bbbb0002"])
        out = self.run_(LEAD_KEEPER_CONF=os.path.join(self.d, "leads.conf"))
        self.assertIn("lead-keeper repairs it", out)
        self.assertEqual(Ntfy.alerts, [])

    def test_an_update_that_keeps_failing_is_said_once(self):
        open(os.path.join(self.d, "fail"), "w").write("")
        for _ in range(4):
            self.run_()
        self.assertEqual(len(Ntfy.alerts), 1)
        self.assertIn("has not updated for 2 weeks", Ntfy.alerts[0]["title"])
        self.assertTrue(os.path.exists(os.path.join(self.d, "state/last-run")))

    def test_dry_run_installs_and_restarts_nothing(self):
        out = self.run_("--dry-run")
        self.assertIn("would run:", out)
        self.assertIn("would respawn bbbb0002 (Monica, idle, RC yes)", out)
        self.assertEqual(self.calls(["update"]) + self.calls(["--bg"]) + self.calls(["respawn"]), [])
        self.assertEqual(os.path.basename(os.path.realpath(os.path.join(self.d, "bin/claude"))), "2.1.285")


if __name__ == "__main__":
    unittest.main()
