# tunnels

A k9s-style TUI for managing cloudflared tunnels and local services on macOS.

## Architecture

- **config.rs** — JSON config at `~/.config/tunnels/config.json`, tunnel + service CRUD, token decode. Services have optional `memo` field.
- **launchd.rs** — LaunchAgent plist generation, start/stop/status via `launchctl`, plist discovery/migration
- **cloudflare.rs** — CF API integration via API tokens (multi-account + per-tunnel), tunnel details + ingress route fetch, route add/remove with DNS management, auto-match tokens to accounts
- **scan.rs** — Service discovery via `lsof`: find listening TCP ports, resolve project names from process cwd
- **app.rs** — App state, tab system (Services/Tunnels), mode machine, CF + scan integration, submenu toggle
- **ui.rs** — ratatui rendering: tab header, tunnel/service tables, dialogs, two-level keybinding bar
- **main.rs** — crossterm event loop, key handlers per mode, full CLI subcommands (tunnel lifecycle, routes, services, tokens, sync)

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
| m | Rename URL (subdomain) |
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
# TUI
tunnels                              # Launch TUI

# Tunnel lifecycle
tunnels list [--json]                # List tunnels
tunnels start <name>                 # Start a tunnel
tunnels stop <name>                  # Stop a tunnel
tunnels restart <name>               # Restart a tunnel
tunnels logs <name> [--lines N]      # View tunnel logs
tunnels add <name> --token <token>   # Add a new tunnel
tunnels rm <name>                    # Delete a tunnel
tunnels rename <old> <new>           # Rename a tunnel
tunnels import                       # Import existing plists

# Routes
tunnels routes [TUNNEL] [--json]     # List ingress routes
tunnels route add <host> <port> --tunnel <name>  # Idempotent
tunnels route rm <host> --tunnel <name>
tunnels route mv <old> <new> --tunnel <name>

# Services
tunnels service list [--json]        # List tracked services
tunnels service add <name> --port <p> [--tunnel <t>] [--memo <m>]
tunnels service rm <name>            # Remove a service
tunnels service edit <name> [--port <p>] [--tunnel <t>] [--memo <m>]
tunnels service scan                 # Scan for listening ports

# Tokens & sync
tunnels token add <token>            # Add CF API token
tunnels token edit <tunnel> --token <token>  # Set per-tunnel token
tunnels sync                         # Sync from Cloudflare API
```
