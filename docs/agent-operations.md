# Running Cloudflare with agents: plan

*Status: agreed plan, 2026-09-24; every decision is in
[Decisions](#decisions). Phases 1 and 2 (`tunnels cf`) are built and shipped in
0.17.0, with mesh forwarding in 0.18.0; phase 3a (the agent side of the
admin UI) in 0.19.0. Phase 3b waits on an admin API token and the pocket-id
client.*

## The idea in one paragraph

Everything we do in the Cloudflare dashboard should be doable by an AI agent,
safely, without the agent handling secrets and without a person clicking. Not
with Terraform, and not with procedural scripts, because both fail the same
way: they assume the world matches their model, and when it doesn't they
report success anyway. The failures we actually hit had that shape: a DNS error
that blamed permissions when the real cause was the account, two machines
overwriting each other's edits, launchd deferring the agent's start, a Mac that
had lost its gateway. An agent that reads what comes back noticed each one.
So the plan has three layers, each playing to that strength:

| Layer | What it is | What it is for |
|---|---|---|
| **1. `tunnels`** | the CLI, the fleet file, the agents | what changes often and needs guardrails: tunnels, routes, DNS, healing, failover |
| **2. `tunnels cf`** | a guarded pass-through to the whole Cloudflare API | everything `tunnels` does not model: Access, SSL, WAF, tokens, zone settings |
| **3. Runbooks** | `docs/runbooks/*.md`, written for agents to follow | multi-step jobs, where each step is followed by a check on the result |

When a runbook gets used often, or keeps going wrong in the same place, its
steps move down a layer: from documented API calls to a real `tunnels`
command with a plan step and tests. That is how routes, forget, destroy and
rotate got here.

## Principles

1. **Check each step's result.** Every change is followed by a read that
   confirms it happened. A step whose check fails stops the job.
2. **Agents never see secrets.** They name an account, a zone, a tunnel;
   `tunnels` picks the token. An agent pulling tokens out of `config.json`
   got blocked as "credential exploration", and it was right to be.
3. **Every change states where it acts**, as every `tunnels` command already
   does: `[read-only]`, `[this Mac only]`, `[cloudflare]`, and so on.
4. **Reading is free; writing is deliberate.** Reads need no flag. Writes need
   `--yes`, and anything that deletes or takes something over needs a person
   to have said so in the conversation.
5. **Every write can be undone.** Before changing something, record what it
   was.
6. **Unknown is not the same as missing.** If a read failed, the answer is
   "unknown", and nothing is removed on the strength of it.
7. **Say when to stop.** Runbooks list the signs that mean "ask a person".
   Agents are good at noticing surprises; the runbook says which surprises
   matter.

## Today (what already exists)

- `tunnels status | plan | apply | doctor`, with `--json`. Scope is shown on
  every command.
- The fleet file, replicated between agents over the tailnet; all six
  machines on 0.16.6.
- Guardrails: live takeovers need `--yes`, removing what the file doesn't
  mention needs `--prune`, destroying a tunnel needs `--allow-destroy`, and a
  failed DNS step rolls back its ingress change.
- `tunnel forget | destroy | rotate` with honest scope.
- The web UI on each agent, tailnet only.
- `tunnels cf` (phases 1–2, 0.17.0): placeholders, token picking, hidden
  secrets, previews until `--yes`, a per-machine log with before/after,
  `cf undo`, refusal of what `tunnels` owns, and every machine's log in the
  web UI. Tested against the fake Cloudflare, and once for real: a TXT record
  on sarameig.gs was created, changed, and both steps undone.

## Layer 2: `tunnels cf`

### Shape

```
tunnels cf <METHOD> <path> [--data JSON | --data-file F] [--yes] [--json]
```

```sh
# reads
tunnels cf GET /zones/{zone:everyday.vet}/settings/ssl
tunnels cf GET /accounts/{account:dorkyrobot}/access/apps
tunnels cf GET /accounts/{account:felixflor}/cfd_tunnel/{tunnel:mac2019-felixflor}

# a write: shows the before state and the request, changes nothing without --yes
tunnels cf PATCH /zones/{zone:everyday.vet}/settings/ssl --data '{"value":"strict"}'
tunnels cf PATCH /zones/{zone:everyday.vet}/settings/ssl --data '{"value":"strict"}' --yes

# undo a logged change
tunnels cf log                      # recent writes, newest first
tunnels cf undo <id>                # replays the recorded before state
```

### Placeholders

Agents write names, and `tunnels` resolves them against the fleet file and
Cloudflare:

| Placeholder | Resolves to | From |
|---|---|---|
| `{account:<alias>}` | account id | fleet `accounts` |
| `{zone:<name>}` | zone id | a zone lookup through that account's token |
| `{tunnel:<alias or id>}` | tunnel id | fleet `tunnels`, or Cloudflare by name when that is unambiguous |
| `{record:<hostname>}` | DNS record id | a DNS lookup in the hostname's zone |

The resolved path is printed before anything runs, so the agent sees exactly
what it is about to touch. An ambiguous name (two tunnels called
`DorkyRobot2`) is an error that lists the candidates, never a guess.

### Who may write

Any machine in the fleet. It is a mesh: no machine is special, and each
writes with the API tokens it holds. A machine without a token for an
account simply cannot write there, and the error says which token to add.

Built in 0.18.0: a machine with no token for an account sends the call to a
peer's agent over the tailnet (`/api/cf-forward`). The token stays where it
is, and the peer makes the call with the same guardrails and logs it with
`requested_by`. `policy.remote_from` in the fleet file lists which machines
may ask; the fleet sets it to every machine except doug-mini.

### Picking the token

The account comes from the path (`{account:…}`, or the account that owns
`{zone:…}`). `tunnels` uses the first token on this machine that can reach
that account. If none can, the error names the account and the permission to
add. The agent never sees the token, and it never appears in argv or logs.

### Safety

| Request | Rule |
|---|---|
| `GET` | always allowed |
| `PATCH`, `PUT`, `POST` | preview by default; `--yes` to send |
| `DELETE` | preview by default; `--yes` to send, and the preview includes what will be lost |
| anything under `/user/tokens` or `/accounts/*/tokens` | `--yes` plus `--i-mean-tokens`; the response is redacted |
| paths `tunnels` owns (tunnel `configurations`, tunnel `DNS` CNAMEs) | refused, with a pointer to the `tunnels` command that does it with guardrails |

The last rule matters. `tunnels cf` must not become a way around the fleet
file. A route changed behind the fleet file's back is exactly the drift the
agents exist to undo, so they would undo it.

### The before state and undo

For every write, `tunnels cf`:

1. `GET`s the same resource first (or its parent collection, for a `POST`)
   and records it;
2. sends the request;
3. `GET`s it again and records the after state;
4. prints a short diff of before and after.

Records go to `~/.config/tunnels/cf-log/<timestamp>-<id>.json` **on the
machine that made the change**. They are never replicated. Each record holds
the method, the resolved path, the request body, before, after, machine, and
fleet serial. The agent serves its machine's log at `/api/cf-log`, and the web
UI gathers the logs from every peer into one timeline, so each change is
visible in one place while still stored where it was made. `undo` runs on the
machine that holds the record, because that machine has the token that made
the change.

`undo` works per method:

| Write | Undo |
|---|---|
| `PATCH`/`PUT` | `PUT` or `PATCH` the before state |
| `POST` (created X) | `DELETE` X |
| `DELETE` | `POST` the before state; possible for most resources, and the preview says when it is not |

Where there is no good before read (some endpoints are write-only), the
preview says "not undoable" and `--yes` alone is not enough.

### Output

Human output shows the resolved path, the scope, the diff, and the check.
`--json` returns `{scope, method, path, resolved_path, before, after,
response, log_id}` so an agent can read the result without parsing text.

## Layer 3: runbooks

### Where they live

`docs/runbooks/<verb>-<thing>.md`, one job per file, with an index at
`docs/runbooks/README.md` that an agent reads first. `CLAUDE.md` points to
that index.

### What every runbook contains

```markdown
# <What this does, in a line>

**Use when:** …            **Don't use when:** …
**Scope:** [cloudflare]     **Undo:** section at the bottom
**Needs:** API token with <permission> for <account>; <machine> reachable

## Before you start
- [ ] `tunnels plan` exits 0            (so you are not mixing this with other drift)
- [ ] <anything else that must be true>

## Steps
1. <command>
   **Check:** <command>; you should see <what>.
   **If not:** <what it means; stop / try X>
2. …

## Stop and ask a person if
- <surprise that means the model in this runbook is wrong>

## Undo
- <commands, in reverse order>

## History
- <date>: <what went wrong last time, and what changed in this file>
```

The **History** section is how a runbook improves. When an agent hits
something the runbook didn't expect, the fix goes into the runbook along with
the reason, the same way the code comments in `tunnels` explain their
incidents.

### First runbooks

| Runbook | Layers it uses | Why first |
|---|---|---|
| `add-a-machine.md` | `tunnels` | done five times today; its failures (edits overwriting each other, the agent not starting, old Command Line Tools) are fresh and worth writing down |
| `publish-the-web-ui.md` | `tunnels` (route) + `tunnels cf` (Access login provider, app, policy) | the first real use of layer 2; see [The web UI on the internet](#the-web-ui-on-the-internet) |
| `retire-public-ssh.md` | `tunnels` | ssh becomes tailnet-only; removes the `ssh-*` hostnames and the `cloudflare-<name>` path in `mesh.conf` |
| `move-a-site.md` | `tunnels` | moving a hostname between machines, with a check at each step |
| `warm-a-standby.md` | `tunnels`, plus the app's own docs | the everyday.vet standby isn't warm; says what "warm" has to mean before failover is safe |
| `rotate-a-leaked-token.md` | `tunnels` (connector) + `tunnels cf` (API tokens) | the incident you want to be fast and calm |
| `retire-a-tunnel.md` | `tunnels` | adopt or destroy, the choice mac-sara is waiting on |

## When a runbook becomes a command

A runbook step becomes a `tunnels` command when any of these is true:

- agents run it several times a week;
- it has gone wrong twice in the same way;
- getting it wrong would take something down (then it wants a plan step and
  a test against the fake Cloudflare);
- it needs to happen without anyone asking (then it belongs to the agent).

Most things never meet any of these, and should stay documented API calls.

## Phases

| Phase | What | Done when |
|---|---|---|
| **1** ✓ | `tunnels cf` for reads: placeholders, token picking, `--json` | an agent can answer "what is the SSL mode on everyday.vet" without any token appearing in its transcript |
| **2** ✓ | `tunnels cf` writes: preview, `--yes`, before/after log, `cf log`, `cf undo`; refusal of paths `tunnels` owns | a PATCH and its undo round-trip against the fake Cloudflare in tests, and once for real on a throwaway setting |
| **3a** ✓ | agent side: verifying the Access token, `admins`, relaying machine actions across the mesh, a target-machine picker in the UI, recording who made each change | tested against a fake Access (its own signing keys): a non-admin gets the page without buttons and 403 on any write; an admin restarts a tunnel on another machine from the page |
| **3b** | Cloudflare side, waiting on the admin token and the pocket-id client: login provider, Access app, `tunnels.felixflor.es` route; runbook `publish-the-web-ui` | signed in as an admin at `tunnels.felixflor.es`, you can restart a tunnel on mini; signed in as anyone else, you can only look |
| **4** | the rest of the first runbooks; `CLAUDE.md` points agents to the index | each has been run once for real, and its History section has an entry |
| **5** | promote whatever phases 3–4 show is worth it | — |

## The web UI on the internet

Decision 4: ssh stays on the tailnet, and the web UI becomes reachable from
the internet through Cloudflare, with sign-in through id.felixflor.es.

### How it fits together

```
browser ──> tunnels.felixflor.es ──> Cloudflare Access ──> tunnel ──> agent :7630
                                          │
                                          └─ sign in with id.felixflor.es (pocket-id, OIDC)
```

- **Cloudflare Access sits in front of the hostname.** Nobody reaches the Mac
  until they have signed in with id.felixflor.es and passed the Access
  policy. For now the policy allows anyone who can sign in to pocket-id;
  narrowing it to a group later is a policy change, not a design change. pocket-id is added to Access as a generic OIDC login provider; it
  already publishes its OIDC configuration (issuer `https://id.felixflor.es`).
- **The agent checks Access's work.** Every request that comes through
  Cloudflare carries a `Cf-Access-Jwt-Assertion` header. The agent verifies
  that token itself: the signature against the team's public keys, the
  audience (this app's `aud` tag), the issuer, and the expiry. If the token
  is missing or bad the answer is 403. That way a mistake in the Access
  policy, or a second route to the same port, doesn't quietly publish the UI.
- **The internet view is read-only.** See below.
- **Tailnet access is unchanged.** Requests from tailnet and loopback addresses
  need no sign-in, as now. That is the break-glass path when Cloudflare,
  pocket-id or the public hostname is down.

### What changes in `tunnels`

| Today | After |
|---|---|
| The agent refuses any request carrying `Cf-Ray` and similar headers | It accepts one only with a valid Access token, as above |
| The fleet file refuses any route to the web port | It allows one only when `[policy.web]` declares its Access app: `public_host`, `team_domain`, `aud` |
| `/api/*` writes need `X-Tunnels: 1` | Unchanged. Through Cloudflare they also need a verified Access token whose email is in `[policy.web] admins` |
| Machine actions work only on the machine serving the page | Any machine: the serving agent relays them over the tailnet (`/api/relay/…`) to the target's agent |

New fleet settings (the hostname is decided; the other two come from the
Access app once it exists):

```toml
[policy.web]
public_host = "tunnels.felixflor.es"
team_domain = "<team>.cloudflareaccess.com"
aud = "<the Access app's audience tag>"
admins = ["felixflores@gmail.com"]   # must match the email on your pocket-id account
```

Verifying the token needs RS256 signature checks, which means one new
dependency (the `jsonwebtoken` crate, or `rsa` + `sha2`). The team's keys
come from `https://<team_domain>/cdn-cgi/access/certs`; the agent caches them
and refreshes them when an unknown key id shows up.

### An admin interface for the whole mesh, for listed emails

*Changed 2026-09-24: this was read-only; it is now the admin interface.*

- **Anyone pocket-id knows can sign in and look.** Only the emails in
  `[policy.web] admins` get the change buttons. The agent decides this from
  the verified Access token's `email` claim, not from the Access policy, so a
  loose policy can't hand out admin by mistake.
- **Admin means the whole mesh, not only the machine serving the page.**
  - Cloudflare actions (apply, prune, takeover, promote, failback,
    `tunnels cf` writes) run on the serving agent. They use mesh forwarding
    when it has no token for the account.
  - Machine actions (start, stop or restart a tunnel, view its logs, run an
    agent pass, relabel tokens) name a target machine. The serving agent
    relays them over the tailnet to that machine's agent, at
    `/api/relay/<action>`.
  - The receiving agent accepts a relay only from a fleet machine on
    `policy.remote_from`, checked by its tailnet address as `/api/cf-forward`
    does. It trusts the relaying agent to have checked the admin's identity.
- **Every change records who made it:** the admin's email plus "via
  tunnels.felixflor.es", in the cf log and in the agent's events.
- **Safeguards stay in the page.** Prune and takeover still ask for
  confirmation. Destroying a tunnel, rotating a tunnel's secret and
  changing API tokens stay CLI-only: they are irreversible, and one wrong
  click from a phone is not the place for them.
- **Changes need the `X-Tunnels: 1` header** through Cloudflare too, so a
  page on another site can't drive a signed-in browser.

### Which machine serves it

Only dorkyrobot2, through `dorkyrobot2-felixflor`, with no standby. pocket-id
runs there too, so if dorkyrobot2 is down, signing in would fail anyway. When
that happens, use the tailnet: every agent serves the same UI there.

### Before building

`tunnels.felixflor.es` doesn't exist in Cloudflare today: there is no DNS
record and no ingress for it on any tunnel, so creating it touches nothing
already running.

### What the runbook has to find out first

- **Is Zero Trust set up on the felixflor account, and what is its team
  domain?** The API token here cannot read the organization settings; it can
  see there are no Access apps and no login providers yet. If Zero Trust has
  never been enabled, turning it on (free plan, choosing a team name) may be
  the one dashboard step left.
- **The token permissions for layer 2:** Access: Organizations (Read),
  Identity Providers (Edit), Apps and Policies (Edit), on the felixflor
  account. Checked 2026-09-24: today's tokens can list Access apps and login
  providers, but not the organization, and not zone settings (reading SSL
  mode gets 403). `tunnels cf` now names the missing permission on a 403.
- **A pocket-id OIDC client** for Cloudflare Access: its redirect URL is
  `https://<team_domain>/cdn-cgi/access/callback`. Whether pocket-id's admin
  API can create the client, or that is one click in its UI, is still to be
  checked.

### Retiring public ssh

With ssh on the tailnet only, the `ssh-*` hostnames and the
`cloudflare-<name>` path in `mesh.conf` go away (runbook
`retire-public-ssh.md`). That leaves two ways into each machine, the tailnet
and the LAN, instead of three. `docs/remote-access.md` exists because of a
night when the one way in was the thing that broke, so the runbook's first
check is that every machine answers on both remaining paths, and it updates
that document to match.

## Decisions

| # | Question | Decision (2026-09-24) |
|---|---|---|
| 1 | Where the `tunnels cf` audit log lives | On the machine that made the change; not replicated |
| 2 | Who may write through `tunnels cf` | Any machine; a true mesh, each writing with the tokens it holds |
| 3 | Should the web UI show the `cf` log | Yes, gathered from every peer into one timeline |
| 4 | ssh behind Access, or tailnet only | ssh on the tailnet only. The web UI goes public through Cloudflare, behind Access with sign-in through id.felixflor.es |
| 5 | Runbook checks run by a tool | Undecided. Keep checks as text an agent reads and judges. Revisit after the first runbooks have been run a few times: if the same check keeps being done mechanically, give it a command |
| 6 | Who may sign in to the web UI | For now, anyone pocket-id knows |
| 7 | The public hostname | `tunnels.felixflor.es` |
| 8 | Can an internet sign-in change anything | ~~No, read-only~~ Changed the same day: yes, for emails listed in `[policy.web] admins`, and across the whole mesh. Everyone else signed in can only look. Destroy, rotate and token changes stay CLI-only |
| 9 | Standby for the web UI | None. It stays on dorkyrobot2, where pocket-id also runs |

## Open questions

1. **The admin email.** `admins` must hold the email on your pocket-id
   account. felixflores@gmail.com is assumed until you say otherwise.
