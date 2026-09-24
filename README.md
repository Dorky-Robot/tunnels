# tunnels

[![Discord](https://img.shields.io/discord/1483879594619568291?color=5865F2&label=Discord&logo=discord&logoColor=white)](https://dorkyrobot.com/discord)

<div align="center">

<img src="tunnels.jpg" alt="tunnels" width="300">

*Cloudflare tunnels across a fleet of Macs: one config file, one CLI, an agent on every machine.*

</div>

`tunnels` manages [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/)
tunnels on macOS, on one Mac or on several Macs sharing one or more Cloudflare accounts.

- **A fleet file** (`~/.config/tunnels/fleet.toml`) lists every tunnel, the machine that runs it,
  and where every hostname goes. It contains no secrets.
- **`tunnels plan`** compares the fleet file with Cloudflare and with this Mac. **`tunnels apply`**
  makes them match.
- **An agent** on each Mac runs that comparison continuously:
  - it restarts tunnels that died, or that launchd has forgotten about (booted out);
  - it repairs ingress and DNS that drifted from the fleet file;
  - it picks up rotated connector tokens by itself;
  - it can fail a hostname over to a standby tunnel.
- **Agents share the fleet file with each other** over your tailnet. Edit it on any machine and
  the others pick up the change within a minute. No central server is involved.
- **A web UI** served by every agent shows the whole fleet. It is reachable **only over the
  tailnet**.
- **Every command says where it acts**, both in `--help` and before it runs: `[this Mac only]`,
  `[read-only]`, `[fleet file + cloudflare]`, and so on. `--json` output includes the same
  information, so scripts and agents can check it too.

The CLI is meant to be driven by people and by coding agents alike. There is no TUI any more;
see [Upgrading from 0.15](#upgrading-from-015).

## Install

```bash
brew install dorky-robot/tap/tunnels     # also: brew install cloudflared
```

Or from source: `cargo build --release && cp target/release/tunnels ~/.local/bin/`.

You need a Cloudflare API token for each account, with these permissions:

- **Account › Cloudflare Tunnel › Edit**
- **Zone › DNS › Edit**
- **Zone › Zone › Read**

Create tokens at [dash.cloudflare.com/profile/api-tokens](https://dash.cloudflare.com/profile/api-tokens).

```bash
tunnels token add <api-token>     # once per Cloudflare account; shows what the token can reach
```

## Getting started

### On the first Mac

```bash
tunnels status              # every tunnel, connector, route and DNS record in every account
tunnels import              # write this Mac and its tunnels into the fleet file
tunnels plan                # what differs from the fleet file (exit code 2 if anything does)
tunnels agent install       # keep this Mac in line; serve the web UI on the tailnet
tunnels web --open
```

`import` writes down only what already works. A hostname is imported when its ingress and its
DNS agree on the same tunnel. Anything else is listed with a note rather than guessed at.

### On every other Mac

```bash
tunnels fleet join <first-mac>   # take the fleet file from that Mac's agent, over the tailnet
tunnels import                   # add this Mac and what it runs
tunnels agent install
```

### Creating tunnels and routes

```bash
tunnels tunnel create web --account myaccount --machine studio     # the agent on studio starts it
tunnels route add app.example.com 3000 --tunnel web                # ingress + DNS; the fleet file records it
tunnels route add app.example.com 3000 --tunnel web --standby web2 # a second tunnel for failover
tunnels route list                                                 # every hostname and where its DNS really goes
tunnels route rm app.example.com
```

Connector tokens never have to be copied between machines. The agent on the machine a tunnel is
assigned to fetches its token through that machine's API token.

## The fleet file

```toml
serial = 12                       # bumped on every change; the highest serial wins
[policy]
interval = 120                    # seconds between agent passes
failover_after = 300              # how long a primary must be down before automatic failover
web_port = 7630
prune = false                     # let agents remove what the file does not mention

[machines.studio]
host = "studio"                   # its tailnet name

[accounts.myaccount]
id = "0123abcd…"
zones = ["example.com"]

[tunnels.web]
id = "6da56f03-…"                 # the Cloudflare tunnel id, which is its identity
account = "myaccount"
machine = "studio"

[[routes]]
host = "app.example.com"
tunnel = "web"
service = "http://localhost:3000"
standby = "web2"                  # optional
failover = "manual"               # or "auto"
```

- Tunnels are identified by their Cloudflare **id** and named by an **alias** you choose. Two
  machines can no longer have two different tunnels that share a name.
- To edit the file safely, use `tunnels fleet edit`. It validates your change, bumps the serial
  and shares the result with the other machines. `tunnels fleet validate` checks a hand edit.
- A route whose zone is in one account but whose tunnel is in another is rejected before anything
  is changed. A tunnel CNAME only works within a single account.

## What happens automatically, and what waits for you

| Kind of change | Who can do it |
|---|---|
| Start a tunnel assigned here, reload one launchd forgot, restart one Cloudflare sees no connectors from | the agent |
| Add or fix ingress and DNS for a route the file declares | the agent that owns the route (the machine running its tunnel) |
| Take a hostname away from a tunnel that is **serving it right now** | you: `apply --yes` |
| Remove ingress or DNS that the file doesn't mention | you: `apply --prune`, or `policy.prune = true` |
| Delete a tunnel in Cloudflare | you: `tunnel destroy`, or `destroy = true` plus `apply --allow-destroy` |

Each route has exactly one owner, so two agents never undo each other's work. When a DNS step
fails, the ingress change for that route is rolled back, so no half-finished route is left
behind.

## Forget, destroy, rotate

These three are easy to confuse, so every one of them states its scope:

| Command | This Mac | Cloudflare |
|---|---|---|
| `tunnels tunnel forget <t>` | stops the tunnel; removes its LaunchAgent and token here | **nothing.** The tunnel still exists and **its tokens still work**, and the command says so |
| `tunnels tunnel destroy <t>` | forgets it here as well | deletes its DNS records, its connections and the tunnel itself. **Every connector token for it stops working** |
| `tunnels tunnel rotate <t>` | takes the new token and restarts | issues a new tunnel secret. **Old tokens stop working**; agents on other machines fetch the new one |

`status`, `doctor` and the web UI list **orphans**: tunnels that exist in your accounts but not
in the fleet file, whose tokens therefore still work. Each orphan comes with both ways out:
`tunnel adopt` to bring it into the fleet, or `tunnel destroy` to delete it.

## Failover

A route with a `standby` is carried by both tunnels. DNS decides which tunnel actually receives
the traffic.

```bash
tunnels promote app.example.com     # send traffic to the standby now
tunnels failback app.example.com    # send it back to the primary
```

With `failover = "auto"`, the agent on the standby's machine promotes the route itself once the
primary has had no connectors for `policy.failover_after` seconds. It records the switch in the
fleet file (`active = "standby"`), so a primary that comes back does not pull the traffic back on
its own; failing back is always your decision. Use `manual` (the default) for anything whose
standby does not share the primary's data.

## The web UI

`tunnels web` prints the address. The UI shows:

- every account, tunnel, connector and route, with where each hostname's DNS actually points;
- orphans, and tunnels that are down;
- the pending plan, with buttons for applying safe changes, pruning, or taking over hostnames;
- promote and failback buttons;
- the agent's recent actions and the tunnel logs.

The UI binds only to the Mac's tailnet address and to localhost. It rejects any request from
elsewhere, including anything that came through a Cloudflare tunnel, and the fleet file refuses
a route that points at the UI's port.

## The rest of Cloudflare: `tunnels cf`

For anything `tunnels` doesn't model (Access, zone settings, WAF), `tunnels cf` passes calls
through to the whole Cloudflare API, with guardrails:

```bash
tunnels cf get   '/zones/{zone:example.com}/settings/ssl'
tunnels cf get   '/accounts/{account:myaccount}/access/apps'
tunnels cf patch '/zones/{zone:example.com}/settings/ssl' --data '{"value":"strict"}'         # preview
tunnels cf patch '/zones/{zone:example.com}/settings/ssl' --data '{"value":"strict"}' --yes   # send
tunnels cf log                   # changes made from this Mac, with before → after
tunnels cf undo <id> --yes       # put it back
```

- **Names instead of ids.** Write `{account:<alias>}`, `{zone:<name>}`, `{tunnel:<alias>}` or
  `{record:<hostname>}`, and the right id is filled in. An ambiguous name is an error that lists
  the candidates.
- **No secrets in view.** The token for the account is picked for you and never printed.
  Connector tokens, client secrets and API token values in responses are hidden.
- **Reads are free; writes are previews until `--yes`.** Every write is read back after it is sent
  and logged on this Mac, together with the request that undoes it. A write with no way to undo
  it needs `--not-undoable`; token changes need `--i-mean-tokens`.
- **What `tunnels` owns is refused.** Tunnel ingress, tunnel tokens and tunnel CNAMEs go through
  `tunnels route` and `tunnels tunnel`, so the fleet file stays the truth.
- **The web UI shows every machine's `cf` log** in one timeline.
- **A 403 says which permission is missing.** The Tunnel and DNS permissions `tunnels` needs don't
  cover zone settings or Access.

## Scripts and agents

- Every command accepts `--json`, and its output includes `"scope"`.
- `tunnels plan` exits with 0 when nothing differs and 2 when something does.
- Writes go through the fleet file, so the change history is kept in
  `~/.config/tunnels/fleet.history/` and in `tunnels fleet history`.
- Useful commands: `tunnels status --json`, `tunnels route list --json`,
  `tunnels doctor --json`, `tunnels agent status`.

## Files

| Path | What it holds |
|---|---|
| `~/.config/tunnels/fleet.toml` | the fleet file (no secrets; shared with the other machines) |
| `~/.config/tunnels/config.json` | this Mac's API tokens and connector tokens (0600, never shared) |
| `~/.config/tunnels/tokens/<tunnel-id>` | connector tokens, 0600; cloudflared reads them with `--token-file` |
| `~/Library/LaunchAgents/com.cloudflare.cloudflared-<name>.plist` | one per tunnel |
| `~/Library/LaunchAgents/com.dorkyrobot.tunnels-agent.plist` | the agent |
| `~/Library/Logs/tunnels/` | tunnel logs and `agent.log` |

## Upgrading from 0.15

- **The TUI is gone.** Use `tunnels status` in a terminal and `tunnels web` for a browser view.
- **`tunnels rm` is gone.** It only ever forgot a tunnel locally. Use `tunnel forget` or
  `tunnel destroy`, depending on which one you mean.
- **`tunnels import` now writes the fleet file.** Taking in existing cloudflared plists is
  `tunnels tunnel import-plists`.
- `list`, `routes`, `start`, `stop`, `restart`, `logs`, `add`, `sync` and `heal` still work as
  aliases.
- Service tracking (`tunnels service …`) is gone. A route's `note` takes its place.
- Your existing config keeps working. Plists that still carry an inline token move to token files
  the next time each tunnel is restarted.
- `tunnels agent install` removes the old `tunnel-watchdog` LaunchAgent, because the agent does
  its job now.

## Getting in when the tunnel is the thing that broke

`ssh` to these machines can run through the very tunnels this tool manages, so restarting a
tunnel the wrong way can lock you out of the machine you are restarting. `tunnels` never
boots out a job it isn't about to load again, and it detaches restarts made over ssh.
`docs/remote-access.md` describes the incident behind these rules and the three independent ways
into each machine.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
