## Moving files between machines — Taildrop

Send files to another mesh machine with Taildrop, not scp or rsync:

    tailscale file cp <file…> <host>:      host is the tailnet name: dorkyrobot1,
                                           felixs-mac-mini, dougs-mac-mini-2024 …
    tailscale file cp --targets            who can receive

`tailscale file cp` can only reach our own tailnet, so nothing sent that way
leaves it, and it is the one transfer agents are allowed without asking. On
the receiving box the file shows up in `~/Projects/inbox` a few seconds
later: `com.dorkyrobot.taildrop-inbox` collects it, and a same-named file
gets a number instead of overwriting. Taildrop sends files, not directories —
`tar czf x.tgz dir` first and unpack from the inbox. It only pushes, so the
box that has the files sends them. Over ssh the full path is
`/opt/homebrew/bin/tailscale` (`/usr/local/bin` on mac2019).

To move rather than copy, check that the file arrived (size or `shasum` on
both ends) before deleting the source. A file landing in an inbox on
dorkyrobot1 is not a deploy: production changes still go through
`sh ship.sh`. Setup lives in `tunnels/mesh/` (`install.sh`, `claude-global.md`,
`com.dorkyrobot.taildrop-inbox.plist`).
