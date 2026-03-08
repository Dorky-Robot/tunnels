Commit, push, create a PR, run review agents, fix issues, and merge for tunnels.

## Step 1: Prepare the branch

Check the current git state:

```bash
git status
git branch --show-current
```

**If on main/master:**
1. Create a feature branch from the changes:
   ```bash
   git checkout -b <descriptive-branch-name>
   ```
2. Stage and commit all changes with a clear commit message.

**If on a feature branch:**
1. Stage and commit any uncommitted changes.
2. If there are no uncommitted changes, continue to Step 2.

## Step 2: Push and create (or update) the PR

```bash
git push -u origin <branch-name>
```

Check if a PR already exists for this branch:

```bash
gh pr view <branch-name> --json number,url 2>/dev/null
```

**If no PR exists**, create one:

```bash
gh pr create --title "<concise title>" --body "## Summary

<1-3 bullet points describing the changes>

## Test plan

- [ ] `cargo build` succeeds
- [ ] `cargo clippy --all-targets -- -D warnings` passes
- [ ] `cargo test --workspace` passes (if tests exist)
- [ ] Manual TUI smoke test
"
```

**If a PR already exists**, note its number and continue.

## Step 3: Review-fix loop

Repeat until all agents approve:

### 3a. Gather the diff

```bash
gh pr diff <PR-number>
```

### 3b. Launch review agents in parallel

Send a **single message** with Task tool calls so they run concurrently. Each agent receives the PR description and full diff.

Launch these review agents:

1. **Security reviewer** (`security-reviewer` agent) — Token handling, command injection via shell-outs to curl/launchctl/lsof/sudo, plist XML injection, credential exposure in logs.
2. **Correctness reviewer** (`correctness-reviewer` agent) — Mode state machine transitions, LaunchAgent lifecycle edge cases, config file consistency, selection index bounds.
3. **Code quality reviewer** (`code-quality-reviewer` agent) — Rust idioms, error handling with anyhow, ratatui rendering patterns, unnecessary clones, test coverage.
4. **Architecture reviewer** (`architecture-reviewer` agent) — Module boundaries (app/ui/config/launchd/cloudflare/scan), dependency direction, public API surface.

Each agent must end with a verdict:

```
VERDICT: APPROVE
VERDICT: APPROVE_WITH_NOTES
VERDICT: REQUEST_CHANGES
```

### 3c. Compile and post the review

Combine agent responses into a review summary and post it:

```bash
gh pr comment <PR-number> --body "<review summary>"
```

### 3d. Fix any issues

If any agent returned `REQUEST_CHANGES`:
1. Fix the issues they identified.
2. Commit and push the fixes.
3. Return to step 3a.

If all agents returned `APPROVE` or `APPROVE_WITH_NOTES`, continue to Step 4.

## Step 4: Merge

```bash
gh pr merge <PR-number> --squash --delete-branch
```

Print the merged PR URL.
