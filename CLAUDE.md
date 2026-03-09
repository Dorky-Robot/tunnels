# tunnels

A k9s-style TUI for managing cloudflared tunnels and local services on macOS.

## Architecture

- **config.rs** — JSON config at `~/.config/tunnels/config.json`, tunnel + service CRUD, token decode. Services have optional `memo` field.
- **launchd.rs** — LaunchAgent plist generation, start/stop/status via `launchctl`, plist discovery/migration
- **cloudflare.rs** — CF API integration via API tokens (multi-account + per-tunnel), tunnel details + ingress route fetch, route add/remove with DNS management, auto-match tokens to accounts
- **scan.rs** — Service discovery via `lsof`: find listening TCP ports, resolve project names from process cwd
- **app.rs** — App state, tab system (Services/Tunnels), mode machine, CF + scan integration, submenu toggle
- **ui.rs** — ratatui rendering: tab header, tunnel/service tables, dialogs, two-level keybinding bar
- **main.rs** — crossterm event loop, key handlers per mode, CLI subcommands (`list`, `import`, `routes`, `route add/rm`)

## Build & Install

```
cargo build --release
cp target/release/tunnels ~/.local/bin/
```

Or via Homebrew: `brew install dorky-robot/tap/tunnels`

## Tab Order

- **Tab 1** (default): Services
- **Tab 2**: Tunnels
- Switch with `1`/`2` or left/right arrows

## Key Bindings

### Services Tab (primary bar)
| Key | Action |
|-----|--------|
| j/k | Navigate |
| a | Add service |
| e | Edit service |
| d | Untrack service |
| . | Toggle secondary actions |
| q | Quit |

### Services Tab (secondary bar, press `.`)
| Key | Action |
|-----|--------|
| S | Scan listening ports |
| R | Sync from Cloudflare API |
| T | Add CF API token |

### Tunnels Tab (primary bar)
| Key | Action |
|-----|--------|
| j/k | Navigate |
| s/x/r | Start/stop/restart tunnel |
| m | Manage routes (subdomains) |
| a | Add new tunnel |
| d | Delete tunnel |
| . | Toggle secondary actions |
| q | Quit |

### Tunnels Tab (secondary bar, press `.`)
| Key | Action |
|-----|--------|
| e | Edit token |
| n | Rename tunnel |
| l | View logs |
| R | Sync from Cloudflare API |
| T | Add CF API token |
| I | Import existing plists |

## CLI Subcommands

```
tunnels                    # Launch TUI
tunnels list [--json]      # List tunnels
tunnels routes [TUNNEL]    # List ingress routes
tunnels route add <hostname> <port> --tunnel <name>  # Idempotent
tunnels route rm <hostname> --tunnel <name>
tunnels import             # Import existing plists
```
