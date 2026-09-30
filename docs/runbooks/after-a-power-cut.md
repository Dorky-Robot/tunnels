# Bring a box back after a power cut, and prove it

**Use when:** a box, or a whole house, lost power or rebooted by itself (a
macOS update counts), and its sites or its ssh are not answering.
**Don't use when:** the box is up and one site is down: that is the site's
own runbook, or `tunnels doctor` on its box.
**Scope:** read-only, except the kickstarts in step 4, which only start what
a plist already says should run.
**Needs:** any other mesh box that is up; ideally one in the same house.

A box comes back by itself only if every layer does, in this order. Each
step checks one layer; stop at the first that fails, because everything
after it depends on it.

| layer | comes up at | what breaks it |
|---|---|---|
| power on | the cut ending | `autorestart` off: it stays off until someone presses the button |
| disk unlock | boot | FileVault on: it waits at a password prompt nobody sees |
| Tailscale | boot, no login | Tailscale as an app instead of the daemon; key expired |
| root watchdog, root cloudflared | boot, no login | not installed (true on most boxes today) |
| login session | auto-login, or a person | no auto-login: every user agent waits for someone |
| tunnels agent, cloudflared agents, apps | the login | macOS 27 leaves user agents loaded at `runs = 0` |

## 1. Which boxes are back (from any box that is up)

```sh
tailscale status                     # online / offline, per box
```

For each box you care about, first the production ones (`dorkyrobot1` for
everyday.vet, `doug-mini` for Monica):

```sh
ssh -o ConnectTimeout=10 <box> true && echo tailnet ok
ssh -o ConnectTimeout=10 <box>-lan true && echo lan ok      # same house only
```

**If neither answers:** the box is off, at a FileVault prompt, or its
Tailscale key expired. None of that is fixable from here. Ask a person at
the box to press the power button and log in, and note in the History
section which layer it was. `tailscale status` shows "offline, last seen …";
key expiry shows in the admin console.

## 2. The box itself will come back next time

Over `ssh <box>`, then `export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH`:

```sh
fdesetup status                                    # FileVault is Off.
pmset -g | grep -E '^ autorestart '                # autorestart 1
launchctl print system/sh.brew.tailscale | grep -m1 'state ='   # state = running
tailscale status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"].get("KeyExpiry","no expiry"))'
stat -f %Su /dev/console                           # a user, not root: someone is logged in
defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser   # who logs in by itself
```

- FileVault **On**, or `autorestart 0`, or no auto-login on a box nobody
  walks up to: the box will need a person after every cut. It is Felix's
  call to change (`sudo pmset -a autorestart 1`; FileVault off is a
  security trade). Write it down; don't change it at 3am.
- A `KeyExpiry` date: that box drops off the tailnet on that date. Disable
  key expiry for it in the Tailscale admin console (a person's job). On
  2026-09-30 every box still had one, in March 2027 (kapwa 16154).
- Tailscale not a system daemon (`sh.brew.tailscale` missing): it waits for
  a login. See [add-a-machine.md](add-a-machine.md), step 1.

## 3. The watchdog

```sh
launchctl print system/com.dorkyrobot.tunnel-watchdog | grep -E '^\s+(state|runs) ='
tail -5 /var/log/tunnel-watchdog.log      # silent when all is well
```

`Could not find service`: not installed. As of 2026-09-30 no box has it
loaded, so this is expected, not new; it means step 4 is on you. Installing
it needs sudo: `docs/remote-access.md`, "The watchdog".

## 4. Tunnels, the agent, and apps

```sh
tunnels agent status          # "agent: loaded", a last pass within ~2 min
tunnels tunnel list           # every tunnel: STATE loaded, a PID
tunnels doctor                # only the problems
```

`doctor` findings that were there before the cut are not yours to fix now:
on 2026-09-30 every box reported the orphan tunnel `mac-sara`. Compare with
another box that didn't lose power.

Then list user agents that are loaded but never started (the macOS 27
`runs = 0` strand) or that should always run and don't:

```sh
for p in ~/Library/LaunchAgents/*.plist; do l=$(basename "$p" .plist)
  s=$(launchctl print gui/$(id -u)/$l 2>/dev/null) || continue
  echo "$s" | grep -q '^	state = running' && continue
  ka=$(plutil -extract KeepAlive raw -o - "$p" 2>/dev/null)
  echo "$s" | grep -q '^	runs = 0$' && echo "stranded (runs = 0)  $l"
  [ "$ka" = true ] && echo "KeepAlive but down  $l"
done; true
```

(The tab in `'^	state'` is a real tab; launchctl indents with tabs.)

Start each one it names:

```sh
launchctl kickstart gui/$(id -u)/<label>
```

A bare `kickstart` (no `-k`) never touches a running process, and none of
these carry your ssh, which runs over the tailnet. Or do all of them at once
with the watchdog script, as yourself, no sudo; it only starts what a plist
declares:

```sh
sh ~/Projects/dorky_robot/tunnels/scripts/tunnel-watchdog.sh; tail -20 ~/Library/Logs/tunnel-watchdog.log
```

**Check:** run the loop again; it prints nothing, or only things you know
are dead on purpose (Felix's keep/kill list). A tunnel that is up but stuck
is `tunnels tunnel restart <name>`, which is safe over the ssh it carries.

Root cloudflared (doug-mini, once its visit is done):
`sudo launchctl print system/com.cloudflare.cloudflared-<name>`. The root
watchdog brings it back; by hand it is
`sudo launchctl kickstart system/com.cloudflare.cloudflared-<name>`.

## 5. Prove it from outside

From a different box, every hostname the fleet routes, grouped by the box
that serves it:

```sh
tunnels route list --json | python3 -c 'import json,sys
for r in json.load(sys.stdin):
    if r["active"]: print(r["machine"], r["host"])' | sort -u | while read m h; do
  printf '%-12s %-45s %s\n' "$m" "$h" "$(curl -s -o /dev/null -m 10 -w '%{http_code}' "https://$h/")"
done
```

Done when the box's rows are 200/30x, as they were before. 502 or 530 on
one host means its app (step 4) is down; every host on a box failing means
the tunnel. Then all three ways in:

```sh
ssh <box> true && ssh cloudflare-<box> true && echo both paths ok
```

## Stop and ask a person if

- the box doesn't answer on any path after 15 minutes;
- a production app (everyday.vet on dorkyrobot1, Monica's sites on
  doug-mini) is still down after step 4, since restarting those has its own
  rules (doug-mini: `consulting/monica-admin` docs, and the Monica lead);
- `tunnels doctor` says a tunnel has no connectors while `tunnel list` says
  it is running: that is Cloudflare or the token, not the power cut.

## Undo

Nothing to undo: the only changes are starting jobs that were meant to run.

## History

- 2026-09-21: a tunnel restarted as bootout + bootstrap over its own ssh
  stranded doug-mini for hours. Rule 1 of the [index](README.md).
- 2026-09-23: the macOS 27 update rebooted doug-mini; auto-login worked, but
  every user agent sat at `runs = 0` for 14.5 hours. Step 4's loop finds
  exactly that.
- 2026-09-30: written. Steps 1–5 run read-only against dorkyrobot2, the mini
  and doug-mini; step 4's loop found the mini's known-dead ollama agent and
  nothing else.
