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
- Turn on a screen (System Settings → General → Sharing) and reach it over
  the tailnet. ssh cannot fix a Mac sitting at a login screen; a screen can.

  Which toggle is an open question: the fleet today runs **Remote
  Management**, not Screen Sharing, and the Sharing pane will not run both —
  Remote Management takes over `screensharingd`. Follow the fleet rather than
  this line (kapwa 9aa73). Either way it answers on 5900.

  Reach it the tailnet way — the port is simply there, no forward, no tunnel:

  ```sh
  desktop-felix-mini          # or: desktop felix-mini, desktop --list
  ```

  `desktop` (installed by `mesh/install.sh`) walks the same three paths this
  document keeps for ssh, tailnet first, and only the tunnel fallback leaves
  anything running. By hand it is `open vnc://felixs-mac-mini:5900`.

  Do **not** build an `ssh -L 5901:localhost:5900` tunnel for this. That was
  the recipe before the tailnet existed, when the only way in was cloudflared
  and a forward inside the ssh session was the only way to carry a screen. It
  still works, so it is easy to keep reaching for and never notice it is two
  commands and a spare port solving a problem you no longer have. And never
  route 5900 — or ARD's 3283 — through cloudflared: those hostnames have no
  Access policy in front of them, so a tunnel ingress would publish your
  screen to anyone who knows the name.

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

`scripts/tunnel-watchdog.sh` covers the two ways a job can be
loaded-looking but dead, neither of which `KeepAlive` sees:

- **booted out** (2026-09-21): the job is gone from its domain, so launchd
  no longer manages it. The watchdog bootstraps any cloudflared agent, and
  the tunnels agent, that is not loaded.
- **loaded, never started** (2026-09-23): after the macOS 27 upgrade every
  agent that starts at load sat at `runs = 0`, `needs LWCR update`, for
  fourteen hours, the tunnel included. The watchdog kickstarts any loaded
  agent, tunnel or not, whose plist says it should be running (`KeepAlive`
  true, or `RunAtLoad` with no runs) and is not.

Since tunnels 0.16 the agent (`tunnels agent install`) does the first job
for tunnels, and `mesh/install.sh` installs the agent instead of the user
watchdog; `tunnels agent install` removes the user watchdog. But the agent is
itself a user LaunchAgent: on 2026-09-23 it sat at `runs = 0` with the rest,
and a user agent needs a login. So on a machine nobody can walk up to
(doug-mini), install the watchdog as a **root LaunchDaemon** as well. It is
there at boot, system daemons came through the upgrade, and run as root the
script looks after whoever is logged in at the console — the tunnels agent
included. `tunnels agent install` does not touch it: it lives in
`/Library/LaunchDaemons`, in the `system` domain. Needs a password, at the
machine or over `ssh <name>` (the tailnet, not the tunnel):

```sh
cd ~/Projects/dorky_robot/tunnels && git pull --ff-only
sudo install -m 755 scripts/tunnel-watchdog.sh /usr/local/bin/tunnel-watchdog.sh
sudo install -m 644 mesh/com.dorkyrobot.tunnel-watchdog.daemon.plist \
  /Library/LaunchDaemons/com.dorkyrobot.tunnel-watchdog.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.dorkyrobot.tunnel-watchdog.plist
```

Check it: `sudo launchctl print system/com.dorkyrobot.tunnel-watchdog`
shows `runs` climbing every minute, and `/var/log/tunnel-watchdog.log`
says what it did (it is silent when all is well). To make it run now:
`sudo launchctl kickstart system/com.dorkyrobot.tunnel-watchdog`.

As root, it also looks after cloudflared tunnels that run as system daemons
(`/Library/LaunchDaemons/com.cloudflare.cloudflared*.plist`), even with
nobody logged in. It bootstraps one that was booted out and kickstarts one
that is loaded but not running. It never loads a LaunchAgent whose label has
such a daemon; that agent is a leftover.

install.sh never touches the daemon copy: after the script changes, re-run
the `install` line for it; the next run picks it up. Where the agent is too
old to exist, install.sh still puts the watchdog in as a user agent that
fires by the clock (`StartCalendarInterval`): on 2026-09-23 those were the
only agents that ran.

Proven against the real failures before it was written down: a job killed
comes back by `KeepAlive`; a job booted out does not, and the watchdog
returns it within one interval; on 2026-09-23 one run of it brought seven
stranded agents up. The root path is not yet proven on a real upgrade.

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
   doug-mini's tunnel is a root LaunchDaemon for exactly this reason
   (2026-09-29), with the same label as the old agent. tunnels manages it in
   the `system` domain.

## The mesh, set up identically

Every machine carries the same setup, byte for byte, from `mesh/`. The
machines and GitHub accounts are listed once, and the rest is generated:

| edit this | what it is |
|---|---|
| `mesh/machines` | every machine, one line: name, tailnet host, login, address, tunnel, vnc port, GitHub account, aliases |
| `mesh/accounts` | every GitHub account a machine can push as: its key file, its git author, its ssh names |
| `mesh/hostkeys/<name>` | that machine's `/etc/ssh` host keys, read from the machine itself |
| `mesh/keys/<name>.pub` | that machine's `id_mesh_ed25519.pub` |

`sh mesh/build` turns those into the files below and `sh mesh/build --check`
says whether what is committed is what they make; install.sh refuses to run
if it is not.

| on every machine | from | what it is |
|---|---|---|
| `~/.ssh/config.d/mesh.conf` | machines | every box, reachable three ways, by the same names everywhere |
| `~/.ssh/config.d/mesh_known_hosts` | hostkeys/ | each box's host keys, under every name it answers to |
| `~/.ssh/config.d/github.conf` | accounts | every GitHub account, by alias, with its key |
| `~/.ssh/config.d/github_known_hosts` | GitHub | GitHub's published host keys (`build --github-hostkeys` refreshes them, checked against GitHub's published fingerprint) |
| `~/.ssh/authorized_keys`, between markers | keys/ | every machine's mesh key; take one out of `keys/` and the next install everywhere stops letting it in |
| `~/.local/bin/github-key-setup` | | this machine's own GitHub key, and git set up to push and sign with it |
| `~/.local/bin/desktop` + `desktop-<name>` | machines | a screen in one word; reads the installed `machines` |
| the tunnels agent (`tunnels agent install`) | | keeps this machine's tunnels in line with the fleet file and brings back any that were booted out; `tunnel-watchdog.sh` instead, on a tunnels older than 0.16 |

Each machine's own `~/.ssh/config` keeps whatever it had and gains one line,
`Include ~/.ssh/config.d/*.conf`, placed after any Includes already at the
top — an Include written below a `Host` line would be scoped to that host
alone. GitHub blocks there are removed, since `github.conf` names the same
keys; older mesh blocks are listed, and removed with `install.sh --tidy`
(they are shadowed, but `IdentityFile` and `LocalForward` add up across every
block that matches). Anything it edits is copied to `~/.ssh/backups/` first.
`sh mesh/install.sh` does all of it and is safe to re-run.

**Change it here, then push it everywhere.** Edit the inventory, `sh mesh/build`,
commit, and `sh mesh/rollout` from any machine in the mesh: it copies `mesh/`
to every other box over the mesh, runs install.sh there, and makes and adds
each one's GitHub key (below). `sh mesh/rollout mac2024` does just one. A copy edited in place on
one box is how one machine quietly stops reaching another while the rest look
fine.

Two things mesh.conf does on purpose, both learned the first time it went out:

- **Every direct path says `ProxyCommand none`.** ssh takes the first value
  of each option, but only for options a block actually sets — so a box's
  older `Host doug-mini` block with a ProxyCommand leaked into the tailnet
  path and sent it through cloudflared. Two of sixteen paths failed until the
  shared file pinned it.
- **The tunnel's ProxyCommand finds cloudflared on PATH**, because mac2019 is
  Intel and keeps Homebrew in `/usr/local`, not `/opt/homebrew`.

### GitHub

`sh mesh/rollout` does all of this for every machine at once, adding the keys
to GitHub itself with `gh ssh-key add` (on the machine it runs from only, which
asks once for the two scopes that allows); `--prune` also deletes older keys on
GitHub titled for a machine. By hand, on one machine, it is:

One key per machine per account, made on that machine by `github-key-setup`
and never copied: retiring a machine is deleting one key on GitHub. It is an
SSH key rather than a `gh auth login` because, on 2026-09-23, every new gh
login revoked the previous machine's token, and pushes died one box at a time.

- `github-key-setup` makes the key for this machine's account (the `github`
  column), prints it, and waits while you add it at
  https://github.com/settings/keys **twice** — as an Authentication Key and
  again as a Signing Key — then swaps it in, sets git's author, turns on
  commit signing, and proves pull and a dry-run push. Until GitHub takes the
  new key it sits at `<key>.new`, so a machine is never without a working one.
- `github-key-setup --check` proves it again; `--clean` deletes the key it
  replaced (kept in `~/.ssh/backups/` until then).
- `~/.ssh/allowed_signers` is every signing key GitHub lists for the account,
  so `git log --show-signature` verifies commits from every machine.
- Dorky-Robot remotes written as `https://github.com/Dorky-Robot/…` go over
  the key too (`url.git@github.com:Dorky-Robot/.insteadOf`), so no machine
  needs a gh token to push.

**Another account** is a line in `mesh/accounts` with its own alias
(`github.com-<account>`) and key file, then `sh mesh/build` and install. On a
machine that should use it, `github-key-setup <account>`. A repo picks the
account by the alias in its remote: `git@github.com-nerdnest:org/repo.git`.

### Adding a machine

1. On the new machine, at its screen: `sh join <name> <host>` (`mesh/join`).
   It names the box, turns on Remote Login, makes `~/.ssh/id_mesh_ed25519`
   and Taildrops the public halves to dorkyrobot2. It refuses where a mesh
   key already exists, so it cannot break a machine already in the mesh.
2. Copy that `.pub` into `mesh/keys/<name>.pub`, and its host keys —
   `cat /etc/ssh/ssh_host_ed25519_key.pub /etc/ssh/ssh_host_rsa_key.pub` —
   into `mesh/hostkeys/<name>`. **Read them from the machine itself** over a
   path you already trust (its screen, or a machine already in the mesh on the
   same LAN), never from `ssh-keyscan` alone: that is the step that makes
   every later connection trustworthy.
3. Add its line to `mesh/machines`: a free vnc port, and its GitHub account
   (or `-`). A note about the box goes on comment lines directly above it.
4. `sh mesh/build`, commit, push.
5. `sh mesh/rollout` — every machine learns the new box, the new box learns
   them, and it gets its GitHub key.
6. Run the matrix in the next section from each one.

A machine the mesh should reach but that should reach nothing (a laptop that
travels, someone else's Mac) gets `mesh/keys/<name>.none`, saying why, in
place of its `.pub`: its key goes into no machine's `authorized_keys`.
Its tunnels agent is kept off `[policy] remote_from` in the fleet file, the
same way: it takes the fleet from the machines on that list, nobody takes
the fleet from it, and `tunnels` refuses to edit the fleet there. Add it to
`[machines.<name>]` from a machine on the list (`tunnels fleet edit`), then
`tunnels fleet join <host of a listed machine>` on it; any route or tunnel of
its own is set from a listed machine (`tunnels tunnel adopt|create|assign
… --machine <name>`, then `tunnels route add` there).

Taking one out is the reverse: delete its line, its `keys/` and `hostkeys/`
files, build, commit, install everywhere, and delete its key on GitHub.

### Proving it

From each machine, every other machine, by every path. The last run
(2026-09-21): tailnet 16/16, tunnel 16/16, LAN within each house.
