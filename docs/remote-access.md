# Getting in, when the way in is what broke

*Written 2026-09-21, the night a tunnel restart locked us out of a machine
in somebody else's house and took a live site down with it. Everything here
is a consequence of that evening rather than a best practice copied from
somewhere.*

## What happened, in four lines

`ssh doug-mini` does not reach that Mac directly. Its `ProxyCommand` runs
`cloudflared access ssh --hostname ssh-mini.homesforsalebymonica.com`, so
every byte goes through **the cloudflared connector running on that Mac** —
the same one serving `monica.homesforsalebymonica.com` and eight other names.

A restart of that connector was issued over that ssh session as
`launchctl bootout` then `launchctl bootstrap`. The bootout severed the
connection carrying the command; the bootstrap half never ran. A job booted
*out* of its domain is no longer managed by launchd, so `KeepAlive` — which
answers "the process died" — has nothing to say about "the job is gone".

Ten hostnames stayed down until somebody could walk to the machine.

## The rule

**Never `bootout` a service that is carrying your connection.** Use
`launchctl kickstart -k`, which restarts a job that stays loaded, so losing
the connection mid-way costs nothing. When the plist itself has changed and
a full reload is unavoidable, hand both halves to a process that outlives
your session (`tunnels` does this itself now — see `launchd.rs::restart`).

## The shape to aim for

Every machine wants **more than one way in, failing for different reasons.**
The mini has always had three and has never been a problem; Doug's mini had
one and is why this page exists.

| path | independent of | starts at |
|---|---|---|
| Tailscale | Cloudflare, public DNS, the sites | boot (system daemon) |
| cloudflared ssh hostname | the LAN, your location | login (user agent) |
| `.local` on the LAN | the internet entirely | boot (mDNS) |

Public serving stays on cloudflared. Tailscale is for reaching the machines,
not for publishing anything.

## Installing the second way in

Per machine, once. Needs a password and a browser, so it cannot be done by
an agent.

```sh
brew install --cask tailscale
sudo tailscale up               # approve the machine in the browser
tailscale status                # it should list every machine you have added
```

Then, in the admin console, for **every machine you cannot walk to**:

- **Machines → <machine> → Disable key expiry.** Node keys expire after 180
  days by default. A remote machine whose key expires drops off the tailnet
  silently — which is the exact failure this is insurance against, on a
  timer. This is the one setting that turns the insurance into a time bomb
  if skipped.
- Turn on **Screen Sharing** (System Settings → General → Sharing) and reach
  it over the tailnet. ssh cannot fix a Mac sitting at a login screen; a
  screen can.

Deliberately **not** used: `tailscale up --ssh`, which replaces sshd's
authentication with tailnet identity. It is good, and it makes your
transport and your authentication the same system — the same class of
coupling that caused the evening above. Plain sshd, reached over the
tailnet, keeps them independent.

## `~/.ssh/config`, after

Friendly names point at the tailnet; the old routes stay, named for what
they are, because one day one of them is the way back in. Fill in the
tailnet names from `tailscale status` once the machines are on it.

```sshconfig
# the default path: works home or away, up before anyone logs in
Host macmini mini
  HostName macmini.<your-tailnet>.ts.net
  User felixflores

Host doug-mini
  HostName doug-mini.<your-tailnet>.ts.net
  User douggraham

# fallback: the tunnel. Kept on purpose — it fails for different reasons.
Host cloudflare-mini
  HostName ssh-mini.felixflor.es
  User felixflores
  ProxyCommand /opt/homebrew/bin/cloudflared access ssh --hostname %h

Host cloudflare-doug-mini
  HostName ssh-mini.homesforsalebymonica.com
  User douggraham
  ProxyCommand /opt/homebrew/bin/cloudflared access ssh --hostname %h

# fallback: the LAN. The only one that survives the internet being out.
Host mini-lan
  HostName felixs-mac-mini.local
  User felixflores
```

## The watchdog

`~/.local/bin/tunnel-watchdog.sh` bootstraps any launchd job in its list
that is not loaded. It exists for the one case `KeepAlive` cannot cover: a
job that was booted out rather than a process that died.

It has to run as a **LaunchDaemon**, not a user agent, or it inherits the
weakness it is meant to cover — a user agent only loads at login, and the
machine that needs saving may be sitting at a login screen. Installing it
needs a password:

```sh
sudo cp ~/.local/bin/tunnel-watchdog.sh /usr/local/bin/tunnel-watchdog.sh
sudo tee /Library/LaunchDaemons/com.dorkyrobot.tunnel-watchdog.plist >/dev/null <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.dorkyrobot.tunnel-watchdog</string>
  <key>ProgramArguments</key><array>
    <string>/bin/sh</string>
    <string>/usr/local/bin/tunnel-watchdog.sh</string>
    <string>com.cloudflare.cloudflared-doug-mini</string>
  </array>
  <key>StartInterval</key><integer>300</integer>
  <key>RunAtLoad</key><true/>
  <key>StandardOutPath</key><string>/var/log/tunnel-watchdog.log</string>
  <key>StandardErrorPath</key><string>/var/log/tunnel-watchdog.log</string>
</dict></plist>
PLIST
sudo launchctl bootstrap system /Library/LaunchDaemons/com.dorkyrobot.tunnel-watchdog.plist
```

Give it the labels that matter on that machine. Proven against the real
failure before it was written down: a job killed comes back by `KeepAlive`;
a job booted out does not, and the watchdog returns it within one interval.

## Checking it actually works

Do this once, on purpose, on a machine you are standing next to — an
untested backdoor is a rumour.

1. `ssh <machine>` over the tailnet. Then `launchctl bootout` its cloudflared
   job, and ssh in again over the tailnet: still fine, because the two do not
   share a path. Put it back with `launchctl bootstrap`.
2. Unplug the machine's ethernet or turn off its Wi-Fi briefly, and confirm
   the LAN path and the tailnet path fail differently rather than together.
3. Reboot it and confirm it comes back **without anyone logging in**. If it
   does not, the tunnel is still a user agent and that is the thing to fix.
