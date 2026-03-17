# tunnels

A k9s-style TUI for managing cloudflared tunnels and local services on macOS.

## Architecture

- **config.rs** — JSON config at `~/.config/tunnels/config.json`, tunnel + service CRUD, token decode. Services have optional `memo` field.
- **launchd.rs** — LaunchAgent plist generation, start/stop/status via `launchctl`, plist discovery/migration
- **cloudflare.rs** — CF API integration via API tokens (multi-account + per-tunnel), tunnel details + ingress route fetch, route add/remove with DNS management, auto-match tokens to accounts
- **scan.rs** — Service discovery via `lsof`: find listening TCP ports, resolve project names from process cwd
- **app.rs** — App state, unified tree view model (UnifiedRow), mode machine with prefix keys + context menu, CF + scan integration
- **ui.rs** — ratatui rendering: tree table (tunnels with nested services), context menu overlay, prefix-aware keybinding bar, dialogs
- **main.rs** — crossterm event loop, unified key handler with context-sensitive dispatch, prefix key handler, context menu handler, full CLI subcommands

## Build & Install

```
cargo build --release
cp target/release/tunnels ~/.local/bin/
```

Or via Homebrew: `brew install dorky-robot/tap/tunnels`

## Tree View

Single unified view: tunnels as parent rows (▼/▶) with services nested underneath. Unlinked services appear under a separator.

## Key Bindings

### Normal mode (context-sensitive)
| Key | On Tunnel | On Service |
|-----|-----------|------------|
| j/k | Navigate | Navigate |
| Enter | Context menu | Context menu |
| Space/←/→ | Toggle expand/collapse | (no-op) |
| s | Start tunnel | — |
| x | Stop tunnel | — |
| r | Restart tunnel | — |
| e | Edit connector token | Edit service |
| n | Rename tunnel | Rename URL |
| d | Delete tunnel | Untrack service |
| l | View logs | — |
| m | Manage routes | — |
| a | Prefix: add... | Prefix: add... |
| t | Prefix: token... | Prefix: token... |
| g | Prefix: global... | Prefix: global... |
| ? | Help | Help |
| q/Esc | Quit | Quit |

### Prefix keys
**a → add...**
| Key | Action |
|-----|--------|
| t | Add tunnel |
| s | Add service |
| r | Add route (tunnel selected) |
| Esc | Cancel |

**t → token...**
| Key | Action |
|-----|--------|
| c | Edit connector token |
| a | Add CF API token |
| Esc | Cancel |

**g → global...**
| Key | Action |
|-----|--------|
| s | Sync from Cloudflare |
| p | Scan listening ports |
| i | Import existing plists |
| Esc | Cancel |

### Context menu (Enter)
Opens a floating menu with actions for the selected row. Navigate with j/k + Enter, or press the shortcut letter.

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
