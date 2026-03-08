# tunnels

A k9s-style TUI for managing cloudflared tunnels and local services on macOS.

## Architecture

- **config.rs** — JSON config at `~/.config/tunnels/config.json`, tunnel + service CRUD, per-tunnel API tokens, legacy token migration, token decode
- **launchd.rs** — LaunchAgent plist generation, start/stop/status via `launchctl`, plist discovery/migration
- **cloudflare.rs** — CF API integration via per-tunnel API tokens (multi-account), tunnel details + ingress route fetch, token verification
- **scan.rs** — Service discovery via `lsof`: find listening TCP ports, resolve project names from process cwd
- **app.rs** — App state, 3-tab system (Tunnels/Services/Routes), mode machine, context menus, CF + scan integration
- **ui.rs** — ratatui rendering: tab header, tunnel/service/route tables, dialogs, context menus, keybinding bar
- **main.rs** — crossterm event loop, key handlers per mode, CLI subcommands (`list`, `import`)

## Key Concepts

- Two token types: **cloudflared token** (base64, starts the tunnel) and **CF API token** (reads tunnel config/routes from CF API)
- API tokens live on tunnel entries (`api_token` field), shared across tunnels in the same CF account
- Context menus (Enter on selected row) for tunnel/service actions including T (Add/Change API token) and X (Remove API token)
- Multi-account support: tunnels grouped by account_id, API token set on one tunnel propagates to all in same account

## Build & Install

```
cargo build --release
cp target/release/tunnels ~/.local/bin/
```

## Key Bindings

| Key | Action |
|-----|--------|
| 1/2/3 | Switch tabs (Tunnels/Services/Routes) |
| j/k | Navigate |
| Enter | Open context menu (actions) |
| a | Add new tunnel/service |
| d | Delete tunnel / untrack service |
| S | Scan listening ports (Services tab) |
| R | Sync from Cloudflare API |
| I | Import existing plists |
| ? | Help |
| q | Quit |

### Tunnel Context Menu (Enter)
s/x/r (start/stop/restart), e (edit token), n (rename), l (logs), T (add/change API token), X (remove API token), d (delete)
