Consult the masters — review the tunnels codebase through the lens of great software engineers.

## Phase 1: Map the Codebase

Thoroughly explore the full project structure. Read ALL source files in `src/`:

- `src/main.rs` — crossterm event loop, key handlers, CLI subcommands
- `src/app.rs` — App state machine, 12-variant Mode enum, tunnel/service/route orchestration
- `src/config.rs` — JSON config persistence, Tunnel/Service/Config types, token decode
- `src/launchd.rs` — macOS LaunchAgent plist generation, launchctl start/stop/status
- `src/cloudflare.rs` — CF API via curl shell-outs, tunnel details + ingress routes
- `src/scan.rs` — Service discovery via lsof, port scanning, project name resolution
- `src/ui.rs` — ratatui rendering, tables, dialogs, context menus, keybinding bar

Also read: `Cargo.toml`, `CLAUDE.md`

Tech stack: Rust (edition 2024), ratatui, crossterm, serde/serde_json, anyhow, base64, dirs. macOS-only. No async runtime. All I/O is synchronous shell-outs.

## Phase 2: Launch Review Agents in Parallel

Send a single message with 8 Task tool calls so they run concurrently. Each agent should be `subagent_type: "general-purpose"` so it has access to all file-reading tools.

**IMPORTANT**: Tell each agent to read all source files in `src/` directly before forming their review.

Shared context to include in every agent prompt:
```
Project: tunnels — a Rust TUI (ratatui + crossterm) for managing cloudflared tunnels on macOS.
Modules: main.rs (event loop), app.rs (state machine), config.rs (persistence), launchd.rs (LaunchAgent lifecycle), cloudflare.rs (CF API), scan.rs (lsof service discovery), ui.rs (rendering).
Key patterns: Mode enum state machine, synchronous shell-outs via std::process::Command, JSON config at ~/.config/tunnels/config.json, per-tunnel CF API tokens.
Read ALL source files in src/ before forming your review.
Report your top 5 findings ranked by impact. For each finding, cite the specific file and line.
Do NOT suggest changes that would reduce capabilities or fight Rust idioms.
```

### Agent 1: Rich Hickey — Simplicity & Data Orientation
Look for complecting (state mixed with identity, I/O mixed with logic), unnecessary abstractions over plain data, mutable state where values would be clearer.

### Agent 2: Alan Kay — Message Passing & Late Binding
Evaluate whether the Mode enum state machine is the right abstraction or if message passing between components would be cleaner. Check if objects/modules communicate through clear boundaries.

### Agent 3: Eric Evans — Domain-Driven Design
Check if the code speaks the cloudflared/Cloudflare domain language. Are Tunnel, Service, Route the right domain concepts? Is the bounded context between local tunnels and CF API clear?

### Agent 4: Composition & Functional Design
Look for pure core / impure shell separation. Can business logic in app.rs be separated from I/O? Are there partial functions that could be made total with better types?

### Agent 5: Joe Armstrong — Fault Tolerance & Isolation
What happens when curl hangs? When launchctl fails? When config.json is corrupted? Is error recovery isolated or does one failure cascade?

### Agent 6: Sandi Metz — Practical Object Design
Check single responsibility (app.rs is ~650 lines with many concerns). Look for Tell Don't Ask violations, dependency direction issues, and cost of change.

### Agent 7: Leslie Lamport — State Machines & Temporal Reasoning
Enumerate all Mode states and valid transitions. Are there unreachable states? Race conditions between config save and launchctl operations? Invariants that aren't enforced?

### Agent 8: Kent Beck — Simple Design & Courage to Change
Apply the four rules. What's the simplest design that could work? What YAGNI exists? What bold simplification would make the codebase clearer?

## Phase 3: Distill

Cross-reference findings across agents. Prioritize by consensus. Filter out suggestions that fight Rust idioms or add complexity without payoff.

## Phase 4: Build the Execution Plan

Create a phased plan grouped into tiers:
- **Tier 1**: Critical fixes (bugs, safety)
- **Tier 2**: Type safety & cleanup
- **Tier 3**: Structural improvements
- **Tier 4**: Architectural evolution

## Phase 5: Present Plan and Get Feedback

Present the plan and ask the user how to proceed before implementing anything.

## Phase 6: Execute

Work through approved tiers, committing after each phase.

## Phase 7: Ship

Run full test suite, create feature branch, run `/ship-it`.
