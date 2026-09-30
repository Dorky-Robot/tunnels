#!/usr/bin/python3
"""scripts/lead-keeper.py against a fake `claude`, a fake `kapwa` and a fake ntfy.

    /usr/bin/python3 tests/lead_keeper_test.py

No real session is started, stopped or resumed and nothing leaves the
machine: `claude` is a script that plays the supervisor from a JSON file
(each "session" is a sleeping process named claude-fake, since a listed pid
only counts when it is alive), `kapwa` records what it was asked, and ntfy
is a server on 127.0.0.1.
"""
import http.server
import json
import os
import signal
import subprocess
import tempfile
import threading
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
SCRIPT = os.path.join(REPO, "scripts/lead-keeper.py")
FIXTURES = os.path.join(REPO, "tests/fixtures")

# world.json: "agents" as `claude agents --json --all` shows them, plus
# "rc" (does a started session reach Remote Control), "broken" (does the
# list fail) and "flags" (respawnFlags a revived session is saved with).
FAKE_CLAUDE = r'''#!/usr/bin/python3
import json, os, sys, random, subprocess
W = os.environ["FAKE_WORLD"]; JOBS = os.environ["CLAUDE_JOBS"]
w = json.load(open(W)); a = sys.argv[1:]
open(W + ".calls", "a").write(json.dumps({"argv": a, "cwd": os.getcwd()}) + "\n")
def save(): json.dump(w, open(W, "w"))
def proc():
    return subprocess.Popen(["/bin/bash", "-c", "exec -a claude-fake sleep 300"], start_new_session=True,
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).pid
def flags(id_, f):
    os.makedirs(os.path.join(JOBS, id_), exist_ok=True)
    json.dump({"respawnFlags": f}, open(os.path.join(JOBS, id_, "state.json"), "w"))
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
        if e.get("id") == a[1] and e.get("pid"):
            try: os.kill(e["pid"], 9)
            except OSError: pass
            e.pop("pid")
    save(); print("stopped " + a[1]); sys.exit(0)
if a[:1] == ["respawn"]:
    for e in w["agents"]:
        if e.get("id") == a[1]: e["pid"] = proc()
    save(); print("respawned " + a[1]); sys.exit(0)
if a[:2] == ["--bg", "--resume"]:
    sid, rest = a[2], a[3:]
    job = [e for e in w["agents"] if e.get("sessionId") == sid and e.get("kind") == "background"]
    if job and len(rest) == 1:          # no flags: wake in place
        job[0]["pid"] = proc(); job[0]["startedAt"] += 1
        save(); print("note: woke session %s with its saved options.\nbackgrounded \x1b[36m·\x1b[0m %s · %s" % (job[0]["id"], job[0]["id"], job[0]["name"])); sys.exit(0)
    name = rest[rest.index("-n") + 1]
    nid = "%08x" % random.getrandbits(32)
    w["agents"].append({"pid": proc(), "id": nid, "kind": "background", "startedAt": 9e12,
                        "sessionId": nid + "-copy", "name": name, "cwd": os.getcwd()})
    flags(nid, rest[:rest.index("-n") + 2])
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


def sleeper():
    return subprocess.Popen(["/bin/bash", "-c", "exec -a claude-fake sleep 300"], start_new_session=True,
                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).pid


class LeadKeeper(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Ntfy)
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()
        cls.srv.server_close()
        subprocess.run(["/usr/bin/pkill", "-f", "claude-fake"])   # sessions the fake started

    def setUp(self):
        Ntfy.alerts.clear()
        self.tmp = tempfile.TemporaryDirectory()
        t = self.d = self.tmp.name
        self.home = os.path.join(t, "home")
        self.cwd = os.path.realpath(os.path.join(self.home, "Projects"))
        os.makedirs(self.cwd)
        self.wcwd = os.path.join(self.cwd, "worktree")
        os.makedirs(self.wcwd)
        json.dump({"projects": {self.cwd: {"hasTrustDialogAccepted": True}}},
                  open(os.path.join(self.home, ".claude.json"), "w"))
        for name, body in (("claude", FAKE_CLAUDE), ("kapwa", FAKE_KAPWA)):
            p = os.path.join(t, name)
            open(p, "w").write(body)
            os.chmod(p, 0o755)
        open(os.path.join(t, "env"), "w").write(
            "MESH_WATCH_NTFY=http://127.0.0.1:%d/topic\n" % self.srv.server_address[1])
        open(os.path.join(t, "leads.conf"), "w").write(
            "# test\nMesh | %s\nMonica | %s | wake up, Monica\nexclude | Mesh · scratch\n" % (self.cwd, self.cwd))
        open(os.path.join(t, "daemon.log"), "w").write("")
        self.world({"agents": []})
        self.add("Mesh", "aaaa0001")
        self.add("Monica", "bbbb0002")
        self.add("Monica · daily briefs site", "cccc0003", cwd=self.wcwd)

    def tearDown(self):
        for e in self.world()["agents"]:
            if e.get("pid"):
                try:
                    os.kill(e["pid"], signal.SIGKILL)
                except OSError:
                    pass
        self.tmp.cleanup()

    def add(self, name, id_, alive=True, rc=True, started=1000, cwd=None, kind="background"):
        w = self.world()
        e = {"id": id_, "name": name, "kind": kind, "startedAt": started, "cwd": cwd or self.cwd,
             "sessionId": id_ + "-0000-4000-8000-000000000000", "status": "idle", "state": "done"}
        if alive:
            e["pid"] = sleeper()
        w["agents"].append(e)
        self.world(w)
        os.makedirs(os.path.join(self.d, "jobs", id_), exist_ok=True)
        json.dump({"respawnFlags": (["--remote-control"] if rc else []) + ["-n", name]},
                  open(os.path.join(self.d, "jobs", id_, "state.json"), "w"))

    def world(self, w=None):
        p = os.path.join(self.d, "world.json")
        if w is None:
            return json.load(open(p))
        json.dump(w, open(p, "w"))

    def kill(self, id_, forget_pid=True, **kw):
        """The session dies: its process goes, and the list drops its pid."""
        w = self.world()
        for e in w["agents"]:
            if e["id"] == id_:
                os.kill(e["pid"], signal.SIGKILL)
                if forget_pid:
                    e.pop("pid")
                e.update(kw)
        self.world(w)

    def set_(self, id_, **kw):
        w = self.world()
        for e in w["agents"]:
            if e["id"] == id_:
                e.update(kw)
        self.world(w)

    def calls(self, *verb):
        p = os.path.join(self.d, "world.json.calls")
        out = [json.loads(l) for l in open(p)] if os.path.exists(p) else []
        return [c for c in out if list(verb) == c["argv"][:len(verb)]]

    def revivals(self):
        return self.calls("--bg") + self.calls("respawn") + self.calls("stop")

    def kapwa(self):
        p = os.path.join(self.d, "kapwa.log")
        return [json.loads(l) for l in open(p)] if os.path.exists(p) else []

    def titles(self):
        return [a["title"].split(": ", 1)[1] for a in Ntfy.alerts]

    def run_(self, *args, **env):
        e = dict(os.environ, HOME=self.home, CLAUDE_BIN=os.path.join(self.d, "claude"),
                 KAPWA_BIN=os.path.join(self.d, "kapwa"), FAKE_WORLD=os.path.join(self.d, "world.json"),
                 FAKE_KAPWA_LOG=os.path.join(self.d, "kapwa.log"), MESH_WATCH_ENV=os.path.join(self.d, "env"),
                 LEAD_KEEPER_CONF=os.path.join(self.d, "leads.conf"), CLAUDE_JOBS=os.path.join(self.d, "jobs"),
                 CLAUDE_DAEMON_LOG=os.path.join(self.d, "daemon.log"),
                 LEAD_KEEPER_STATE=os.path.join(self.d, "state"), LEAD_KEEPER_RC_WAIT="0",
                 LEAD_KEEPER_PRESSURE_LEVEL="99", CLAUDE_UPDATE_LOCK=os.path.join(self.d, "update-running"))
        e.update(env)
        p = subprocess.run(["/usr/bin/python3", SCRIPT] + list(args), env=e,
                           capture_output=True, text=True, timeout=120)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def state(self):
        return json.load(open(os.path.join(self.d, "state/state.json")))

    # ---- all well ----------------------------------------------------------

    def test_all_up_does_nothing_but_look(self):
        self.assertEqual(self.run_(), "")
        self.assertEqual([c["argv"] for c in self.calls()], [["agents", "--json", "--all"]])
        self.assertEqual(Ntfy.alerts + self.kapwa(), [])

    # ---- who it keeps ------------------------------------------------------

    def test_a_dead_lead_is_woken_in_place_on_the_second_pass_and_told_to_tell_cto(self):
        self.run_()
        self.kill("aaaa0001")
        self.assertEqual(self.run_(), "")              # one pass could be a respawn
        self.assertEqual(self.revivals(), [])
        out = self.run_()
        wake = self.calls("--bg")
        self.assertEqual(len(wake), 1)
        # no flags: flags would start a copy instead of waking this one
        self.assertEqual(wake[0]["argv"][:3], ["--bg", "--resume", "aaaa0001-0000-4000-8000-000000000000"])
        self.assertEqual(len(wake[0]["argv"]), 4)
        self.assertIn('SendMessage CTO: "Mesh back"', wake[0]["argv"][3])
        self.assertEqual(wake[0]["cwd"], self.cwd)
        self.assertEqual(len(out.strip().splitlines()), 1)  # one line per revival
        self.assertIn("revived Mesh by wake", out)
        self.assertEqual(Ntfy.alerts + self.kapwa(), [])    # a revival that works is not news
        self.assertEqual(self.run_(), "")

    def test_a_dead_worker_comes_back_in_its_own_cwd_and_tells_its_lead(self):
        self.run_()
        self.kill("cccc0003")
        self.run_()
        self.run_()
        wake = self.calls("--bg")[0]
        self.assertEqual(wake["cwd"], self.wcwd)
        self.assertIn('SendMessage "Monica" that you are back', wake["argv"][3])

    def test_a_stale_pid_in_the_list_is_dead(self):
        self.run_()
        self.kill("aaaa0001", forget_pid=False, status="blocked")   # listed pid, no process
        self.run_()
        self.run_()
        self.assertEqual(len(self.calls("--bg")), 1)

    def test_a_removed_session_never_comes_back(self):
        self.run_()
        self.kill("cccc0003")
        self.kill("aaaa0001")
        w = self.world()
        w["agents"] = [e for e in w["agents"] if e["id"] not in ("cccc0003", "aaaa0001")]   # claude rm
        self.world(w)
        for _ in range(4):
            self.run_()
        self.assertEqual(self.revivals(), [])
        # a removed worker is finished; a removed lead is worth one word
        self.assertEqual(self.titles(), ["lead Mesh is gone"])

    def test_only_the_newest_duplicate_counts(self):
        self.add("Mesh", "0ld00001", alive=False, started=10)
        self.add("Mesh", "0ld00002", alive=False, started=20)
        for _ in range(3):
            self.run_()
        self.assertEqual(self.revivals(), [])                # one alive: the name is alive
        self.kill("aaaa0001")
        self.run_()
        self.run_()
        wake = self.calls("--bg")
        self.assertEqual(len(wake), 1)
        self.assertEqual(wake[0]["argv"][2], "aaaa0001-0000-4000-8000-000000000000")

    def test_names_that_are_not_ours_or_are_excluded_are_never_touched(self):
        self.add("Meshy", "dddd0004", alive=False)
        self.add("Mesh · scratch", "eeee0005", alive=False)
        self.add("someone else", "ffff0006", alive=False)
        for _ in range(3):
            self.run_()
        self.assertEqual(self.revivals(), [])

    def test_a_lead_note_in_the_config_is_used(self):
        self.run_()
        self.kill("bbbb0002")
        self.run_()
        self.run_()
        self.assertEqual(self.calls("--bg")[0]["argv"][3], "wake up, Monica")

    # ---- how -----------------------------------------------------------------

    def test_an_idle_retire_comes_back_by_respawn_without_a_note(self):
        self.run_()
        self.kill("cccc0003")
        open(os.path.join(self.d, "daemon.log"), "a").write(
            "[2026-09-30T16:08:19.438Z] [bg] bg retire cccc0003: idle-prompt, idle 8h\n")
        self.run_()
        out = self.run_()
        self.assertEqual(self.calls("--bg"), [])
        self.assertEqual(self.calls("respawn")[0]["argv"], ["respawn", "cccc0003"])
        self.assertIn("revived Monica · daily briefs site by respawn", out)

    def test_a_retire_before_this_process_started_is_not_this_death(self):
        self.set_("aaaa0001", startedAt=1790786167066)             # 2026-09-30T16:36Z
        self.run_()
        self.kill("aaaa0001")
        open(os.path.join(self.d, "daemon.log"), "a").write(
            "[2026-09-30T10:40:18.949Z] [bg] bg retire aaaa0001: idle-prompt, idle 8h\n")
        self.run_()
        self.run_()
        self.assertEqual(self.calls("respawn"), [])
        self.assertEqual(len(self.calls("--bg")), 1)

    def test_a_session_saved_without_rc_comes_back_as_a_copy_with_it(self):
        self.add("Mesh · probe", "abab0007", alive=False, rc=False)
        self.run_()
        self.run_()
        argv = self.calls("--bg")[0]["argv"]
        self.assertEqual(argv[3:6], ["--remote-control", "-n", "Mesh · probe"])

    def test_a_live_session_that_lost_rc_is_stopped_and_copied_but_not_while_busy(self):
        self.add("Mesh · worker", "abab0008", rc=False)
        self.set_("abab0008", status="busy")
        for _ in range(3):
            self.run_()
        self.assertEqual(self.revivals(), [])                # mid-turn: wait
        self.set_("abab0008", status="waiting")
        self.run_()
        self.run_()
        self.assertEqual(self.revivals(), [])                # a question pending: wait
        self.set_("abab0008", status="idle")
        self.run_()
        out = self.run_()
        self.assertEqual(self.calls("stop")[0]["argv"], ["stop", "abab0008"])
        argv = self.calls("--bg")[0]["argv"]
        self.assertEqual(argv[3:6], ["--remote-control", "-n", "Mesh · worker"])
        self.assertIn("Remote Control had dropped", argv[6])
        self.assertIn("by rc", out)
        for _ in range(3):                                   # the copy is the newest now
            self.run_()
        self.assertEqual(len(self.calls("--bg")), 1)

    def test_a_vanished_interactive_lead_is_resumed_from_its_conversation(self):
        w = self.world()
        w["agents"] = [e for e in w["agents"] if e["id"] != "aaaa0001"]
        w["agents"].append({"pid": sleeper(), "kind": "interactive", "name": "Mesh", "startedAt": 5,
                            "sessionId": "1a1a-cto", "cwd": self.cwd, "status": "idle"})
        self.world(w)
        self.run_()
        w = self.world()
        os.kill(w["agents"][-1]["pid"], signal.SIGKILL)
        w["agents"] = w["agents"][:-1]                         # gone from the list
        self.world(w)
        self.run_()
        self.run_()
        argv = self.calls("--bg")[0]["argv"]
        self.assertEqual(argv[2:6], ["1a1a-cto", "--remote-control", "-n", "Mesh"])

    # ---- stuck where it stands ------------------------------------------------

    def stuck(self, id_, fixture, status="waiting"):
        """Point a session's transcript at a copy of a fixture."""
        path = os.path.join(self.d, id_ + ".jsonl")
        open(path, "w").write(open(os.path.join(FIXTURES, fixture)).read())
        j = os.path.join(self.d, "jobs", id_, "state.json")
        st = json.load(open(j))
        st["linkScanPath"] = path
        json.dump(st, open(j, "w"))
        self.set_(id_, status=status)
        return path

    def test_a_worker_stuck_on_a_worktree_trust_prompt_is_told_once_and_not_revived(self):
        path = self.stuck("cccc0003", "trust-stall.jsonl")
        for _ in range(4):
            self.run_()
        self.assertEqual(self.revivals(), [])                # same cwd, same hang: do not revive
        self.assertEqual(self.titles(), ["Monica - daily briefs site is stuck on a trust prompt"])  # ASCII header
        body = Ntfy.alerts[0]["body"]
        self.assertIn("Monica · daily briefs site (cccc0003, Monica's)", body)
        self.assertIn("since 2026-09-30T17:09:29Z", body)
        self.assertIn("without EnterWorktree", body)
        self.assertIn("2 messages queued", body)
        self.assertEqual(len([k for k in self.kapwa() if k[0] == "say" and "--t" in k]), 1)
        # restarted by someone and moving again: one "back", and the item is closed
        open(path, "a").write(json.dumps({"type": "assistant", "timestamp": "2026-09-30T18:00:00Z",
                                          "message": {"role": "assistant", "content": [{"type": "text", "text": "ok"}]}}) + "\n")
        self.run_()
        self.run_()
        self.assertEqual(self.titles()[1:], ["Monica - daily briefs site is back"])
        self.assertEqual([k[1] for k in self.kapwa() if k[0] == "done"], ["4f2a9"])

    def test_a_long_tool_call_is_not_a_stall(self):
        self.stuck("cccc0003", "long-tool-call.jsonl", status="busy")
        for _ in range(3):
            self.run_()
        self.assertEqual(Ntfy.alerts, [])

    def test_a_fresh_worktree_step_is_not_yet_a_stall(self):
        self.stuck("cccc0003", "trust-stall.jsonl")
        for _ in range(3):
            self.run_(LEAD_KEEPER_TRUST_MIN="100000000")
        self.assertEqual(Ntfy.alerts, [])

    def test_an_idle_session_is_not_a_stall(self):
        self.stuck("cccc0003", "trust-stall.jsonl", status="idle")   # it finished; nothing hangs
        for _ in range(3):
            self.run_()
        self.assertEqual(Ntfy.alerts, [])

    def test_dry_run_names_the_stuck_one(self):
        self.stuck("cccc0003", "trust-stall.jsonl")
        out = self.run_("--dry-run")
        self.assertIn("STUCK", out)
        self.assertIn("since 2026-09-30T17:09:29Z", out)

    # ---- safety ----------------------------------------------------------------

    def test_a_living_process_is_never_given_a_second_copy(self):
        self.run_()
        w = self.world()
        w["agents"][0].pop("pid")                              # listed dead, still running
        self.world(w)
        for _ in range(5):
            self.run_()
        self.assertEqual(self.revivals(), [])
        self.assertEqual(self.titles(), ["lead Mesh is not answering"])

    def test_leads_first_and_workers_wait_while_memory_is_tight(self):
        self.run_()
        self.kill("cccc0003")
        self.kill("aaaa0001")
        self.run_(LEAD_KEEPER_PRESSURE_LEVEL="0")
        out = self.run_(LEAD_KEEPER_PRESSURE_LEVEL="0")
        self.assertEqual([c["argv"][2][:8] for c in self.calls("--bg")], ["aaaa0001"])
        self.assertIn("deferred Monica · daily briefs site", out)
        self.run_()
        self.assertEqual(len(self.calls("--bg")), 2)

    def test_update_in_progress_pauses_it(self):
        self.run_()
        self.kill("aaaa0001")
        open(os.path.join(self.d, "update-running"), "w").write("%d\n" % os.getpid())
        for _ in range(3):
            self.run_()
        self.assertEqual(self.revivals(), [])
        os.remove(os.path.join(self.d, "update-running"))
        self.run_()
        self.run_()
        self.assertEqual(len(self.calls("--bg")), 1)

    def test_untrusted_cwd_is_not_tried(self):
        json.dump({"projects": {}}, open(os.path.join(self.home, ".claude.json"), "w"))
        self.run_()
        self.kill("aaaa0001")
        self.run_()
        self.run_()
        self.assertEqual(self.revivals(), [])
        self.assertIn("does not trust", Ntfy.alerts[0]["body"])

    # ---- hearing about it ----------------------------------------------------

    def test_no_rc_link_is_the_privacy_hang_said_once_and_held_until_the_binary_changes(self):
        self.run_()
        w = self.world()
        w["rc"] = False
        self.world(w)
        self.kill("aaaa0001")
        self.kill("cccc0003")
        self.run_()
        out = self.run_()
        self.assertIn("revival FAILED Mesh", out)
        self.assertEqual(len(self.calls("--bg")), 1)          # the worker was not tried: it would hang too
        self.assertEqual(self.calls("stop")[0]["argv"], ["stop", "aaaa0001"])
        self.assertEqual(self.titles(), ["grant Full Disk Access to Claude"])
        self.assertIn(os.path.join(self.d, "claude"), Ntfy.alerts[0]["body"])
        for _ in range(3):                                     # no loop
            self.run_(LEAD_KEEPER_RETRY_HOURS="0")
        self.assertEqual(len(self.calls("--bg")), 1)
        self.assertEqual(len(Ntfy.alerts), 1)
        # a new binary lifts the hold
        w = self.world()
        w["rc"] = True
        self.world(w)
        os.rename(os.path.join(self.d, "claude"), os.path.join(self.d, "claude2"))
        os.symlink(os.path.join(self.d, "claude2"), os.path.join(self.d, "claude"))
        out = self.run_()
        self.assertIn("hold lifted", out)
        self.assertEqual(len(self.calls("--bg")), 3)

    def test_clear_lifts_the_hold(self):
        self.run_()
        w = self.world()
        w["rc"] = False
        self.world(w)
        self.kill("aaaa0001")
        self.run_()
        self.run_()
        w = self.world()
        w["rc"] = True
        self.world(w)
        self.run_()
        self.assertEqual(len(self.calls("--bg")), 1)
        self.assertIn("cleared everything", self.run_("--clear"))
        self.run_()
        self.run_()
        self.assertEqual(len(self.calls("--bg")), 2)

    def test_a_failed_command_is_said_once_retried_later_and_its_recovery_closes_the_item(self):
        self.run_()
        self.kill("aaaa0001")
        json.dump({"projects": {self.cwd: {"hasTrustDialogAccepted": True}}},
                  open(os.path.join(self.home, ".claude.json"), "w"))
        os.rename(os.path.join(self.d, "claude"), os.path.join(self.d, "claude.real"))
        open(os.path.join(self.d, "claude"), "w").write(
            "#!/bin/sh\n[ \"$1\" = --bg ] && { echo 'Error: refused'; exit 1; }\nexec %s \"$@\"\n"
            % os.path.join(self.d, "claude.real"))
        os.chmod(os.path.join(self.d, "claude"), 0o755)
        self.run_()
        self.run_()
        for _ in range(3):
            self.run_()
        self.assertEqual(self.titles(), ["Mesh did not come back"])
        os.rename(os.path.join(self.d, "claude.real"), os.path.join(self.d, "claude"))
        self.run_(LEAD_KEEPER_RETRY_HOURS="0")
        self.run_()
        self.assertEqual(self.titles(), ["Mesh did not come back", "Mesh is back"])
        done = [k for k in self.kapwa() if k[0] == "done"]
        self.assertEqual(done[0][1], "4f2a9")                  # closes the item it opened
        self.assertEqual(len([k for k in self.kapwa() if k[0] == "say" and "--t" in k]), 1)

    def test_a_lead_dying_twice_in_a_day_is_said_once(self):
        self.run_()
        for _ in range(3):
            self.kill("aaaa0001")
            self.run_()
            self.run_()
            self.run_()
        self.assertEqual(len(self.calls("--bg")), 3)
        self.assertEqual(self.titles(), ["lead Mesh died twice in 24h"])

    def test_unreadable_list_judges_nothing_and_says_so_once(self):
        self.run_()
        w = self.world()
        w["broken"] = True
        self.world(w)
        for _ in range(8):
            self.run_(LEAD_KEEPER_UNREADABLE_RUNS="3")
        self.assertEqual(self.revivals(), [])
        self.assertEqual(self.titles(), ["lead-keeper is blind"])

    def test_dry_run_changes_nothing(self):
        self.run_()
        self.kill("aaaa0001")
        before = self.state()
        out = self.run_("--dry-run")
        self.assertIn("Mesh                           DEAD", out)
        self.assertIn("would run, from %s" % self.cwd, out)
        self.assertIn("Monica                         up", out)
        self.assertEqual(self.revivals(), [])
        self.assertEqual(self.state(), before)
        self.assertEqual(Ntfy.alerts, [])


if __name__ == "__main__":
    unittest.main()
