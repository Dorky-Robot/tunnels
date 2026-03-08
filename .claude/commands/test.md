Build, lint, and test tunnels.

Run these checks in sequence, stopping on first failure:

## Step 1: Format check

```bash
cargo fmt --all -- --check
```

If formatting fails, run `cargo fmt --all` to fix, then report what changed.

## Step 2: Clippy

```bash
cargo clippy --all-targets -- -D warnings
```

If clippy fails, fix the issues and re-run.

## Step 3: Build

```bash
cargo build
```

## Step 4: Test

```bash
cargo test --workspace
```

## Step 5: Report

Print a summary:
- Format: pass/fail
- Clippy: pass/fail (list any warnings fixed)
- Build: pass/fail
- Tests: pass/fail (N passed, N failed)
