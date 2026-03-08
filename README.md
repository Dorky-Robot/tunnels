# tunnels

A [k9s](https://k9scli.io/)-style TUI for managing [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/) tunnels and local services on macOS.

Two tabs: **Tunnels** manages your cloudflared instances via LaunchAgents. **Services** auto-discovers what's running on your machine, matches it to Cloudflare ingress routes, and shows you the public URL.

```
 tunnels   1 Tunnels   2 Services
╶──────────────────────────────────────────────────────────────────────────────╴
 PROJECT         PORT   TUNNEL             STATUS       URL
 katulong        3001   mac-2024           connected    https://katulong-prime.felixflor.es
 abot            7070   —                  —            —
 levee           3333   —                  —            —
╶──────────────────────────────────────────────────────────────────────────────╴
 1/2 tabs  j/k navigate  S scan  a add  e edit  d untrack  ? more  q quit
```

## Prerequisites

- **macOS** (uses LaunchAgents and `lsof` for service discovery)
- **cloudflared** — `brew install cloudflared`
- **Cloudflare login** (optional, for ingress route resolution):
  ```bash
  cloudflared tunnel login
  ```
  This creates `~/.cloudflared/cert.pem` which tunnels reads (read-only) to fetch your tunnel configurations from the Cloudflare API. Without it, the Tunnels tab still works — you just won't see CF names, edge status, or auto-resolved URLs in the Services tab.

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

## Usage

```bash
tunnels          # Launch TUI
tunnels list     # List tunnels (non-interactive)
tunnels import   # Import existing cloudflared plists
```

## Tabs

### 1 — Tunnels

Manages cloudflared tunnel instances as macOS LaunchAgents. Each tunnel auto-starts at login.

| Key | Action |
|-----|--------|
| `s` | Start tunnel |
| `x` | Stop tunnel |
| `r` | Restart tunnel |
| `a` | Add new tunnel |
| `e` | Edit token |
| `n` | Rename tunnel |
| `d` | Delete tunnel |
| `l` | View logs |
| `R` | Sync from Cloudflare API |
| `I` | Import existing plists |

### 2 — Services

Tracks what's running on your machine and links it to Cloudflare tunnels.

| Key | Action |
|-----|--------|
| `S` | Scan listening ports |
| `a` | Add service manually |
| `e` | Edit service |
| `d` | Untrack service |

Press `S` to scan — it uses `lsof` to find all listening TCP ports, resolves the project name from the process's working directory, and cross-references Cloudflare ingress rules to auto-fill the tunnel name, status, and public URL.

## How it works

- **Config** stored at `~/.config/tunnels/config.json`
- **Plists** generated in `~/Library/LaunchAgents/`
- **Logs** written to `~/Library/Logs/tunnels/`
- **Cloudflare API** credentials read from `~/.cloudflared/cert.pem` (created by `cloudflared tunnel login`)
- Tunnels **auto-start at login** via `RunAtLoad`

### Adding a tunnel

1. Go to [Cloudflare Zero Trust](https://one.dash.cloudflare.com/) → Networks → Tunnels
2. Create a tunnel and copy the token
3. In the TUI, press `a`, enter a name and paste the token
4. Press `s` to start

### Migrating from system-level LaunchDaemons

If cloudflared was installed via `cloudflared service install`, it runs as a root-owned LaunchDaemon. Press `I` to import — if daemon plists are found, the TUI will offer to migrate them to user-level LaunchAgents (one-time sudo, then never again).

## License

MIT
