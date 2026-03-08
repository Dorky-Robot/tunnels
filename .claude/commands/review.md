Run a multi-perspective review on a pull request for tunnels.

## Step 1: Fetch the PR diff

```bash
gh pr diff $ARGUMENTS --repo $(gh repo view --json nameWithOwner --jq .nameWithOwner)
```

Also fetch the PR description for context:

```bash
gh pr view $ARGUMENTS --json title,body
```

## Step 2: Launch 4 review agents in parallel

Send a **single message** with 4 Task tool calls so they run concurrently. Each agent receives the PR description and full diff.

1. **Security reviewer** (`security-reviewer` agent) — Token handling, command injection via curl/launchctl/lsof/sudo, plist XML injection, credential exposure.

2. **Architecture reviewer** (`architecture-reviewer` agent) — Module boundaries (app/ui/config/launchd/cloudflare/scan), dependency direction, public API surface, config as source of truth.

3. **Correctness reviewer** (`correctness-reviewer` agent) — Mode state machine transitions, LaunchAgent lifecycle, config consistency, selection bounds, CF API error handling.

4. **Code quality reviewer** (`code-quality-reviewer` agent) — Rust idioms, anyhow error handling, ratatui patterns, cloning, test coverage.

Each agent must end with a verdict: `VERDICT: APPROVE`, `VERDICT: APPROVE_WITH_NOTES`, or `VERDICT: REQUEST_CHANGES`.

## Step 3: Synthesize verdicts

```
## Review Summary for PR #<N>

### Security
<verdict> — <key findings or "No issues">

### Architecture
<verdict> — <key findings or "No issues">

### Correctness
<verdict> — <key findings or "No issues">

### Code Quality
<verdict> — <key findings or "No issues">

### Overall
<APPROVE / APPROVE_WITH_NOTES / REQUEST_CHANGES>
<1-2 sentence summary>
```

## Step 4: Post as PR comment

```bash
gh pr comment $ARGUMENTS --body "<the review summary>"
```
