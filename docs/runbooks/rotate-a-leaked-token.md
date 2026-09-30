# Rotate a tunnel token that leaked

**Use when:** a tunnel's connector token (the long `eyJ…` string cloudflared
runs with) was seen somewhere it shouldn't be: a transcript, a paste, `ps`
output, a plist in a backup.
**Don't use when:** it was a Cloudflare **API** token (`cfut_…`). Roll that
in the Cloudflare dashboard, then give each box the new one from the web
UI (the account card's **rotate**), or `tunnels token add <new>` and
`tunnels token rm <n>` on each box that had it.
**Scope:** [this Mac + cloudflare]. **No preview, no undo:** from the moment
`rotate` returns, every token issued before it is dead, everywhere.
**Needs:** the tunnel's owner reachable over the tailnet, holding an API
token for the tunnel's account (`tunnels token list` there).

## Before you start

1. **Name it once.** On any box, with the fleet alias of the tunnel
   (`tunnels route list` shows which tunnel carries a hostname):

   ```sh
   A=<alias>
   read OWNER ACCT ID <<< "$(tunnels fleet show --json | python3 -c 'import json,sys; t=json.load(sys.stdin)["tunnels"][sys.argv[1]]; print(t["machine"], t["account"], t["id"])' $A)"
   echo "owner=$OWNER account=$ACCT"
   ```

2. **Get onto the owner by a path the rotation can't cut.** Rotating
   restarts the owner's cloudflared; if your ssh runs through it, you lose
   the box.

   ```sh
   ssh -G $OWNER | grep -i '^proxycommand'    # must print nothing
   ssh $OWNER                                 # the tailnet (or $OWNER-lan), never cloudflare-$OWNER
   export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH
   ```

   Set `A`, `OWNER`, `ACCT` and `ID` again there (step 1), then its local
   name:

   ```sh
   N=$(tunnels tunnel list --json | python3 -c 'import json,sys; print(next(t["name"] for t in json.load(sys.stdin)["tunnels"] if t.get("fleet_alias")==sys.argv[1]))' $A)
   ```

3. **Write down the before state.** Nothing here prints a token.

   ```sh
   date -u +%FT%TZ                       # T: everything after the rotation is newer than this
   tunnels tunnel list                   # $N: its PID, and TOKEN "file" or "in plist"
   tunnels cf get "/accounts/{account:$ACCT}/cfd_tunnel/{tunnel:$A}/connections" | grep -E '"(client_id|opened_at)"'
   ```

   plus the index's host loop for this box (`| grep "^$OWNER "`).

4. **Every copy of this token in the mesh**, as the agents see them:

   ```sh
   curl -s http://127.0.0.1:7630/api/accounts | python3 -c 'import json,sys
   for acct in json.load(sys.stdin):
       for m,c in acct["tunnels"]:
           if c.get("alias")==sys.argv[1]: print(m, c["name"], c["state"], "token_file=%s" % c["token_file"], "current=%s" % c["current"])
       if acct.get("unknown"): print("unknown, agent not answering:", acct["unknown"])' $A
   ```

   Expect the owner, plus any box keeping a spare copy. Copies `tunnels`
   didn't write it can't see; on the owner, list file **names** only:
   `ls ~/.cloudflared/` and
   `grep -l -- '--token' ~/Library/LaunchAgents/com.cloudflare.cloudflared-*.plist`.

5. **A root LaunchDaemon?** `ls /Library/LaunchDaemons | grep cloudflared`.
   If it lists `com.cloudflare.cloudflared-$N` (doug-mini, after its visit),
   step 3 below needs the owner's password.

## Steps

1. **Rotate**, on the owner:

   ```sh
   tunnels tunnel rotate $A
   ```

   **Check:** `✓ <alias> has a new secret — every connector token issued
   before now no longer works; this Mac took the new one and restarted
   <name>`. Over ssh the restart is detached, so your session survives it.
   **If not:** an error before "new secret" changed nothing; fix it and
   rerun. An error after it means the old token is already dead and the
   owner doesn't have the new one yet: go to step 2's fix line now.

2. **The owner runs the new token, from a file.**

   ```sh
   tunnels tunnel list                  # $N: loaded, a new PID, TOKEN file
   plutil -extract ProgramArguments json -o - ~/Library/LaunchAgents/com.cloudflare.cloudflared-$N.plist | grep -c '"--token"'   # 0
   ls -l ~/.config/tunnels/tokens/$ID   # -rw-------, written just now
   ```

   and the copy list (Before, 4) shows the owner `token_file=True current=True`.
   **Fix line:** `tunnels tunnel restart $N` on the owner; or **paste tunnel
   token** on its card in the web UI. (Rotated from another box, the owner's
   agent takes the new token within one pass, 120 s:
   `tunnels agent status` says `its tunnel was rotated; took the new
   connector token`.)

3. **Root daemon only.** tunnels 0.25.x doesn't manage a tunnel run as a
   root LaunchDaemon; later releases do it through `sudo -n` or print these
   two lines. On the owner, with its password:

   ```sh
   sudo install -o root -g wheel -m 600 ~/.config/tunnels/tokens/$ID /etc/cloudflared/$N.token
   sudo launchctl kickstart -k system/com.cloudflare.cloudflared-$N
   ```

   Skip it and the daemon keeps its open connections on a dead token and
   fails at its next restart or reboot.

4. **Cloudflare sees only the new connector.** The `connections` read from
   Before, 3 again: every `opened_at` is after T and the `client_id` is new.
   An old `client_id` still there is some cloudflared holding a connection
   it opened with the old token: find it in the copy list and stop it
   there. It can't reconnect.

5. **Every hostname answers as before** (the host loop again).

6. **Clean up the other copies.** Rerun the copy list: every box other than
   the owner now shows `current=False`. That is the proof Cloudflare no
   longer honours what leaked. On each, `tunnels tunnel forget <its name>`
   (local only). Delete the leftover files from Before, 4 once the owner
   runs from `~/.config/tunnels/tokens/`.

## What to report

You never hold the old token, so you can't try it. You can show three
things: `rotate` returned, every pre-rotation copy reads `current=False`
while the owner's reads `current=True`, and every live connection opened
after T. Never paste a token into a report.

## Stop and ask a person if

- the owner doesn't answer over the tailnet (you'd be rotating blind);
- the owner's hostnames are down after step 2's fix line: that is an
  outage, so tell the owning lead now;
- it is a root daemon and nobody with the owner's password is around:
  rotate anyway only if the leak is worse than the next reboot killing the
  tunnel.

## Undo

None. A rotated secret can't be put back. The fix for a bad rotation is the
right token on the owner (step 2's fix line).

## History

- 2026-09-29: doug-mini's plist carried its token inline (visible in `ps`),
  and a copy sat in an old transcript on the mini. Rotation planned, not yet
  run.
- 2026-09-30: written. Every read here run against the live mesh
  (doug-mini-dorkyrobot: fleet lookup, tunnel list, connections, copy list,
  stray files); `rotate` itself not run.
