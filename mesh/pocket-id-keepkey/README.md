# Pocket ID v1 → v2, keeping the signing key

Upstream v2 drops file keys (`data/keys/jwt_private_key.json`) and generates a new one, which signs
everyone out of every app and breaks any app that read the keys at boot (bahay, Everyday Vet's
faces) until it is restarted. `keepkey` puts v1's key into v2's database through v2's own key
provider, the code `pocket-id key-rotate` uses, so the JWKS keeps the same kid and key.

Used on 2026-10-27 for id-dev (the mini) and id.everyday.vet (dr1). id.felixflor.es (dr2) was
upgraded earlier the same day without it, so its key changed.

| file | what |
|---|---|
| `main.go`, `build.sh` | keepkey, built inside Pocket ID's source at the release tag (Go is on the mini): `sh build.sh v2.19.1` |
| `upgrade.sh` | the switch where the plist may be reloaded: stop v1, cold copy of `data/`, v2 migrates on a spare loopback port, keepkey, start; any failure puts v1 back |
| `launcher.sh` | the same switch for a kickstart-only box (doug-mini): it replaces `bin/pocket-id`, the plist stays as it was |
| `rollback-launcher.sh` | back to v1 on a kickstart-only box |

## Before the switch, on the box

1. `./backup.sh` and read its `verified` line. Count users, clients, client groups, passkeys.
2. Download `pocket-id_darwin_arm64` and `checksums.txt` with `gh release download` and check
   the sum. Keep the old binary as `bin/pocket-id-1.16.0`.
3. **Strip quarantine before anything runs**: `xattr -cr` on what arrived by Taildrop, then copy it
   to its final path. A quarantined unsigned binary hangs with no screen to ask on, and macOS
   keeps hanging that path even after `xattr -d` (dr1, 10-27: EV's Pocket ID was down 5 minutes).
   Run each binary once with a time limit before touching the job.
4. `encryption.key`: `(umask 077; openssl rand -base64 32 > encryption.key)`. v2's keys live
   encrypted in the database: back this file up with the data, always.
5. `backup.sh` afterwards: copy `encryption.key` instead of `data/keys`, and check it instead of
   `jwt_private_key.json`.

v2 needs `ENCRYPTION_KEY_FILE` and `ACTORS_HOST=127.0.0.1`: without the latter it listens for its
actors on 0.0.0.0:1414 (UDP). With it, `lsof -nP -iUDP -a -c pocket-id` shows `127.0.0.1:1414`.

## Proving it

The same kid from outside (`/.well-known/jwks.json`), the version in the log, the counts, the
client secrets (`json_extract(credentials, '$.secrets[0].hash')` equals v1's `secret`), and each
app's own `/login` followed to `/interaction` and `/api/oidc/interactions/<id>` naming the client
with step `authenticate`. v2 validates `/authorize` server-side: a hand-made URL without `state`
is refused, so follow the app's real redirect.
