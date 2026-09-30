# Rotate a tunnel token that leaked

**Use when:** a tunnel's connector token (the long `eyJ…` string cloudflared
runs with) was seen somewhere it shouldn't be: a transcript, a paste, `ps`
output, a plist in a backup.
**Don't use when:** it was a Cloudflare **API** token (`cfut_…`). Roll that
one in the Cloudflare dashboard, then give each box the new one from the web
UI (the account card's **rotate**) or with `tunnels token add <new>` and
`tunnels token rm <n>` on each box that had it.
**Scope:** [this Mac + cloudflare]. There is no preview and no undo:
from the moment `rotate` returns, every token issued before it is dead,
everywhere.
**Needs:** the tunnel's fleet alias; a box with an API token for the
tunnel's account; the tunnel's owner reachable over the tailnet.

## Before you start

1. **Name the tunnel and its owner.**

   ```sh
   tunnels route list | grep <alias>      # the hostnames it carries
   tunnels fleet show | grep -A4 '^\[tunnels.<alias>\]'   # machine = its owner
   ```

2. **Get onto the owner by a path the rotation can't cut.** Rotating
   restarts the owner's cloudflared. If your ssh runs through it, you lose
   the box.

   ```sh
   ssh -G <owner> | grep -i '^proxycommand'    # must print nothing
   ssh <owner>                                 # the tailnet; or <owner>-lan
   export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH
   ```

   Never `ssh cloudflare-<owner>` for this. On the owner, `tunnels token
   list` must show a token for the tunnel's account; if not, run the rotate
   from a box that has one and the owner's agent follows within a pass.

3. **Write down the before state** (no token is printed by any of these):

   ```sh
   date -u +%FT%TZ                                  # T: the rotation time is after this
   tunnels tunnel list                              # its local name, PID, TOKEN column
   tunnels cf get '/accounts/{account:<acct>}/cfd_tunnel/{tunnel:<alias>}/connections' \
     | grep -E '"(client_id|opened_at)"'
   ```

   and the baseline of every hostname it carries (the loop in the
   [index](README.md), filtered to this box). `<acct>` is the fleet account
   alias, `dorkyrobot` or `felixflor`.

4. **Every copy of this token in the mesh**, from the owner's agent:

   ```sh
   curl -s http://127.0.0.1:7630/api/accounts | python3 -c 'import json,sys
   for acct in json.load(sys.stdin):
       for m,c in acct["tunnels"]:
           if c.get("alias")==sys.argv[1]: print(m, c["name"], c["state"], "token_file=%s" % c["token_file"], "current=%s" % c["current"])
       if acct.get("unknown"): print("unknown, agent not answering:", acct["unknown"])' <alias>
   ```

   Expect the owner, plus any box that keeps a spare copy. `tunnels` can't
   see copies it didn't write: on the owner also look for file names (not
   contents): `ls ~/.cloudflared/`, and
   `grep -l -- '--token' ~/Library/LaunchAgents/com.cloudflare.cloudflared-*.plist`.

5. **Is the tunnel a root LaunchDaemon?**
   `ls /Library/LaunchDaemons | grep cloudflared`. If it is (doug-mini, once
   its visit is done), plan for the sudo step in step 3 below.

## Steps

1. **Rotate**, on the owner:

   ```sh
   tunnels tunnel rotate <alias>
   ```

   **Check:** it prints `<alias> has a new secret — every connector token
   issued before now no longer works; this Mac took the new one and
   restarted <name>`. Over ssh the restart is detached, so your session
   survives it.
   **If not:** an error before "new secret" changed nothing; fix it and
   rerun. An error after it (fetching or restarting) means the old token is
   already dead and the owner doesn't have the new one yet: go straight to
   step 2's fix line.

2. **The owner runs on the new token, from a file.**

   ```sh
   tunnels tunnel list        # <name>: loaded, a new PID, TOKEN file
   plutil -extract ProgramArguments json -o - \
     ~/Library/LaunchAgents/com.cloudflare.cloudflared-<name>.plist | grep -c '"--token"'   # 0
   ls -l ~/.config/tunnels/tokens/<tunnel-id>        # -rw-------, written just now
   ```

   and step 4 from Before again: the owner shows `token_file=True
   current=True`.
   **If not:** rotating from another box leaves the owner to its agent;
   wait one pass (`policy.interval`, 120 s) and look for
   `its tunnel was rotated; took the new connector token` in
   `tunnels agent status`. Still nothing: `tunnels tunnel restart <name>` on
   the owner, or **paste tunnel token** on its card in the web UI.

3. **Root daemon only.** tunnels 0.25.x doesn't manage a tunnel running as
   a root LaunchDaemon; newer releases do it through `sudo -n` or print the
   two lines to run. Either way, on the owner, with its password:

   ```sh
   sudo install -o root -g wheel -m 600 ~/.config/tunnels/tokens/<tunnel-id> /etc/cloudflared/<name>.token
   sudo launchctl kickstart -k system/com.cloudflare.cloudflared-<name>
   ```

   Skipping this leaves the daemon on a dead token: it keeps its current
   connections and fails at its next restart or reboot.

4. **Cloudflare sees only the new connector.**

   ```sh
   tunnels cf get '/accounts/{account:<acct>}/cfd_tunnel/{tunnel:<alias>}/connections' \
     | grep -E '"(client_id|opened_at)"'
   ```

   **Check:** every `opened_at` is after T, and the `client_id` differs from
   the one you wrote down. **If an old `client_id` is still there:** some
   cloudflared still holds a connection it opened with the old token. Find
   it with the copy list (Before, step 4) and stop it there; it can't
   reconnect.

5. **Every hostname answers as before** (the index's loop).

6. **Clean up the other copies.** Rerun the copy list. Any box other than
   the owner now shows `current=False`: that copy is dead, which is the
   proof Cloudflare no longer honours what leaked. Remove it on that box
   with `tunnels tunnel forget <its local name>` (local only; it changes
   nothing in Cloudflare). Delete leftover files you found in Before step 4
   once the owner is confirmed running from `~/.config/tunnels/tokens/`.

## What you can and cannot prove

You never handle the old token, so you can't try it. What you can show:
`rotate` returned (Cloudflare replaced the secret), the owner's copy is
`current=True` while every copy made before is `current=False`, and every
live connection opened after T. Report those three, and never paste a token
into the report.

## Stop and ask a person if

- the owner doesn't answer over the tailnet (you'd be rotating blind);
- the owner isn't running the new token after step 2's fix line, and its
  hostnames are down: that's an outage, tell the owning lead now;
- the tunnel is a root daemon and nobody with the owner's password is there:
  rotate anyway only if the leak is worse than the next reboot killing it.

## Undo

None. A rotated secret can't be put back; the fix for a bad rotation is a
correct token on the owner (step 2's fix line).

## History

- 2026-09-29: doug-mini's plist carried its token inline, so it showed in
  `ps`, and a copy sat in an old transcript on the mini. Rotation was
  planned, not yet run. The rotate, token-file and copy checks here were run
  read-only against the live mesh on 2026-09-30.
