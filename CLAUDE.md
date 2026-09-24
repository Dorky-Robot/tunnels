# tunnels

Cloudflare tunnels across a fleet of Macs, driven by a config file and a CLI and kept in line by
an agent on every machine. There is no TUI. The web UI, served by every agent on the tailnet
(and optionally on a public hostname behind pocket-id sign-in), is the visual interface.

## Architecture

```
  fleet.toml ──┐                        ┌── cf.rs (Cloudflare API)
               ├─ plan.rs ── apply.rs ──┤
  observe.rs ──┘                        └── launchd.rs (this Mac)
```

- **fleet.rs**: the fleet file (`~/.config/tunnels/fleet.toml`, overridden by `TUNNELS_FLEET`).
  It holds machines, accounts, tunnels (an alias pointing at a Cloudflare id), routes, and policy.
  No secrets.
  - `validate()` catches the mistakes that have hurt before, such as a hostname aimed at a tunnel
    in a different account from its zone.
  - `Fleet::edit` re-reads the file, applies the change, bumps `serial`, validates and saves.
- **config.rs**: this Mac's secrets (`config.json`): connector tokens and API tokens. **Every
  mutation goes through `Config::edit`, which re-reads the file first**, and `save` is private.
  A long-running process that blindly wrote its stale copy once lost a token.
- **cf.rs**: a typed Cloudflare client (ureq). Never shell out to curl, which puts the token in
  argv. Ingress rules keep their raw JSON so `originRequest` and similar fields survive a rewrite.
  `TUNNELS_CF_API` overrides the base URL; the tests point it at a fake Cloudflare.
- **observe.rs**: a snapshot of every account, tunnel, connector, ingress rule and tunnel CNAME,
  plus this Mac's launchd state.
  - A `Want` narrows what gets fetched; the agent asks only for what it owns.
  - `ingress: None` means *unknown*, not *empty*. The plan must never remove what it has not seen.
- **plan.rs**: pure; takes a fleet and a snapshot and returns actions and findings. Each action
  carries its `scope`, its `owner` machine, and flags:
  - `prune`: removes something the file doesn't mention; needs `--prune`.
  - `guarded`: a live takeover; needs `--yes`.
  - `destroy`: deletes a tunnel; needs `--allow-destroy`.

  Agents run only actions with none of these flags, and only the ones they own.
- **apply.rs**: runs actions in a fixed order (local, ingress, DNS, removals, destroys). When a
  DNS step fails, it rolls back that route's ingress change.
- **agent.rs**: the per-machine loop.
  - It pulls the newest fleet from peers, reloads booted-out jobs, and restarts dead connectors,
    but only after two bad passes and at most once every 5 minutes.
  - It refetches a rotated connector token, repairs drift it owns, and performs automatic
    failover.
  - When the binary changes (`brew upgrade`) it execs the new one in place; its plist is
    `ProcessType Interactive` because launchd deferred its respawn on doug-mini.
- **web.rs** + **web/index.html**: the agent's HTTP API and the UI.
  - Binds to the tailnet IP and loopback only, and rejects non-tailnet addresses.
  - POSTs need an `X-Tunnels: 1` header.
  - `door()` decides how a request arrived. Tailnet or loopback: full rights. Proxied
    (Cloudflare headers): only from loopback, only for `[policy.web] public_host`, and then
    `/auth/*` or a live session. POSTs need an admin. Peer endpoints (`/api/cf-forward`,
    `/api/relay-exec`, `/api/notify`) refuse anything proxied.
  - `/api/relay` runs a machine action here or relays it to the target's `/api/relay-exec`,
    which checks `may_forward` (allowlist plus the caller's tailnet address).
- **access.rs**: sign-in for the public UI. The agent is itself an OIDC client of pocket-id:
  authorization code + PKCE with a public client (no secret), the ID token verified against the
  provider's JWKS (RS256, audience = client_id, issuer, expiry), and in-memory sessions behind
  an HttpOnly, Secure, SameSite=Lax cookie. Admin = the email is in `[policy.web] admins`.
  Never Cloudflare Access: Felix's rule is that apps sign people in themselves.
- **tokens.rs**: a machine's tokens for the UI, with no secret in any view. API tokens are
  identified by a sha256 fingerprint; connector tokens carry whether they still match
  Cloudflare. It also holds add/remove/refresh and `refetch_connector`. The UI reaches them
  through `/api/mesh-tokens` and the relay actions `token-add|token-rm|token-refresh|connector-refetch|tunnel-rotate`
  (rotate runs the CLI's `tunnel rotate` on the target machine).
- **sync.rs**: fleet replication. The file is served at `/api/fleet` and the newest `serial`
  wins. `notify` wakes the peers.
- **status.rs**: the shared view model used by `tunnels status` and the web UI.
- **scope.rs**: `Scope` for every command. `cf::Client::new` panics if the current command
  declared `Local`.
- **api.rs**: `tunnels cf`, a pass-through to the whole Cloudflare API. It resolves
  `{account|zone|tunnel|record:name}` placeholders, picks the token, hides secrets in
  responses, and refuses writes to what `tunnels` owns (cfd_tunnel, tunnel CNAMEs).
  Writes are previews until `--yes`; each is logged to `cf-log/` with before, after and an
  undo request. When no token here reaches the account (the `NotHere` error, and only that),
  the call is forwarded to a peer agent's `/api/cf-forward`. The peer checks the caller is on
  `policy.remote_from` and that the request comes from its tailnet address, then makes the call
  and logs it with `requested_by`. Plan: `docs/agent-operations.md`.
- **main.rs**: the clap CLI. The `SCOPES` table is the single source of truth for scopes, and
  tests hold every command's help text to it.

## Rules

- **Say where a command acts.** New commands get an entry in `SCOPES`, and their doc comment
  starts with that scope's tag. `tunnels rm` once said "Delete a tunnel" but only forgot it
  locally, so its token kept working in Cloudflare. It is gone; `tunnel forget` (local) and
  `tunnel destroy` (Cloudflare) replace it.
- **Unknown is not missing.** Never plan a removal from data that wasn't fetched.
- **One owner per thing.** Ingress belongs to the machine running the tunnel; DNS belongs to the
  machine running the route's *active* tunnel.
- **No silent takeovers.** A hostname served by a live tunnel moves only with `--yes`.

## Don't strand the machine you're on

This tool restarts the tunnels that carry our ssh. On 2026-09-21, a bootout-then-bootstrap over a
tunneled ssh session took Doug's mini offline: the bootout killed the session before the
bootstrap could run.

- Restart loaded jobs with `launchctl kickstart -k` (`launchd::kickstart`).
- When a plist has to change, `launchd::restart` detaches both halves whenever
  `SSH_CONNECTION`/`SSH_TTY` is set.
- The agent reloads booted-out jobs; this replaces `scripts/tunnel-watchdog.sh`.
- Connector tokens live in `~/.config/tunnels/tokens/<id>` (0600) and plists use `--token-file`,
  so a token change needs only a kickstart.

## Build, test, release

```
cargo test            # unit tests + tests/cli.rs (the real binary against a fake Cloudflare)
cargo build --release
```

Release: bump `Cargo.toml`, tag `vX.Y.Z`, and push the tag. `.github/workflows/release.yml`
builds both architectures and updates `dorky-robot/homebrew-tap`.

`mesh/` is the one source for the mesh ssh config (three paths per machine: tailnet, `-lan`,
`cloudflare-`). Push it to every machine with `sh mesh/install.sh`, which also runs
`tunnels agent install`. `docs/remote-access.md` explains why it is shaped this way and how to add
a machine.
