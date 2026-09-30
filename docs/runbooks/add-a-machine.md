# Add a machine to the mesh

**Use when:** a new Mac should be reachable from every other box, by the
same names, and may run tunnels.
**Don't use when:** you only want to send it a file (Taildrop needs only
Tailscale), or it is a machine that should reach nothing: then it gets
`mesh/keys/<name>.none` instead of a key (`docs/remote-access.md`, "Adding
a machine").
**Scope:** the new Mac; this repo's `mesh/` inventory; `authorized_keys` on
every box; the fleet file.
**Needs:** someone at the new Mac's screen with its password (steps 1–3);
dorkyrobot2 (where `join` sends its keys); a box on the fleet's
`policy.remote_from` for step 6.

Pick two names before you start: `<name>`, the short one everybody types
(`mini`), and `<host>`, its tailnet name and `hostname -s` (`felixs-mac-mini`).

## At the new Mac's screen

1. **Tailscale as a system daemon**, so it is up at boot with nobody logged
   in, and never the App Store app:

   ```sh
   brew install tailscale cloudflared dorky-robot/tap/tunnels
   sudo brew services start tailscale
   sudo tailscale up                  # approve it in the browser
   ```

   **Check:** `launchctl print system/sh.brew.tailscale | grep -m1 'state ='`
   says `running`, and `tailscale status` lists the other boxes. Don't use
   `tailscale up --ssh`; plain sshd over the tailnet is the setup.

   Then in the Tailscale admin console, **Machines → `<host>` → Disable key
   expiry**. **Check:**

   ```sh
   tailscale status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["Self"].get("KeyExpiry","no expiry"))'
   ```

   prints `no expiry`. A date means the box silently drops off the tailnet
   on that date, 180 days out.

2. **It comes back after a power cut by itself.**

   ```sh
   sudo pmset -a autorestart 1
   fdesetup status                     # Off, or it waits at boot for a password
   ```

   plus auto-login for the account that runs its services (System Settings →
   Users & Groups), if nobody will be there to log in. FileVault and
   auto-login are Felix's call per box; write down which way it went.

3. **Join**, from a checkout of this repo (or a copy of `mesh/join`):

   ```sh
   sh mesh/join <name> <host>
   ```

   It names the box, turns on Remote Login, makes `~/.ssh/id_mesh_ed25519`
   and Taildrops the public halves to dorkyrobot2. It refuses on a box that
   already has a mesh key, so it can't break one already in the mesh.

## On dorkyrobot2, in a worktree of this repo

4. **Take in its keys.** They arrive in `~/Projects/inbox`. Put its mesh key
   in `mesh/keys/<name>.pub` and its host keys in `mesh/hostkeys/<name>`.
   Host keys must come from the machine itself (join sent them, or its
   screen), never from `ssh-keyscan` alone: they are what makes every later
   connection trustworthy.

5. **One line in `mesh/machines`**: name, host, login, tailnet IP, its
   `ssh-…` tunnel hostname or `-`, a free vnc port, its GitHub account or
   `-`, aliases. Never edit `mesh.conf` or `mesh_known_hosts`: they are
   generated.

   ```sh
   sh mesh/build && sh mesh/build --check      # build: up to date
   git add mesh && git commit                  # then land it on main and push
   sh mesh/rollout                             # every box learns it; it learns them
   ```

   **Check:** rollout ends with one line per machine. From dorkyrobot2 and
   from the new box, `ssh <name> true`, `ssh <name>-lan true` (same house)
   and, once step 7 routes one, `ssh cloudflare-<name> true`.
   **If rollout fails at the GitHub key step:** that step needs `gh` scopes
   on the box it runs from. The ssh setup has already landed; do the key by
   hand with `github-key-setup` on the new box.

## Tunnels

6. **Put it in the fleet, from a box on `policy.remote_from`.** A box off
   that list follows the fleet but may not edit it, and a new box is off it
   until you add it.

   ```sh
   tunnels fleet edit    # add [machines.<name>] host = "<host>"; add "<name>"
                         # to remote_from only if it should publish the fleet
   ```

   Then on the new box:

   ```sh
   tunnels fleet join <a listed box's host> --machine <name>
   tunnels token add <api-token>     # only if it should change Cloudflare itself
   tunnels agent install             # rollout already ran this; it is idempotent
   tunnels agent status              # loaded, same fleet serial as the others
   ```

7. **Its tunnel**, from a listed box: `tunnels tunnel create <alias>
   --account <acct> --machine <name>`. The new box's agent fetches the
   connector token with its own API token and starts it; without one, give
   it the token on its card in the web UI. Routes: `tunnels route add`.

8. **The root watchdog**, so stranded user agents are started after a
   reboot. Needs the password, on the box or over `ssh <name>`:

   ```sh
   cd ~/Projects/dorky_robot/tunnels && git pull --ff-only
   sudo install -d /usr/local/bin
   sudo install -m 755 scripts/tunnel-watchdog.sh /usr/local/bin/tunnel-watchdog.sh
   sudo install -m 644 mesh/com.dorkyrobot.tunnel-watchdog.daemon.plist \
     /Library/LaunchDaemons/com.dorkyrobot.tunnel-watchdog.plist
   sudo launchctl bootstrap system /Library/LaunchDaemons/com.dorkyrobot.tunnel-watchdog.plist
   ```

   (`bootstrap` is right here: the job carries no connection, and it isn't
   loaded yet.) **Check:** `sudo launchctl print
   system/com.dorkyrobot.tunnel-watchdog | grep runs` climbs every minute.

## Done when

- every box reaches it on every path, and it reaches every box;
- `tunnels agent status` on it shows the current fleet serial;
- it passes [after-a-power-cut.md](after-a-power-cut.md) steps 2–4. Better
  still, reboot it once with nobody at it and run step 5 from elsewhere.

## Stop and ask a person if

- `join` says the box already has a mesh key: you are on the wrong machine;
- the host keys that arrived don't match what the box's own screen shows;
- `tunnels fleet edit` refuses: you are on a box off `remote_from`.

## Undo

Delete its line in `mesh/machines` and its `keys/` and `hostkeys/` files,
`sh mesh/build`, commit, `sh mesh/rollout`; delete its key on GitHub;
`tunnels fleet edit` to remove `[machines.<name>]`; remove it from the
tailnet in the admin console.

## History

- 2026-09-26: join steps meant for a new Mac were typed into dorkyrobot2 and
  replaced its key; every box refused it. `join` now refuses where a key
  exists.
- 2026-09-30: written from `docs/remote-access.md` and the code. Proven
  read-only: `mesh/build --check`, the Tailscale and KeyExpiry checks on
  three boxes, `tunnels import --dry-run`, the `--help` of every tunnels
  command used. `join`, `rollout` and the sudo steps were not run.
