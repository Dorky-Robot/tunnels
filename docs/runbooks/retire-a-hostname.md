# Retire a public hostname

**Use when:** a hostname like `william.felixflor.es` should stop existing:
no DNS, no ingress, gone from the fleet.
**Don't use when:** the hostname should move to another box or tunnel (that
is `tunnels route add … --yes` on the new tunnel) or be renamed (`tunnels
route mv old new`, which brings the new name up before it takes the old one
down).
**Scope:** [fleet file + cloudflare]. The app behind it keeps running.
**Needs:** someone with the say-so to retire it (a deletion needs Felix's
own OK, not a relayed one); a box on `policy.remote_from` with an API token
for the hostname's account (dorkyrobot2 has both).

## What `route rm` deletes, and what it leaves

| | |
|---|---|
| **deletes** | the `[[routes]]` entry in the fleet file (the serial goes up by one); the ingress rule for that hostname on every fleet tunnel carrying it; the DNS record for it **only if** it is a CNAME to a fleet tunnel (`<id>.cfargotunnel.com`) or to a tunnel that no longer exists |
| **leaves** | any other record with that name (A, TXT, MX, a CNAME to somewhere else); the app and its port; the tunnel; other hostnames on the same backend |

Before 2026-09 `route rm` deleted the DNS record whatever it pointed at
(kapwa 50e2b). It doesn't now.

## Before you start

On dorkyrobot2 (or another box on `remote_from`), name it once:

```sh
H=<hostname>            # e.g. william.felixflor.es
Z=<its zone>            # the Cloudflare zone: felixflor.es, everyday.vet, …
```

```sh
tunnels plan; echo "exit $?"          # 0: nothing else is drifting, so the plan below is only yours
tunnels route list | grep -E "^$H "     # its tunnel, service, and DNS state
```

Write down the before state, so the Undo works:

```sh
tunnels fleet show | grep -B1 -A4 "\"$H\""     # the route block: tunnel, service, standby
tunnels cf get "/zones/{zone:$Z}/dns_records?name=$H" | grep -E '"(type|content)"'
curl -s -o /dev/null -w '%{http_code}\n' https://$H/
tunnels fleet history | head -1                  # serial N
```

Then check nothing else needs it: another route to the same `service` port
(`tunnels route list | grep localhost:<port>`) shares the backend, so leave
the app alone; a line in `mesh/watch.conf` naming it goes in the same
change.

## Steps

1. **Remove it.**

   ```sh
   tunnels route rm $H
   ```

   **Check:** it announces `removing <host>` and the apply report lists the
   ingress removal and the DNS delete. If this box has no token for the
   account it says the owning agent carries it out; wait one agent pass
   (120 s) and go on.

2. **Prove it is gone.**

   ```sh
   tunnels fleet history | head -1          # serial N+1, by this box
   tunnels route list | grep -c "^$H "   # 0
   tunnels cf get "/zones/{zone:$Z}/dns_records?name=$H"   # [] (or only non-tunnel records)
   tunnels plan; echo "exit $?"              # 0
   dig +short $H                         # nothing, once caches expire
   ```

   A 530 from `curl` straight afterwards is Cloudflare's edge before the
   DNS change reaches it; `dig` coming back empty is the proof.

3. **Nothing else moved.** Rerun the index's host loop for the box that
   served it: every other hostname answers as before.

## The "not in the fleet file" case

`tunnels route rm <host>` stops with `<host> is not in the fleet file` and
changes nothing. That means someone added it by hand in the dashboard, or
the fleet already dropped it and the Cloudflare side was left behind.
`scripts/mesh-watch.py` reports this as `monitor:drift`.

1. `tunnels plan` shows what is left, marked `(not in the fleet file)`:
   `<host> → … off <tunnel>` (ingress) and `DNS <host> → <tunnel>`. These
   are prune actions, so neither the agents nor a plain `apply` touch them.
2. Remove only those, only for this hostname:

   ```sh
   tunnels apply --prune --host $H
   ```

3. If `plan` shows nothing for it, it isn't a tunnel route at all (an A
   record, or a CNAME to another service). `tunnels` doesn't own it; delete
   it with `tunnels cf`, which previews first and logs an undo:

   ```sh
   tunnels cf delete "/zones/{zone:$Z}/dns_records/{record:$H}"          # preview
   tunnels cf delete "/zones/{zone:$Z}/dns_records/{record:$H}" --yes    # send
   ```

## Stop and ask a person if

- `tunnels plan` before you start is not 0 and the drift is not yours;
- the DNS record points at a tunnel outside the fleet (an orphan such as
  `mac-sara`): `plan` leaves it and `tunnels cf delete` refuses it (tunnel
  CNAMEs belong to routes). The way out is deciding that tunnel: `tunnel
  adopt` it and then `route rm`, or `tunnel destroy`, which deletes its DNS
  too;
- the hostname is production (everyday.vet, homesforsalebymonica.com) and
  the go-ahead didn't come from Felix directly.

## Undo

```sh
tunnels route add $H <service> --tunnel <alias>   # the service and tunnel you wrote down
```

It puts back ingress and DNS and adds the route to the fleet. A record you
deleted with `tunnels cf delete` comes back with `tunnels cf undo <id>
--yes` (`tunnels cf log` lists the ids).

## History

- 2026-09-29: `bridge-mini.felixflor.es` retired with `route rm`; fleet
  serial 49; its tunnel CNAME deleted.
- 2026-09-30: `william.felixflor.es` retired (serial 50). It shared its
  backend with `jerry.felixflor.es`, which stayed: the app kept running.
  Right afterwards curl gave 530 while `dig` was already empty.
- 2026-09-30: written. The reads above run against the live fleet with
  `H=jerry.felixflor.es`; the `cf delete` preview on it answered "refused:
  DNS records that point at a tunnel belong to its route". `route rm` itself
  was not run (it is a write); its refusal message is from `src/main.rs`.
