#!/usr/bin/python3
"""scripts/mesh-watch.py against a local site and a fake ntfy.

    /usr/bin/python3 tests/mesh_watch_test.py

Nothing leaves the machine: the checked site, the canary and the alert
endpoint are all servers this test runs on 127.0.0.1.
"""
import http.server
import json
import os
import subprocess
import tempfile
import threading
import unittest

REPO = os.path.dirname(os.path.dirname(os.path.realpath(__file__)))
SCRIPT = os.path.join(REPO, "scripts/mesh-watch.py")


class Site(http.server.BaseHTTPRequestHandler):
    status = {"/up": 200, "/flaky": 200}
    body = {"/up": b"hello Monica", "/flaky": b"hello Monica"}
    alerts = []

    def do_GET(self):
        if self.path == "/app":                  # an app sending you to sign in
            self.send_response(302)
            self.send_header("Location", "http://id.example.invalid/authorize?x=1")
            self.end_headers()
            return
        code = self.status.get(self.path, 200)
        self.send_response(code)
        self.end_headers()
        self.wfile.write(self.body.get(self.path, b"ok") if code < 500 else b"")

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        Site.alerts.append({"title": self.headers.get("Title"), "body": self.rfile.read(n).decode()})
        self.send_response(200 if self.path == "/topic" else 500)
        self.end_headers()

    def log_message(self, *a):
        pass


class MeshWatch(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Site)
        cls.base = "http://127.0.0.1:%d" % cls.srv.server_address[1]
        threading.Thread(target=cls.srv.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.srv.shutdown()

    def setUp(self):
        Site.alerts.clear()
        Site.status.update({"/up": 200, "/flaky": 200})
        Site.body["/flaky"] = b"hello Monica"
        self.dir = tempfile.mkdtemp()
        fleet = os.path.join(self.dir, "fleet.json")
        json.dump({"routes": [{"host": "ssh-x.example", "tunnel": "t", "service": "ssh://localhost:22"}],
                   "tunnels": {"t": {"machine": "boxa"}}}, open(fleet, "w"))
        self.fleet = fleet
        conf = os.path.join(self.dir, "watch.conf")
        with open(conf, "w") as f:
            f.write("extra %s/up   always up\n" % self.base)
            f.write("extra %s/flaky the one we break\n" % self.base)
            f.write("expect %s Monica\n" % self.base.split("//")[1])
        self.env = dict(os.environ, MESH_WATCH_STATE=os.path.join(self.dir, "state"),
                        MESH_WATCH_ENV=os.path.join(self.dir, "none"), MESH_WATCH_CONF=conf,
                        MESH_WATCH_FLEET=fleet, MESH_WATCH_MACHINES=os.path.join(self.dir, "none"),
                        MESH_WATCH_CANARY=self.base + "/up", MESH_WATCH_RETRY_GAP="0",
                        MESH_WATCH_NTFY=self.base + "/topic", MESH_WATCH_DRIFT_MIN="0",
                        MESH_WATCH_AGENTS=os.path.join(self.dir, "LaunchAgents"), HOME=self.dir)

    def watch(self, *args):
        p = subprocess.run([SCRIPT, "--no-machines", *args], env=self.env, capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def incidents(self):
        p = os.path.join(self.dir, "state/incidents.jsonl")
        return [json.loads(l)["event"] for l in open(p)] if os.path.exists(p) else []

    def test_an_installed_keeper_that_stops_running_is_an_incident(self):
        self.watch()                       # not installed: not watched
        self.assertEqual(Site.alerts, [])
        os.makedirs(os.path.join(self.dir, "LaunchAgents"))
        open(os.path.join(self.dir, "LaunchAgents/com.dorkyrobot.lead-keeper.plist"), "w").write("")
        beat = os.path.join(self.dir, ".local/state/lead-keeper/last-run")
        os.makedirs(os.path.dirname(beat))
        open(beat, "w").write("2026-01-01T00:00:00Z\n")
        self.watch()
        self.watch()
        self.assertEqual(len(Site.alerts), 1)
        self.assertIn("lead-keeper", Site.alerts[0]["body"])
        self.assertIn("last ran", Site.alerts[0]["body"])
        import datetime
        open(beat, "w").write(datetime.datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ\n"))
        self.watch()
        self.watch()
        self.assertEqual(len(Site.alerts), 2)
        self.assertIn("back", Site.alerts[1]["title"])

    def test_one_alert_per_incident_and_one_on_recovery(self):
        self.watch()
        self.assertEqual(Site.alerts, [])
        Site.status["/flaky"] = 502
        self.watch()                       # first failing run: not yet
        self.assertEqual(Site.alerts, [])
        self.watch()                       # second: the incident opens
        self.assertEqual(len(Site.alerts), 1)
        self.assertIn("HTTP 502", Site.alerts[0]["body"])
        self.watch()
        self.watch()                       # still down: no storm
        self.assertEqual(len(Site.alerts), 1)
        Site.status["/flaky"] = 200
        self.watch()                       # one pass is not yet a recovery
        self.assertEqual(len(Site.alerts), 1)
        self.watch()
        self.assertEqual(len(Site.alerts), 2)
        self.assertIn("back", Site.alerts[1]["title"])
        self.assertIn("down ", Site.alerts[1]["body"])
        self.watch()
        self.assertEqual(len(Site.alerts), 2)
        self.assertEqual(self.incidents(), ["opened", "recovered"])

    def test_a_blip_is_logged_not_alerted(self):
        Site.status["/flaky"] = 503
        self.watch()
        Site.status["/flaky"] = 200
        self.watch()
        self.watch()
        self.assertEqual(Site.alerts, [])
        self.assertEqual(self.incidents(), [])

    def test_the_content_check_catches_a_page_that_answers_wrong(self):
        Site.body["/flaky"] = b"<html>some parking page</html>"
        self.watch()
        out = self.watch()
        self.assertIn("lacks 'Monica'", out)
        self.assertEqual(len(Site.alerts), 1)

    def test_many_failures_in_one_run_are_one_message(self):
        Site.status.update({"/up": 500, "/flaky": 500})
        self.env["MESH_WATCH_CANARY"] = self.base + "/canary"
        self.watch()
        self.watch()
        self.assertEqual(len(Site.alerts), 1)
        self.assertIn("Outside the tunnels", Site.alerts[0]["body"])
        self.assertEqual(Site.alerts[0]["body"].count(" since "), 2)

    def test_an_offline_monitor_judges_nothing(self):
        Site.status["/flaky"] = 500
        self.env["MESH_WATCH_CANARY"] = "http://127.0.0.1:9/"
        for _ in range(3):
            p = subprocess.run([SCRIPT, "--no-machines"], env=self.env, capture_output=True, text=True)
            self.assertEqual(p.returncode, 3)
        self.assertEqual(Site.alerts, [])
        self.assertEqual(self.incidents(), [])

    def test_an_alert_that_cannot_be_sent_waits_for_the_next_run(self):
        self.env["MESH_WATCH_NTFY"] = self.base + "/broken"
        Site.status["/flaky"] = 500
        self.watch()
        out = self.watch()
        self.assertIn("1 alert(s) waiting", out)
        self.env["MESH_WATCH_NTFY"] = self.base + "/topic"
        Site.alerts.clear()
        out = self.watch()
        self.assertIn("0 alert(s) waiting", out)
        self.assertEqual(len(Site.alerts), 1)
        self.assertIn("down", Site.alerts[0]["title"])

    def test_heartbeat_alerts_once_when_the_monitor_is_unreachable(self):
        self.env.update(MESH_WATCH_RETRIES="0")
        bin_ = os.path.join(self.dir, "bin")
        os.makedirs(bin_)
        with open(os.path.join(bin_, "ssh"), "w") as f:     # every box unreachable
            f.write("#!/bin/sh\necho 'ssh: connect: Operation timed out' >&2\nexit 255\n")
        os.chmod(os.path.join(bin_, "ssh"), 0o755)
        env = dict(self.env, PATH=bin_ + ":/usr/bin:/bin")
        run = lambda: subprocess.run([SCRIPT, "heartbeat", "dorkyrobot2"], env=env, capture_output=True, text=True)
        for _ in range(4):
            self.assertEqual(run().returncode, 0)
        self.assertEqual(len(Site.alerts), 1, Site.alerts)
        self.assertIn("annot reach dorkyrobot2", Site.alerts[0]["body"])


    def test_a_flapping_host_is_one_incident(self):
        Site.status["/flaky"] = 502
        self.watch(); self.watch()          # opens
        for code in (200, 502, 200, 502, 200, 502):
            Site.status["/flaky"] = code
            self.watch()
        self.assertEqual(len(Site.alerts), 1)

    def test_a_fleet_that_cannot_be_read_falls_back_and_says_so(self):
        self.watch()                        # caches the fleet
        open(self.fleet, "w").write("not json")
        out = self.watch()
        self.assertIn("FAIL monitor:fleet", out)
        self.watch()
        self.assertEqual(len(Site.alerts), 1)
        self.assertIn("cannot read the fleet", Site.alerts[0]["body"])

    def fake_ssh(self, script):
        bin_ = os.path.join(self.dir, "bin")
        os.makedirs(bin_, exist_ok=True)
        with open(os.path.join(bin_, "ssh"), "w") as f:
            f.write("#!/bin/sh\n" + script)
        os.chmod(os.path.join(bin_, "ssh"), 0o755)
        env = dict(self.env, PATH=bin_ + ":/usr/bin:/bin", MESH_WATCH_RETRIES="0")
        return lambda: subprocess.run([SCRIPT, "heartbeat", "dorkyrobot2"], env=env, capture_output=True, text=True)

    def test_heartbeat_alerts_at_once_when_the_monitor_is_stale(self):
        run = self.fake_ssh("echo 2020-01-01T00:00:00Z ok\n")
        self.assertIn("FAIL", run().stdout)
        self.assertEqual(len(Site.alerts), 1)
        self.assertIn("not watching", Site.alerts[0]["title"])
        self.assertIn("get in: ssh dorkyrobot2", Site.alerts[0]["body"])

    def test_heartbeat_names_a_monitor_that_has_lost_its_internet(self):
        import datetime
        t = datetime.datetime.utcnow()
        fresh = t.strftime("%Y-%m-%dT%H:%M:%SZ")
        old = (t - datetime.timedelta(minutes=30)).strftime("%Y-%m-%dT%H:%M:%SZ")
        run = self.fake_ssh("echo %s offline %s\n" % (fresh, old))
        run()
        self.assertEqual(len(Site.alerts), 1)
        self.assertIn("no internet since", Site.alerts[0]["body"])
        run = self.fake_ssh("echo %s ok\n" % fresh)
        run()
        self.assertEqual(len(Site.alerts), 2)
        self.assertIn("watching again", Site.alerts[1]["title"])

    def test_the_monitor_writes_offline_into_its_heartbeat(self):
        self.env["MESH_WATCH_CANARY"] = "http://127.0.0.1:9/"
        subprocess.run([SCRIPT, "--no-machines"], env=self.env, capture_output=True)
        self.assertIn(" offline ", open(os.path.join(self.dir, "state/last-run")).read())

    def test_old_history_is_pruned_and_incidents_kept(self):
        st = os.path.join(self.dir, "state")
        os.makedirs(st)
        for n in ("checks-2001-01.jsonl", "incidents.jsonl"):
            open(os.path.join(st, n), "w").write("{}\n")
        self.watch()
        self.assertFalse(os.path.exists(os.path.join(st, "checks-2001-01.jsonl")))
        self.assertTrue(os.path.exists(os.path.join(st, "incidents.jsonl")))

    def test_a_200_that_is_an_error_page_is_caught_without_an_expect(self):
        with open(self.env["MESH_WATCH_CONF"], "w") as f:
            f.write("extra %s/flaky\n" % self.base)
        for body in (b"<html><title>502 Bad Gateway</title></html>", b"no healthy upstream",
                     b"<title>Welcome to nginx!</title>"):
            Site.body["/flaky"] = body
            self.assertIn("error page", self.watch())

    def test_a_sign_in_redirect_is_not_followed_to_the_id_host(self):
        with open(self.env["MESH_WATCH_CONF"], "w") as f:
            f.write("extra %s/app\n" % self.base)
        self.assertIn("0 failing", self.watch())   # id.example.invalid is never asked

    def test_429_means_up(self):
        Site.status["/flaky"] = 429
        self.watch(); self.watch()
        self.assertEqual(Site.alerts, [])

    def test_a_live_host_missing_from_the_fleet_is_checked_and_noted_quietly(self):
        live = os.path.join(self.dir, "live.json")
        json.dump({"ssh-x.example": "t", self.base.split("//")[1]: "t"}, open(live, "w"))
        self.env.update(MESH_WATCH_DRIFT_MIN="60", MESH_WATCH_LIVE=live)
        drift = os.path.join(self.dir, "state/drift.json")
        out = self.watch()
        host = self.base.split("//")[1]
        self.assertIn("http:" + host, out)        # the unlisted host is checked at once (https, so it fails here)
        with open(self.env["MESH_WATCH_CONF"], "a") as f:
            f.write("skip %s test server has no https\n" % host)
        for _ in range(3):                        # two more fetches: now it is drift
            d = json.load(open(drift)); d["fetched"] = "2000-01-01T00:00:00Z"; json.dump(d, open(drift, "w"))
            self.watch()
        notes = [a for a in Site.alerts if a["title"].startswith("note:")]
        self.assertEqual(len(Site.alerts), 1, Site.alerts)
        self.assertEqual(len(notes), 1)
        self.assertIn("not in the fleet file", notes[0]["body"])

if __name__ == "__main__":
    unittest.main(verbosity=2)
