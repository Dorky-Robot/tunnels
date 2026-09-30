# Runbooks

Short, runnable steps for the mesh jobs that come up at the worst times. Each
one says when to use it, what to check before you start, one step at a time
with the check that proves it worked, and when to stop and get a person. The
plan behind them is [`../agent-operations.md`](../agent-operations.md),
"Layer 3".

| Runbook | Use it when |
|---|---|
| [after-a-power-cut.md](after-a-power-cut.md) | a box (or the house) lost power or rebooted, and you need it back and proven |
| [rotate-a-leaked-token.md](rotate-a-leaked-token.md) | a tunnel's connector token was seen somewhere it shouldn't be |
| [add-a-machine.md](add-a-machine.md) | a new Mac joins the mesh |
| [retire-a-hostname.md](retire-a-hostname.md) | a public hostname should stop existing |
| [revive-a-claude-session.md](revive-a-claude-session.md) | a Claude lead or worker is gone, or stuck on "connecting…" |

## Three rules that apply to every one of them

1. **Never cut the path you're on.** Before anything that restarts
   cloudflared, Tailscale, sshd or networking, know how you are connected:

   ```sh
   ssh -G <box> | grep -i '^proxycommand'   # prints nothing: tailnet, safe
                                            # prints cloudflared: you ride the tunnel
   ```

   Go in by `ssh <box>` (tailnet) or `ssh <box>-lan`, never by
   `ssh cloudflare-<box>`, when the tunnel is what you are touching. Restart a
   loaded job with `launchctl kickstart -k`, never `bootout` then `bootstrap`.
   The story is in [`../remote-access.md`](../remote-access.md).
2. **Over ssh, PATH has no Homebrew.** Start remote commands with
   `export PATH=/opt/homebrew/bin:/usr/local/bin:$PATH`. "command not found"
   over ssh is PATH, not a missing install.
3. **Unknown is not missing.** A check that could not run (box unreachable,
   token can't read the account) is "unknown". Don't remove, restart or
   rotate anything on the strength of it.

## Bridges

A bridge is a service outside the mesh the mesh depends on. These runbooks
cross three; when a step fails, ask first whether the bridge is down:

| bridge | the runbooks use it for | if it is down |
|---|---|---|
| **Cloudflare** | public hostnames, tunnels, DNS, `tunnels cf`, the `cloudflare-<box>` ssh path | sites are down everywhere at once; ssh by the tailnet or LAN still works; don't rotate or retire anything |
| **Tailscale** | `ssh <box>`, the agents' fleet sync, Taildrop | use `<box>-lan` or `cloudflare-<box>`; a box that drops off alone is more likely its key or its daemon |
| **GitHub** | this repo, `mesh/rollout`'s key step, releases of `tunnels` | nothing running stops; landing and rollout wait |

The alert bridges (ntfy.sh, and the planned Healthchecks.io) are in
[`../mesh-watch.md`](../mesh-watch.md#bridges).

## Two checks the runbooks reuse

**What the agent on a box thinks of its tunnel tokens**, with no secret shown
(loopback has full rights on the agent):

```sh
curl -s http://127.0.0.1:7630/api/tokens | python3 -c 'import json,sys
for c in json.load(sys.stdin)["connectors"]:
    print(c["name"], c.get("alias"), c["state"], "token_file=%s" % c["token_file"], "current=%s" % c["current"])'
```

`current=True` means the token on this box is the one Cloudflare issues now;
`False` means it is dead (rotated away); `None` means unknown (no API token
here reaches that account).

**Does every public hostname answer**, from any box, grouped by the box that
serves it (add `| grep '^<box> '` for one box):

```sh
tunnels route list --json | python3 -c 'import json,sys
for r in json.load(sys.stdin):
    if r["active"]: print(r["machine"], r["host"])' | sort -u | while read m h; do
  printf '%-12s %-45s %s\n' "$m" "$h" "$(curl -s -o /dev/null -m 10 -w '%{http_code}' "https://$h/")"
done
```

200 or a 30x redirect (to sign-in) is up; `ssh-*` hosts answer 200 too. 502
or 530 is a tunnel or its app down; 000 is no answer at all. `scripts/mesh-watch.py` does
this properly (expected text, retries); the loop is the 3am version.

## When a runbook is wrong

Fix it in the same sitting: change the step, and add a line to its
**History** saying what happened and what changed. That is how these get
better.
