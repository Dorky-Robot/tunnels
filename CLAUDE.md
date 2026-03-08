# tunnels

A k9s-style TUI for managing cloudflared tunnels and local services on macOS.

## Architecture

- **config.rs** — JSON config at `~/.config/tunnels/config.json`, tunnel + service CRUD, token decode
- **launchd.rs** — LaunchAgent plist generation, start/stop/status via `launchctl`, plist discovery/migration
- **cloudflare.rs** — CF API integration via API tokens (multi-account), tunnel details + ingress route fetch, auto-match tokens to accounts
- **scan.rs** — Service discovery via `lsof`: find listening TCP ports, resolve project names from process cwd
- **app.rs** — App state, tab system (Tunnels/Services), mode machine, CF + scan integration
- **ui.rs** — ratatui rendering: tab header, tunnel/service tables, dialogs, keybinding bar
- **main.rs** — crossterm event loop, key handlers per mode, CLI subcommands (`list`, `import`)

## Build & Install

```
cargo build --release
cp target/release/tunnels ~/.local/bin/
```

## Key Bindings

| Key | Action |
|-----|--------|
| 1/2 | Switch tabs (Tunnels/Services) |
| j/k | Navigate |
| s | Start tunnel |
| x | Stop tunnel |
| r | Restart tunnel |
| a | Add new tunnel/service |
| e | Edit token/service |
| d | Delete tunnel / untrack service |
| l | View logs |
| S | Scan listening ports (Services tab) |
| R | Sync from Cloudflare API (both tabs) |
| T | Add CF API token (both tabs) |
| I | Import existing plists |
| ? | Help |
| q | Quit |
