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
| `~/.local/bin/tunnel-watchdog.sh` + its agent | | finds this user's cloudflared agents and brings back any that were booted out |

Each machine's own `~/.ssh/config` keeps whatever it had and gains one line,
`Include ~/.ssh/config.d/*.conf`, placed after any Includes already at the
top — an Include written below a `Host` line would be scoped to that host
alone. GitHub blocks there are removed, since `github.conf` names the same
keys; older mesh blocks are listed, and removed with `install.sh --tidy`
(they are shadowed, but `IdentityFile` and `LocalForward` add up across every
block that matches). Anything it edits is copied to `~/.ssh/backups/` first.
`sh mesh/install.sh` does all of it and is safe to re-run.

**Change it here, then push it everywhere.** Edit the inventory, `sh mesh/build`,
commit, and run `mesh/install.sh` on each machine. A copy edited in place on
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

1. On the new machine: `ssh-keygen -t ed25519 -N "" -C "mesh:$(hostname -s)" -f ~/.ssh/id_mesh_ed25519`
2. Copy that `.pub` into `mesh/keys/<name>.pub`, and its host keys —
   `cat /etc/ssh/ssh_host_ed25519_key.pub /etc/ssh/ssh_host_rsa_key.pub` —
   into `mesh/hostkeys/<name>`. **Read them from the machine itself** over a
   path you already trust (its screen, or a machine already in the mesh on the
   same LAN), never from `ssh-keyscan` alone: that is the step that makes
   every later connection trustworthy.
3. Add its line to `mesh/machines`: a free vnc port, and its GitHub account
   (or `-`). A note about the box goes on comment lines directly above it.
4. `sh mesh/build`, commit, push.
5. `sh mesh/install.sh` on every machine — the others learn the new box, and
   the new box learns them. On the new one, then `github-key-setup`.
6. Run the matrix in the next section from each one.

Taking one out is the reverse: delete its line, its `keys/` and `hostkeys/`
files, build, commit, install everywhere, and delete its key on GitHub.

### Proving it

From each machine, every other machine, by every path. The last run
(2026-09-21): tailnet 16/16, tunnel 16/16, LAN within each house.
