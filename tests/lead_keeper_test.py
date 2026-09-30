#!/usr/bin/python3
"""scripts/lead-keeper.py against a fake `claude`, a fake `kapwa` and a fake ntfy.

    /usr/bin/python3 tests/lead_keeper_test.py

No real session is started, stopped or resumed and nothing leaves the
machine: `claude` is a script that plays the supervisor from a JSON file,
`kapwa` records what it was asked, and ntfy is a server on 127.0.0.1.
"""
import http.server
import json
import os
import subprocess
import tempfile
import threading
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
SCRIPT = os.path.join(REPO, "scripts/lead-keeper.py")

# The supervisor, as far as lead-keeper can see it. world.json holds the
# `claude agents` entries plus knobs: "rc" (does a started session show its
# Remote Control link), "broken" (does `claude agents` fail).
FAKE_CLAUDE = r'''#!/usr/bin/python3
import json, os, sys, random
W = os.environ["FAKE_WORLD"]
w = json.load(open(W))
a = sys.argv[1:]
open(W + ".calls", "a").write(json.dumps({"argv": a, "cwd": os.getcwd()}) + "\n")
def save(): json.dump(w, open(W, "w"))
if a[:1] == ["agents"]:
    if w.get("broken"): sys.exit(1)
    print(json.dumps(w["agents"])); sys.exit(0)
if a[:1] == ["logs"]:
    e = [e for e in w["agents"] if e.get("id") == a[1] and e.get("pid")]
    if not e: print("Couldn't read logs for %s — job not found" % a[1]); sys.exit(1)
    print("\x1b[1m/remote-control\x1b[0m is active · https://claude.ai/code/session_01ABC" if w.get("rc", True) else "/rc connecting…")
    sys.exit(0)
if a[:1] == ["stop"]:
    for e in w["agents"]:
        if e.get("id") == a[1]: e.pop("pid", None)
    save(); print("stopped " + a[1]); sys.exit(0)
if a[:2] == ["--bg", "--resume"]:
    sid, rest = a[2], a[3:]
    job = [e for e in w["agents"] if e.get("sessionId") == sid and e.get("kind") == "background"]
    if job and len(rest) == 1:          # no flags: wake in place
        job[0]["pid"] = 90000 + len(w["agents"]); job[0]["startedAt"] += 1
        save(); print("note: woke session %s with its saved options (--remote-control, -n).\nbackgrounded \x1b[36m·\x1b[0m %s · %s" % (job[0]["id"], job[0]["id"], job[0]["name"])); sys.exit(0)
    name = rest[rest.index("-n") + 1] if "-n" in rest else "?"
    nid = "%08x" % random.getrandbits(32)
    w["agents"].append({"pid": 91000, "id": nid, "kind": "background", "startedAt": 9e12,
                        "sessionId": nid + "-copy", "name": name, "cwd": os.getcwd()})
    save(); print("backgrounded · %s · %s" % (nid, name)); sys.exit(0)
print("fake claude: unexpected " + " ".join(a)); sys.exit(2)
'''

FAKE_KAPWA = r'''#!/usr/bin/python3
import json, os, sys
open(os.environ["FAKE_KAPWA_LOG"], "a").write(json.dumps(sys.argv[1:]) + "\n")
print("said 4f2a9 on #mesh" if sys.argv[1] == "say" and len(sys.argv) > 2 else "ok")
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


def entry(name, id_, pid=None, kind="background", cwd="/tmp"):
    e = {"id": id_, "name": name, "kind": kind, "startedAt": 1000,
         "sessionId": id_ + "-0000-4000-8000-000000000000", "cwd": cwd, "state": "done"}
    if pid:
        e["pid"] = pid
    return e


class LeadKeeper(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Ntfy)
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()

    def setUp(self):
        Ntfy.alerts.clear()
        self.tmp = tempfile.TemporaryDirectory()
        t = self.d = self.tmp.name
        self.home = os.path.join(t, "home")
        self.cwd = os.path.join(self.home, "Projects")
        os.makedirs(self.cwd)
        self.cwd = os.path.realpath(self.cwd)
        json.dump({"projects": {self.cwd: {"hasTrustDialogAccepted": True}}},
                  open(os.path.join(self.home, ".claude.json"), "w"))
        for name, body in (("claude", FAKE_CLAUDE), ("kapwa", FAKE_KAPWA)):
            p = os.path.join(t, name)
            open(p, "w").write(body)
            os.chmod(p, 0o755)
        open(os.path.join(t, "env"), "w").write(
            "MESH_WATCH_NTFY=http://127.0.0.1:%d/topic\n" % self.srv.server_address[1])
        open(os.path.join(t, "leads.conf"), "w").write(
            "# test\nMesh | %s\nMonica | %s | wake up, Monica\n" % (self.cwd, self.cwd))
        self.world({"agents": [entry("Mesh", "aaaa0001", 111, cwd=self.cwd),
                               entry("Monica", "bbbb0002", 222, cwd=self.cwd),
                               entry("Monica · daily briefs site", "cccc0003", cwd=self.cwd)]})

    def tearDown(self):
        self.tmp.cleanup()

    def world(self, w=None):
        p = os.path.join(self.d, "world.json")
        if w is None:
            return json.load(open(p))
        json.dump(w, open(p, "w"))

    def set_agent(self, id_, **kw):
        w = self.world()
        for e in w["agents"]:
            if e["id"] == id_:
                for k, v in kw.items():
                    if v is None:
                        e.pop(k, None)
                    else:
                        e[k] = v
        self.world(w)

    def calls(self, verb=None):
        p = os.path.join(self.d, "world.json.calls")
        out = [json.loads(l) for l in open(p)] if os.path.exists(p) else []
        return [c for c in out if verb is None or c["argv"][:len(verb)] == verb]

    def kapwa(self):
        p = os.path.join(self.d, "kapwa.log")
        return [json.loads(l) for l in open(p)] if os.path.exists(p) else []

    def run_(self, *args, **env):
        e = dict(os.environ, HOME=self.home, CLAUDE_BIN=os.path.join(self.d, "claude"),
                 KAPWA_BIN=os.path.join(self.d, "kapwa"), FAKE_WORLD=os.path.join(self.d, "world.json"),
                 FAKE_KAPWA_LOG=os.path.join(self.d, "kapwa.log"), MESH_WATCH_ENV=os.path.join(self.d, "env"),
                 LEAD_KEEPER_CONF=os.path.join(self.d, "leads.conf"),
                 LEAD_KEEPER_STATE=os.path.join(self.d, "state"), LEAD_KEEPER_RC_WAIT="0",
                 CLAUDE_UPDATE_LOCK=os.path.join(self.d, "update-running"))
        e.update(env)
        p = subprocess.run(["/usr/bin/python3", SCRIPT] + list(args), env=e,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def state(self):
        return json.load(open(os.path.join(self.d, "state/state.json")))

    def test_all_up_does_nothing_but_look(self):
        out = self.run_()
        self.assertEqual(out, "")
        self.assertEqual([c["argv"] for c in self.calls()], [["agents", "--json", "--all"]])
        self.assertEqual(Ntfy.alerts, [])
        self.assertEqual(self.kapwa(), [])

    def test_dead_lead_is_woken_in_place_on_the_second_pass(self):
        self.run_()
        self.set_agent("aaaa0001", pid=None)
        self.assertEqual(self.run_(), "")              # one pass could be a respawn
        self.assertEqual(self.calls(["--bg"]), [])
        out = self.run_()
        wake = self.calls(["--bg"])
        self.assertEqual(len(wake), 1)
        # no flags: flags would start a copy instead of waking this one
        self.assertEqual(wake[0]["argv"][:3], ["--bg", "--resume", "aaaa0001-0000-4000-8000-000000000000"])
        self.assertEqual(len(wake[0]["argv"]), 4)
        self.assertIn("SendMessage CTO", wake[0]["argv"][3])
        self.assertEqual(wake[0]["cwd"], self.cwd)
        self.assertEqual(len(out.strip().splitlines()), 1)  # one line per revival
        self.assertIn("revived Mesh by wake", out)
        self.assertIn("claude.ai/code/session_01ABC", out)
        self.assertEqual(Ntfy.alerts, [])                   # a revival that works is not news
        self.assertEqual(self.kapwa(), [])
        self.assertEqual(self.run_(), "")                   # and it is up now

    def test_removed_entry_is_resumed_from_its_transcript_with_its_name(self):
        self.run_()
        sid = "aaaa0001-0000-4000-8000-000000000000"
        tdir = os.path.join(self.home, ".claude/projects", self.cwd.replace("/", "-"))
        os.makedirs(tdir)
        open(os.path.join(tdir, sid + ".jsonl"), "w").write("{}\n")
        w = self.world()
        w["agents"] = [e for e in w["agents"] if e["id"] != "aaaa0001"]   # claude rm
        self.world(w)
        self.run_()
        out = self.run_()
        argv = self.calls(["--bg"])[0]["argv"]
        self.assertEqual(argv[:7], ["--bg", "--resume", sid, "--remote-control", "-n", "Mesh", argv[6]])
        self.assertIn("revived Mesh by resume", out)

    def test_no_transcript_means_down_said_once(self):
        self.run_()
        w = self.world()
        w["agents"] = [e for e in w["agents"] if e["id"] != "aaaa0001"]
        self.world(w)
        for _ in range(4):
            self.run_()
        self.assertEqual(self.calls(["--bg"]), [])
        self.assertEqual([a["title"].split(": ", 1)[1] for a in Ntfy.alerts], ["lead Mesh is down"])
        self.assertIn("transcript", Ntfy.alerts[0]["body"])
        self.assertEqual(len([k for k in self.kapwa() if k[0] == "say" and "--t" in k]), 1)

    def test_a_living_process_is_never_given_a_second_copy(self):
        hold = subprocess.Popen(["/bin/bash", "-c", "exec -a claude-lead sleep 60"])
        try:
            self.set_agent("aaaa0001", pid=hold.pid)
            self.run_()
            self.set_agent("aaaa0001", pid=None)            # listed dead, still running
            for _ in range(5):
                self.run_()
            self.assertEqual(self.calls(["--bg"]), [])
            self.assertEqual([a["title"].split(": ", 1)[1] for a in Ntfy.alerts], ["lead Mesh is not answering"])
        finally:
            hold.kill()

    def test_failed_revival_alerts_once_stops_its_copy_and_backs_off(self):
        self.run_()
        w = self.world()
        w["rc"] = False
        self.world(w)
        self.set_agent("aaaa0001", pid=None)
        self.run_()
        out = self.run_()
        self.assertIn("revival FAILED Mesh", out)
        self.assertEqual(self.calls(["stop"])[0]["argv"], ["stop", "aaaa0001"])
        self.assertEqual([a["title"].split(": ", 1)[1] for a in Ntfy.alerts], ["lead Mesh did not come back"])
        self.assertIn("privacy prompt", Ntfy.alerts[0]["body"])
        for _ in range(3):                                   # no tight loop
            self.run_()
        self.assertEqual(len(self.calls(["--bg"])), 1)
        self.assertEqual(len(Ntfy.alerts), 1)
        # after the back-off it tries again, and this time it works
        w = self.world()
        w["rc"] = True
        self.world(w)
        self.run_(LEAD_KEEPER_RETRY_HOURS="0")
        self.assertEqual(len(self.calls(["--bg"])), 2)
        self.run_()
        self.assertEqual([a["title"].split(": ", 1)[1] for a in Ntfy.alerts],
                         ["lead Mesh did not come back", "lead Mesh is back"])
        done = [k for k in self.kapwa() if k[0] == "done"]
        self.assertEqual(done[0][1], "4f2a9")               # closes the item it opened
        self.assertEqual(len([k for k in self.kapwa() if k[0] == "say" and "--t" in k]), 1)

    def test_dying_twice_in_a_day_is_said_once(self):
        self.run_()
        for _ in range(3):
            self.set_agent("aaaa0001", pid=None)
            self.run_()
            self.run_()
            self.run_()
        self.assertEqual(len(self.calls(["--bg"])), 3)
        self.assertEqual([a["title"].split(": ", 1)[1] for a in Ntfy.alerts], ["lead Mesh died twice in 24h"])

    def test_only_exact_names_are_leads(self):
        self.run_()
        self.run_()
        # "Monica · daily briefs site" is dead and must stay that way
        self.assertEqual(self.calls(["--bg"]), [])
        self.set_agent("bbbb0002", pid=None)
        self.run_()
        self.run_()
        argv = self.calls(["--bg"])[0]["argv"]
        self.assertEqual(argv[2], "bbbb0002-0000-4000-8000-000000000000")
        self.assertEqual(argv[3], "wake up, Monica")         # its own note

    def test_unreadable_list_judges_nothing_and_says_so_once(self):
        self.run_()
        w = self.world()
        w["broken"] = True
        self.world(w)
        for _ in range(8):
            self.run_(LEAD_KEEPER_UNREADABLE_RUNS="3")
        self.assertEqual(self.calls(["--bg"]), [])
        self.assertEqual([a["title"].split(": ", 1)[1] for a in Ntfy.alerts], ["lead-keeper is blind"])

    def test_update_in_progress_pauses_it(self):
        self.run_()
        self.set_agent("aaaa0001", pid=None)
        open(os.path.join(self.d, "update-running"), "w").write("%d\n" % os.getpid())
        for _ in range(3):
            self.run_()
        self.assertEqual(self.calls(["--bg"]), [])
        os.remove(os.path.join(self.d, "update-running"))
        self.run_()
        self.run_()
        self.assertEqual(len(self.calls(["--bg"])), 1)

    def test_untrusted_cwd_is_not_tried(self):
        json.dump({"projects": {}}, open(os.path.join(self.home, ".claude.json"), "w"))
        self.run_()
        self.set_agent("aaaa0001", pid=None)
        self.run_()
        self.run_()
        self.assertEqual(self.calls(["--bg"]), [])
        self.assertIn("does not trust", Ntfy.alerts[0]["body"])

    def test_dry_run_changes_nothing(self):
        self.run_()
        self.set_agent("aaaa0001", pid=None)
        before = self.state()
        out = self.run_("--dry-run")
        self.assertIn("Mesh           DEAD", out)
        self.assertIn("would run, from %s" % self.cwd, out)
        self.assertIn("Monica         up", out)
        self.assertEqual(self.calls(["--bg"]), [])
        self.assertEqual(self.state(), before)
        self.assertEqual(Ntfy.alerts, [])


if __name__ == "__main__":
    unittest.main()
