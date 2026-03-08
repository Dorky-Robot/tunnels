# tunnels

A [k9s](https://k9scli.io/)-style TUI for managing multiple [cloudflared](https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/) tunnel instances on macOS.

```
 tunnels — cloudflared tunnel manager
╶──────────────────────────────────────────────────────────────────────╴
 NAME             STATUS     PID      TUNNEL ID      TOKEN
 ──────────────── ────────── ──────── ────────────── ──────────────────
 production       running    41023    a3f8c92d...    eyJhIjoiM2Y4...
 staging          running    41087    7bf2e61a...    eyJhIjoiZWYz...
 dev              stopped    -        c4d9a7b3...    eyJhIjoiNDFk...
╶──────────────────────────────────────────────────────────────────────╴

╶──────────────────────────────────────────────────────────────────────╴
 j/k navigate  s start  x stop  r restart  a add  e edit  n rename
 d delete  l logs  I import  ? help  q quit
```

## Install

```bash
brew tap dorky-robot/tap
brew install dorky-robot/tap/tunnels
```

Or build from source:

```bash
cargo install --path .
```

## Usage

```bash
tunnels          # Launch TUI
tunnels list     # List tunnels (non-interactive)
tunnels import   # Import existing cloudflared plists
```

## TUI Keybindings

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate up/down |
| `s` | Start tunnel |
| `x` | Stop tunnel |
| `r` | Restart tunnel |
| `a` | Add new tunnel |
| `e` | Edit token |
| `n` | Rename tunnel |
| `d` | Delete tunnel |
| `l` | View logs |
| `I` | Import existing plists |
| `?` | Help |
| `q` | Quit |

## How it works

Each tunnel is a cloudflared instance managed via macOS LaunchAgents (`~/Library/LaunchAgents/`). No sudo required.

- **Config** stored at `~/.config/tunnels/config.json`
- **Plists** generated in `~/Library/LaunchAgents/`
- **Logs** written to `~/Library/Logs/tunnels/`
- Tunnels **auto-start at login** via `RunAtLoad`
- Import discovers existing plists from both `~/Library/LaunchAgents/` and `/Library/LaunchDaemons/`

## Adding a tunnel

1. Go to [Cloudflare Zero Trust](https://one.dash.cloudflare.com/) → Networks → Tunnels
2. Create a tunnel and copy the token
3. In the TUI, press `a`, enter a name and paste the token
4. Press `s` to start

## License

MIT
