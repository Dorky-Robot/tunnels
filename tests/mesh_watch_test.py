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
        json.dump({"routes": []}, open(fleet, "w"))
        conf = os.path.join(self.dir, "watch.conf")
        with open(conf, "w") as f:
            f.write("extra %s/up   always up\n" % self.base)
            f.write("extra %s/flaky the one we break\n" % self.base)
            f.write("expect %s Monica\n" % self.base.split("//")[1])
        self.env = dict(os.environ, MESH_WATCH_STATE=os.path.join(self.dir, "state"),
                        MESH_WATCH_ENV=os.path.join(self.dir, "none"), MESH_WATCH_CONF=conf,
                        MESH_WATCH_FLEET=fleet, MESH_WATCH_MACHINES=os.path.join(self.dir, "none"),
                        MESH_WATCH_CANARY=self.base + "/up", MESH_WATCH_RETRY_GAP="0",
                        MESH_WATCH_NTFY=self.base + "/topic")

    def watch(self, *args):
        p = subprocess.run([SCRIPT, "--no-machines", *args], env=self.env, capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        return p.stdout

    def incidents(self):
        p = os.path.join(self.dir, "state/incidents.jsonl")
        return [json.loads(l)["event"] for l in open(p)] if os.path.exists(p) else []

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
        self.watch()
        self.assertEqual(len(Site.alerts), 2)
        self.assertIn("back", Site.alerts[1]["title"])
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
        self.assertIn("2 checks down", Site.alerts[0]["title"])

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
        self.assertIn("stopped", Site.alerts[0]["title"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
