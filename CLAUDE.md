# tunnels

A k9s-style TUI for managing multiple cloudflared tunnel instances on macOS.

## Architecture

- **config.rs** — JSON config at `~/.config/tunnels/config.json`, token decode
- **launchd.rs** — LaunchDaemon plist generation, start/stop/status via `launchctl`
- **app.rs** — App state, mode machine (Normal, Adding, Editing, Confirming, Logs, Help)
- **ui.rs** — ratatui rendering, dialogs, keybinding bar
- **main.rs** — crossterm event loop, CLI fallback (`tunnels list`, `tunnels import`)

## Build & Install

```
cargo build --release
cp target/release/tunnels ~/.local/bin/
```

## Key Bindings

| Key | Action |
|-----|--------|
| j/k | Navigate |
| s | Start tunnel |
| x | Stop tunnel |
| r | Restart tunnel |
| a | Add new tunnel |
| e | Edit token |
| d | Delete tunnel |
| l | View logs |
| I | Import existing plists |
| ? | Help |
| q | Quit |
