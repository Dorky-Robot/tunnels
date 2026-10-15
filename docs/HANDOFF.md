# Mesh handoff (2026-10-14)

Written by the Mesh lead (session e186c935 on dorkyrobot2) when the lead layer was wound
down (CEO d-126). It assumes you know nothing about the mesh. Check every claim against
the machine before acting on it: the code and the boxes win over this file.

## What "Mesh" is
- The machines and what keeps them up:
  - Cloudflare tunnels: this repo, with the `tunnels` CLI, agent, fleet file, watchdog and the `cdp` CD pipeline;
  - Tailscale, Taildrop and the ssh paths into every box;
  - backups and storage: Black5TB, WD6TB, Yottamaster, Orange4TB, S3;
  - Immich, Pocket ID, and launchd jobs on every box.
- Every machine has three ways in: `<name>`, `<name>-lan` and `cloudflare-<name>` (see docs/remote-access.md).

| Machine | Role |
|---|---|
| dorkyrobot2 (dr2) | Black5TB = primary photo library; Immich; Pocket ID id.felixflor.es; cdp; CEO session |
| dorkyrobot1 (dr1) | **Everyday Vet production**; WD6TB (nightly backup of Black5TB) + Yottamaster (RAID1) |
| macmini (Felix's mini) | Orange4TB (old Nextcloud files); pocket-id-dev; diwa hub. Holds the AWS profile `meiggs-flores-s3`, which carries ROOT keys: use it in place, never copy |
| mac2024 | Felix's laptop |
| mac2019 | Dogtopia (prod) + staging; reaches the internet only through dr2's Thunderbolt-bridge NAT |
| doug-mini | Monica's production: ~/Prod only via ship scripts; never touch tunnel/tailscale/sshd |

## Where things live
- This repo, `~/Projects/dorky_robot/tunnels` (GitHub Dorky-Robot/tunnels, diwa `Dorky-Robot/tunnels`).
  - Work in a worktree and fast-forward main.
  - Felix has OK'd pushing finished tunnels work.
- `~/Projects/mesh-lead/` (not git) holds the working files:
  - epics/: step-level detail;
  - briefs/: past worker briefs;
  - cd/: policy v2, design, EV patches 0001–0008;
  - other-stores/, takeout-leftovers/, photo-repairs/: reports with tsv proofs;
  - approved-deletions/: scripts and logs of the 10-14 deletes;
  - safety-install/: Felix's sudo scripts and verify.sh.
- Backlog: `~/Projects/head-of-product/ready/mesh.md` (the board and dispatcher read it).
- History: `~/.claude/projects/-Users-dorkyrobot2-Projects/memory/mesh-lead-ledger.md`.
  - It is long, and its "Waiting on Felix" section is STALE (mostly answered or moot). Use the list below instead.
- Nightly photo backup: dr2 `~/Services/media-backup/backup.sh` (not git; tests in tests/guard-test.sh).
  - launchd job com.dorkyrobot.media-backup at 03:30; log ~/Library/Logs/media-backup.log.
  - Copies Black5TB/{Media,Proxies,Docs} and Immich (backups, upload, profile, library) to dr1 WD6TB.
  - Anything replaced or removed goes to WD6TB/_media-replaced/<day>/ for 90 days.
- Immich: dr2 `~/Services/immich` (:2283, media.felixflor.es) and `~/Services/immich-vet` (:2284, vet.felixflor.es, empty).
  - Both run v3.3.1 in colima (6 CPU / 6 GiB), started by com.dorkyrobot.colima, which retries on failure.
  - Guard jobs: com.dorkyrobot.immich-library-guard and immich-offline-repair.

## Live (verified 10-14)
- **Immich:** restored 10-14 from the 10-10 dumps, then upgraded to v3.3.1.
  - It had to be restored because an agent ran `rm -rf` on dr2's ~/.colima on 10-12.
  - 111,604 live assets, plus 1,367 trashed (the offline baseline; no purge before 2027-09-15).
  - Library scan and watch are OFF and trash keeps 365 days. Keep it that way.
- **media-backup guard, fixed 10-14:** pushes itemize as `<f`, not `>f`, so until then it had checked nothing.
  - The Immich pass is ON (flag ~/Services/media-backup/immich-on); its first real run is 10-15 03:30.
  - The old WD6TB-only dumps are archived for good in WD6TB/Backups/<inst>/archive-pre-1014.
- **cdp:** installed on dr2 (com.dorkyrobot.cdp) with the kill switch ON (`~/.config/cdp/STOP`) and no products.
  - Since tunnels fa42407 it follows EV's `ship.sh --undo {sha}` contract (exit 0/1/2).
- **Tunnel keys** are in private files (`--token-file`) on dr2, dr1, the mini and doug-mini. doug-mini's was rotated 10-13.
- **Storage cleanup (epic-every-copy):** landed; about 555 GB back. Proofs and logs are in mesh-lead/{other-stores,takeout-leftovers,approved-deletions}.
  - Kept on purpose: every Pixelmator .pxd and metadata.info (S3 and disk), the non-photo Takeout zips, and one 3.1 MB old Nextcloud export.

## In flight / dangling
- **Workers and holds:** no Mesh workers are running. The only hold is the cdp kill switch, which stays ON until Felix says otherwise.
- **Safety install:** Felix's three sudo lines (mesh-lead/safety-install/FELIX.md).
  - `verify.sh all` at 10-14 ~14:00 still showed 7 FAIL (for example, no heartbeat on the mini), so it is not done or only partly done.
  - Run `sh ~/Projects/mesh-lead/safety-install/verify.sh all` before assuming either way.
- **EV's side of CD:** EV patches 0001–0008 are ready, along with a fix for ship.sh refusing a refs/cd candidate ("not on main").
  - EV's lead held both until Felix types "build EV's CD side". With the leads gone, CEO decides where that is typed.
- **mesh/leads.conf:** two temporary excludes remain ("realuxe demo", "weekly changes addendum"). Remove them, or retire them along with lead-keeper.

## Open Felix asks (only he can answer)
A yes for prod, deletions, DNS, money or sudo must be typed in the session that runs the step. Worker permission classifiers refuse a relayed yes, and that is correct.
1. "Delete the Nextcloud client 9703ab13… in id.felixflor.es → Admin → OIDC Clients." Done by hand in a browser; there is no API key for it.
2. "Run the three safety-install lines (FELIX.md), mini first." On his own terminal; needs sudo passwords.
3. "Where should development and Claude sessions live?" Our recommendation: builds and tests on mac2024. Numbers are in mesh-lead/epics/room-on-machines.md. Any session can ask.
4. "Build EV's CD side" (applies patches 0001–0008), and later `cdp init` / `cdp go ev`. In the session that will run them.
5. "Delete the two dead doug-mini token copies (~/.cloudflared/doug-mini.token, com.cloudflare.cloudflared-doug-mini.plist.orig-20260920)?" In the running session, or at the doug-mini visit.
6. Keep/kill list (mesh-lead/keep-kill.md): mac2019 Wi-Fi fallback, mini leftovers, old kita on mac2024. Not approved.
7. "Move Dogtopia off mac2019 and retire the machine?" This is EV-owned prod.

## Traps learned (each one cost us)
- **TCC hangs:** opening Black5TB from a Claude session on dr2 can hang open() forever.
  - Always use `ssh dorkyrobot2 '<cmd>'`. `ssh localhost` fails on the host key.
- **Never cut the ssh path you stand on.**
  - Use `launchctl kickstart -k`, never bootout+bootstrap over ssh.
  - After `tunnels tunnel restart` (0.25.0), check within seconds that the job is loaded. On 10-13 dr1's EV tunnel stayed unloaded for ~40 s.
- **Auto-approving agents delete things.**
  - Hermes "mesh-lead" (mac2019) ran `rm -rf ~/.ollama ~/.colima` on dr2 at 10-12 17:12, on "see what we can do". It took Immich's Postgres with it. Hermes was removed 10-14.
  - To find who did something, search every Hermes profile DB (~/.hermes/profiles/*/state.db).
- **Immich restore:**
  - The dumps are single-database pg_dumps, so restore them with `--dbname=immich`.
  - Before immich_server starts, confirm library scan/watch are off and nothing purges offline assets.
- **media-backup and dr1's rrsync:**
  - It anchors `/` at the WD6TB root and refuses `..`.
  - `--backup-dir` must exist first (`--mkpath`).
  - Test through tests/rrsync-dr1, never a local destination.
- **S3:** sara-photo-shoots is versioned.
  - Delete by VersionId (every version and delete marker of the exact keys), or the space doesn't come back.
  - Name+size is not proof; use ETag or md5.
- **Background sessions:** lead-keeper revives a `claude stop`'d session, so use `claude rm <id>` for a finished worker. Workers freeze silently on TCC: check each one within 30 minutes. runlog job classes have no "ops" or "code"; use feature or other.
- **The global CLAUDE.md is not ours any more.** Since 10-15 every box's `~/.claude/CLAUDE.md` imports ecosystem-log's `claude/global.md`, and launchd `com.dorkyrobot.ecosystem-log-pull` keeps it current (ecosystem-log `claude/README.md`).
  - install.sh's old step 6 (render `mesh/claude-global.md` + `claude-coordinator.md` between markers in that file) lives only in mac2024's 8 unpushed commits. When they are reconciled, drop step 6 and those two files; their text is already in the master. Run as-is, it would append the old block after the import.
  - `claude -p` over plain ssh can't reach the login keychain on mac2024, the mini and dr1 ("Not logged in" / "OAuth session expired"). Run it as a one-shot `launchctl submit` job instead.
- **PATH:** non-login ssh shells lack /opt/homebrew/bin (mac2019 uses /usr/local/bin). "Command not found" usually means PATH.

## Next READY items, in order
1. Check the 10-15 03:30 media-backup log: guard counts, `immich=0`, and about 35 old dumps set aside under _media-replaced/2026-10-15/Backups/.
2. Safety install: once Felix has run his three lines, run `verify.sh <box>` per box. Then do the proof test: break and restore a throwaway hostname, and check that one ntfy and one email arrive.
3. EV S3 bucket to Glacier (Felix approved). First find out why ~62 GB sits in STANDARD. Use the mini's root AWS profile in place.
4. Pin Claude Code's version on the mini, mac2024, dr1, mac2019 and doug-mini, as on dr2: DISABLE_AUTOUPDATER=1 plus a scheduled update.
5. Add the Immich history location to docs/runbooks (restore from media-backup). Run the Sara decode pass on 13,749 files at night, read-only.
6. Updates reach every machine safely (signed releases, shared Claude setup). Waits on Felix's Apple signing secrets.
