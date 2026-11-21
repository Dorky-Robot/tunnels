# Roll, list or delete a Cloudflare API token

**Use when:** an API token (`cfut_…`, the kind `tunnels token list` shows) leaked or is due a new
value; an old one must be found and deleted; or you want to know which box runs on which token.
**Don't use when:** it is a tunnel's connector token (the long `eyJ…` string): that is
`rotate-a-leaked-token.md`.
**Scope:** [cloudflare + the box whose token rolls]. **No undo:** a rolled token's old value is dead
at once, and a deleted token is gone.
**Needs:** the mini reachable over the tailnet. It is the one box that holds a token allowed to manage
tokens (Felix, 2026-11-21: "let's set up felix mac mini"), one per Cloudflare login, in
`~/.config/cf-tokens/<login>.token`, readable by that account only. No other box can do any of this:
their own tokens lack User › API Tokens, on purpose.

Everything is `mesh/cf-tokens`, installed on the mini as `~/.local/bin/cf-tokens`. It never prints a
token's value. `<login>` is the fleet's account alias, `dorkyrobot` or `felixflor`.

## Look

```sh
ssh macmini cf-tokens list
```

One line a token: its id, when it was made and last used, and which box runs on it. **NO BOX RUNS ON
IT** is a candidate for deleting; read its name and dates first, since a token may serve something
that is not a box of ours (a CI job, a script of Felix's).

## Delete one no box runs on

```sh
ssh macmini cf-tokens delete <login> <id>          # a preview
ssh macmini cf-tokens delete <login> <id> --yes
```

It refuses a token a box runs on, the manager itself, and any delete while a box does not answer.

## Roll a box's token

```sh
ssh macmini cf-tokens roll <login> <box>           # a preview
ssh macmini cf-tokens roll <login> <box> --yes
ssh macmini cf-tokens roll <login> --all --yes     # every box in turn, stopping at the first that fails
```

Cloudflare gives the token a new value (its id and permissions stay), and the mini hands it to the box
over ssh's stdin: the box's `tunnels token add` keeps it and `tunnels token rm` forgets the old one.
**Check:** `✓ <box> runs on the new value; the old one is dead`.
**If the hand-over fails** the new value waits on the mini (`~/.config/cf-tokens/pending/`), the box's
tunnels keep running (connectors do not use the API token) but its `tunnels` cannot change Cloudflare
until it has it: `ssh macmini cf-tokens deliver <login> <box>` once the box is reachable.

Nothing restarts: an API token carries no connections, so no path you stand on is cut.

## The manager token itself

Only Felix makes it: signed in to that Cloudflare login, My Profile → API Tokens → Create Token → the
**Create Additional Tokens** template → Create. He types it at the mini's hidden prompt, never into a
chat, a note or a file:

```sh
ssh -t macmini cf-tokens receive <login>
```

It may mint a token with any permission that login has, which is why it lives on one box, in one
file, and no session copies it anywhere. If the mini is lost or that file is seen, delete the manager
in the dashboard first; every box's own token keeps working.

## Install or update

From the tunnels landing checkout, after a landing that touched it:

```sh
git show main:mesh/cf-tokens | ssh macmini 'cat > ~/.local/bin/cf-tokens.new && chmod 755 ~/.local/bin/cf-tokens.new && mv ~/.local/bin/cf-tokens.new ~/.local/bin/cf-tokens'
```

## History

- 2026-11-21: written with the script. Proved without a manager token: the mini asked all six boxes
  which token each runs on, for both logins, and refused to roll with nothing to roll with.
