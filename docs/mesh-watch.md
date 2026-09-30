# mesh-watch: the mesh seen from outside

The tunnel-watchdog on each box asks launchd whether a job is loaded. That is
the inside view, and a tunnel can be loaded, running and serving 530s.
`scripts/mesh-watch.py` asks what a visitor asks: does the page come back, and
is it the right page? It reports. It fixes nothing.

| where | what | job |
|---|---|---|
| dorkyrobot2 | every 5 min: every hostname, every machine | `mesh/com.dorkyrobot.mesh-watch.plist` |
| mini | every 5 min: did dorkyrobot2's monitor run in the last 11 min? | `mesh/com.dorkyrobot.mesh-watch-heartbeat.plist` |

The two watch each other: the monitor also checks that the mini's heartbeat
has run lately (`MESH_WATCH_PEER=mini`), and checks the mini itself like any
other machine.

dorkyrobot2 was the least loaded of the always-on boxes when this was set up
(2026-09-30: load 1.3 on 12 cores, 74% memory free, 0.5 of 2 GB swap used;
the mini had load 1.9 on 8 cores, 8 GB of RAM, 4.9 of 6 GB swap used).
dorkyrobot1 is production and stays clean; mac2019 is being retired;
doug-mini is Doug's.

## What it checks

Nothing is listed by hand that can be derived.

- **Every route in the fleet file** (`tunnels fleet show --json`): `https://<host>/`,
  redirects followed. It passes on a 2xx final page that is not empty, is not
  a Cloudflare error page, is not an error or placeholder page by its title
  (502, "Welcome to nginx", "no healthy upstream"…), and contains the `expect`
  text if one is set.
  A redirect to sign-in at `id.<domain>` or `id-dev.<domain>` ends the check
  as a pass: the app answered. The id host is its own check, and following
  every app there would hit Pocket ID several times a run from one address,
  which it answers with 429. A 429 anywhere counts as up.
- **Every hostname Cloudflare really serves through a tunnel**, asked hourly
  (`tunnels cf get`: every tunnel's ingress and every tunnel CNAME, read-only).
  A live hostname the fleet file does not list is checked anyway. When two
  fetches in a row see the fleet and Cloudflare disagree, either way round,
  that is one low-priority note (`monitor:drift`), since nothing is down
  because of it. A failed fetch reports nothing: unknown is not missing.
- **Every machine in `mesh/machines`**: `tailscale ping`, `ssh <name> true`,
  and `ssh cloudflare-<name> true` where it has a tunnel. That last one is the
  check for the `ssh-*` routes. Not the agent's `:7630`, which the mac2019
  and doug-mini firewalls do not answer.
- **`mesh/watch.conf`** adds what neither file knows: hosts to skip and why,
  pages no route covers, and text a page must contain. An `extra` can be a
  site outside the tunnels or a second page on a routed host, such as the
  Everyday Vet desks' `/health`, which answers `{"dots":N,"ok":true,…}` only
  when the kita store opens; a lock or sign-in page still says "Everyday Vet",
  so the page check alone cannot tell. An extra on a routed host is filed
  under the machine that serves it. `expect` takes a host or an extra's exact
  URL, and the URL wins, so `/health` wants `"ok":true` while the page wants
  its text.

`scripts/mesh-watch.py --list` prints every check and every skip with its reason.

## Incidents

A failing check is retried twice, 15 s apart, in the same run. An incident
opens after two failing runs in a row (so within about ten minutes) and is
alerted once. It closes after two passing runs in a row, alerted once with
how long it was down, so a host that flaps stays one incident instead of an
alert every few minutes. Everything that opens in one run is one message,
grouped by the machine that serves it; when every check on a machine fails,
the message says the machine looks down instead of listing forty sites. Each
line says what failed and since when, and each machine gets its three ways in.

If the monitor cannot reach `www.cloudflare.com` it judges nothing that run:
a dead uplink on dorkyrobot2 is not forty dead sites. It writes that into its
heartbeat, and if it lasts past 11 minutes the heartbeat tells Felix, in one
message, that dorkyrobot2 is running but blind. An alert that cannot be sent
waits and goes with the next run.

If `tunnels fleet show` fails, the monitor checks the last fleet it could
read (`fleet.json`) and reports `monitor:fleet` as an incident of its own.

The heartbeat reads `~/.local/state/mesh-watch/last-run` on dorkyrobot2 over
the tailnet, then over Cloudflare. A heartbeat older than 11 minutes (two
missed runs) or missing is alerted at once: a monitor that dies is known
within about 12 minutes. Not reaching dorkyrobot2 by either path is alerted
after two runs, since the fault may be the mini's.

## History

Under `~/.local/state/mesh-watch/` on dorkyrobot2 (heartbeat files on the mini):

- `checks-YYYY-MM.jsonl`: one line per check per run (`ts id ok code ms tries detail`).
  About 15,000 lines (2 MB) a day, so about 60 MB a month; six months are kept.
- `incidents.jsonl`: `opened`, `recovered` and `retired` events, which is
  what an uptime board wants. Kept for good.
- `state.json`: open incidents, streaks, alerts waiting to be sent.
- `fleet.json`: the last fleet that could be read.
- `drift.json`: the last two hourly reads of what Cloudflare serves.
- `last-run`: the heartbeat, `<time> ok` or `<time> offline <since>`.

## Alerts

Alerts go to ntfy: `MESH_WATCH_NTFY=https://ntfy.sh/<topic>` in
`~/.config/mesh-watch/env` (mode 600) on both boxes. The topic is the only
secret, and it is not in git. Without it, alerts are logged as waiting and
nothing leaves the box. ntfy.sh is outside the mesh, so an alert about the
mesh does not depend on the mesh; Felix gets it on his phone through the ntfy
app, subscribed to that topic.

## Install (sudo, on each box)

Not done until the box choice and the channel are approved.

    # dorkyrobot2
    sudo cp mesh/com.dorkyrobot.mesh-watch.plist /Library/LaunchDaemons/
    sudo launchctl bootstrap system /Library/LaunchDaemons/com.dorkyrobot.mesh-watch.plist

    # mini
    sudo cp mesh/com.dorkyrobot.mesh-watch-heartbeat.plist /Library/LaunchDaemons/
    sudo launchctl bootstrap system /Library/LaunchDaemons/com.dorkyrobot.mesh-watch-heartbeat.plist

They are LaunchDaemons so they run after a power cut with nobody logged in,
and they run as the box's user (`UserName`) because the checks are that
user's ssh keys. Both run the script from the box's main checkout of this
repo, so landing on main and pulling updates them. To run one now:
`sudo launchctl kickstart system/com.dorkyrobot.mesh-watch`.

## Test

    /usr/bin/python3 tests/mesh_watch_test.py

Everything runs against servers on 127.0.0.1: one alert per incident, one on
recovery, a blip is not an incident, a wrong page fails, many failures are
one message, a flapping host is one incident, an unreadable fleet falls back
and says so, an offline monitor judges nothing and says so in its heartbeat,
an unsent alert waits, old history is pruned, an expect keyed by URL beats
the host's, a page on a routed host is filed under its machine, and the
heartbeat alerts once for a monitor that is stale, blind or unreachable.
