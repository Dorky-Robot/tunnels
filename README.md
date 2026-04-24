# tunnels

[![Discord](https://img.shields.io/discord/1483879594619568291?color=5865F2&label=Discord&logo=discord&logoColor=white)](https://dorkyrobot.com/discord)

<div align="center">

<img src="tunnels.jpg" alt="tunnels" width="300">

*A k9s-style TUI for managing cloudflared tunnels and local services.*

</div>

A [k9s](https://k9scli.io/)-style TUI for managing [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/) tunnels and local services on macOS.

Two tabs: **Services** tracks what's running on your machine with tunnel status and public URLs. **Tunnels** manages your cloudflared instances via LaunchAgents.

```
 tunnels   1 Services   2 Tunnels
╶──────────────────────────────────────────────────────────────────────────────────╴
 PROJECT         PORT   STATUS       URL                             MEMO
 web-app         3000   connected    https://app.example.com         production
 api-server      8080   connected    https://api.example.com
 postgres        5432   —            —                               local db
╶──────────────────────────────────────────────────────────────────────────────────╴
 j/k nav  a add  e edit  d del  . more  q quit
```

```
 tunnels   1 Services   2 Tunnels
╶──────────────────────────────────────────────────────────────────────────────────╴
 NAME               STATUS     PID        CF NAME            EDGE
 prod-tunnel        running    12345      my-tunnel          iad,dfw,ord,lax
 dev-tunnel         stopped    -          dev                —
╶──────────────────────────────────────────────────────────────────────────────────╴
 j/k nav  s/x/r start/stop/restart  m routes  a add  d del  . more  q quit
```

Press `.` to reveal secondary actions (sync CF, tokens, scan, import, etc). Press `?` for full help.

## CLI

```bash
tunnels                    # Launch TUI
tunnels list [--json]      # List tunnels
tunnels routes [TUNNEL]    # List ingress routes
tunnels route add <hostname> <port> --tunnel <name>
                           # Add subdomain mapping (idempotent)
tunnels route rm <hostname> --tunnel <name>
                           # Remove subdomain mapping
tunnels import             # Import existing plists
```

Route commands are idempotent — safe to re-run to fix DNS if it failed the first time.

## Prerequisites

- **macOS** (uses LaunchAgents and `lsof` for service discovery)
- **cloudflared** — `brew install cloudflared`
- **Cloudflare API tokens** (optional, for route management and ingress resolution):

  Press `T` in the TUI to add tokens — paste any token and it auto-matches to the right CF account. Supports multiple accounts and per-tunnel tokens.

  Create tokens at [dash.cloudflare.com/profile/api-tokens](https://dash.cloudflare.com/profile/api-tokens) with:
  - **Account > Cloudflare Tunnel > Read** (required)
  - **Zone > DNS > Edit** (for route management)

## Install

```bash
brew tap dorky-robot/tap
brew install dorky-robot/tap/tunnels
```

Or build from source:

```bash
git clone https://github.com/Dorky-Robot/tunnels.git
cd tunnels
cargo build --release
cp target/release/tunnels ~/.local/bin/
```

## Tabs

### 1 — Services

Tracks what's running on your machine and links it to Cloudflare tunnels. Each service has an optional memo field for notes.

| Key | Action |
|-----|--------|
| `a` | Add service |
| `e` | Edit service |
| `d` | Untrack service |

Secondary (`.`):

| Key | Action |
|-----|--------|
| `S` | Scan listening ports |
| `R` | Sync from Cloudflare API |
| `T` | Add CF API token |

### 2 — Tunnels

Manages cloudflared tunnel instances as macOS LaunchAgents. Each tunnel auto-starts at login.

| Key | Action |
|-----|--------|
| `s` | Start tunnel |
| `x` | Stop tunnel |
| `r` | Restart tunnel |
| `m` | Manage routes (subdomains) |
| `a` | Add new tunnel |
| `d` | Delete tunnel |

Secondary (`.`):

| Key | Action |
|-----|--------|
| `e` | Edit token |
| `n` | Rename tunnel |
| `l` | View logs |
| `R` | Sync from Cloudflare API |
| `T` | Add CF API token |
| `I` | Import existing plists |

### Route Management

Press `m` on a tunnel to manage its subdomain routes. Routes are Cloudflare ingress rules + DNS records. Adding a route creates both the ingress rule and the CNAME record. The operation is idempotent — re-running fixes anything that's broken (e.g. missing DNS).

```
 Routes: prod-tunnel
╶────────────────────────────────────────────────────╴
 HOSTNAME                    SERVICE                DNS
 app.example.com             http://localhost:3000   ✓
 api.example.com             http://localhost:8080   ✓
 (catch-all)                 http_status:404         ✓
╶────────────────────────────────────────────────────╴
 j/k nav  a add route  d delete route  Esc back
```

## How it works

- **Config** stored at `~/.config/tunnels/config.json`
- **Plists** generated in `~/Library/LaunchAgents/`
- **Logs** written to `~/Library/Logs/tunnels/`
- **Cloudflare API** tokens stored in config (supports multiple CF accounts + per-tunnel tokens)
- Tunnels **auto-start at login** via `RunAtLoad`

### Adding a tunnel

1. Go to [Cloudflare Zero Trust](https://one.dash.cloudflare.com/) → Networks → Tunnels
2. Create a tunnel and copy the token
3. In the TUI, press `a`, enter a name and paste the token
4. Press `s` to start

### Adding a route (CLI)

```bash
# Map a subdomain to a local port (creates ingress rule + DNS CNAME)
tunnels route add app.example.com 3000 --tunnel prod-tunnel

# Safe to re-run — fixes DNS if it failed
tunnels route add app.example.com 3000 --tunnel prod-tunnel
```

### Migrating from system-level LaunchDaemons

If cloudflared was installed via `cloudflared service install`, it runs as a root-owned LaunchDaemon. Press `I` to import — if daemon plists are found, the TUI will offer to migrate them to user-level LaunchAgents (one-time sudo, then never again).

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
