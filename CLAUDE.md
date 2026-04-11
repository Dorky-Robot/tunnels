# tunnels

A TUI for managing cloudflared tunnels and local services on macOS. Maps local ports to public URLs.

## Architecture

- **config.rs** — JSON config at `~/.config/tunnels/config.json`, tunnel + service CRUD, token decode. Services have optional `memo` field.
- **launchd.rs** — LaunchAgent plist generation, start/stop/status via `launchctl`, plist discovery/migration
- **cloudflare.rs** — CF API integration via API tokens (multi-account + per-tunnel), tunnel details + ingress route fetch, route add/remove with DNS management, auto-match tokens to accounts
- **scan.rs** — Service discovery via `lsof`: find listening TCP ports, resolve project names from process cwd
- **route_import.rs** — Pure parse/retarget/group helpers for `tunnels route import` (JSON on stdin, idempotent via `cloudflare::add_route`)
- **app.rs** — App state, flat port list model (PortRow + Health), mode machine, link/unlink flows, settings modal, CF sync
- **ui.rs** — ratatui rendering: flat port list, settings modal overlay, keybinding bar, dialogs
- **main.rs** — crossterm event loop, 9-key normal mode handler, settings/link/unlink handlers, full CLI subcommands

## Build & Install

```
cargo build --release
cp target/release/tunnels ~/.local/bin/
```

Or via Homebrew: `brew install dorky-robot/tap/tunnels`

## TUI View

Flat list of services showing: PORT, NAME, URL (if linked), HEALTH GLYPH. Primary action is `Enter` to link a port to a URL. Tunnels are infrastructure managed via `.` settings modal.

### Health Glyphs
- `✓` — Linked, tunnel running, edge connected
- `✗` — Linked but unhealthy
- `●` — Not linked

## Key Bindings (9 keys)

| Key | Action |
|-----|--------|
| j/k | Navigate |
| Enter | Link port to URL (or edit existing) |
| d | Unlink (if linked) / Remove (if not) |
| a | Add a service |
| l | View tunnel logs |
| . | Settings (tokens, tunnels, scan, import) |
| ? | Help |
| q | Quit |

### Settings modal (`.`)
Navigable list with API tokens, tunnels, and actions (add token, add tunnel, scan ports, import plists, sync CF). Enter to select, d to remove, Esc to close.

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
tunnels route import [--tunnel <name>] [--dry-run]  # Reads JSON from stdin
  # e.g. tunnels routes mac-mini --json | tunnels route import --tunnel home-mesh

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
