# Restore a photo or video from the media backup

**Use when:** a file in the library (`Black5TB/Media`, `Black5TB/Proxies` on
dorkyrobot2) is missing, truncated or won't decode, and you want the last good
copy back.
**Don't use when:** the whole Black5TB is gone (same sources, but restore in
bulk with the `rsync` in step 3 from `WD6TB/Media/`, after asking a person),
or the damage was done *before* the backup ever saw the file: a file that was
already truncated when it arrived has no good copy here.
**Scope:** read-only on the backup. The only write is the restored file on
dorkyrobot2, which you put in a scratch folder first.
**Needs:** ssh to `dorkyrobot1` (WD6TB lives there) and to `dorkyrobot2`.

## How the backup is laid out

The nightly job (03:30 on dorkyrobot2, `com.dorkyrobot.media-backup`) copies
`Media/` and `Proxies/` to `WD6TB` on dorkyrobot1 and **never destroys a
previous copy**:

| where on `WD6TB` (dorkyrobot1) | what it holds |
|---|---|
| `Media/…`, `Proxies/…` | the mirror: last night's primary, **except** files the guard held back |
| `_media-replaced/<YYYY-MM-DD>/Media/…` | what a file looked like *before* that day's run replaced or deleted it. Same relative path as in the library. Kept at least 90 days, then pruned |

The guard: before each run, every file the run would overwrite is checked.
If the primary copy is smaller than the backup copy, or is a JPEG without its
end marker (FFD9) or a PNG without IEND, it is **held**: the backup keeps the
good copy and the run log says so. Videos get the size test only.

```sh
ssh dorkyrobot2 'grep HELD ~/Library/Logs/media-backup.log | tail'
```

A HELD line is either a real problem on the primary (look at it) or a
deliberate smaller edit (release it, step 5).

## 1. Find the good copy

Replace the path with the one you want (relative to the drive, starting
`Media/` or `Proxies/`):

```sh
ssh dorkyrobot1 'P="Media/2019/2019-04/IMG_0001.JPG"
  ls -l "/Volumes/WD6TB/$P" /Volumes/WD6TB/_media-replaced/*/"$P" 2>/dev/null'
```

Every line is a version: the mirror, and one per day it was replaced. Take the
**largest, oldest-good** one. If the mirror copy is the same size as the broken
primary, the damage was already there, go back to an older day folder. If
nothing is listed, the backup never had a good copy: stop.

## 2. Check it is whole before you trust it

```sh
ssh dorkyrobot1 'f="/Volumes/WD6TB/…the path you chose…"; ls -l "$f"; tail -c 2 "$f" | xxd -p'
# a JPEG should print ffd9 (trailing 00 padding is also fine)
# a video: ffprobe "$f" >/dev/null && echo ok      (ffprobe is in immich_server on dorkyrobot2)
```

## 3. Copy it to a scratch folder on dorkyrobot2, not over the original

```sh
ssh dorkyrobot2 'mkdir -p ~/Projects/restore-scratch &&
  rsync -av dorkyrobot1:"/Volumes/WD6TB/…the path you chose…" ~/Projects/restore-scratch/'
```

Check it there (step 2 again, locally). Nothing has changed in the library yet.

## 4. Put it back

Only after you have looked at the scratch copy. Black5TB is written through the
container that already has access to it (launchd jobs can't touch the drive):

```sh
ssh dorkyrobot2 'export PATH=/opt/homebrew/bin:$PATH
  docker run --rm -v /Volumes/Black5TB:/dst -v ~/Projects/restore-scratch:/in:ro \
    media-backup:local cp -p "/in/IMG_0001.JPG" "/dst/Media/2019/2019-04/IMG_0001.JPG"'
```

The next night's run sees a changed file; it is larger than the backup copy,
so the guard lets it through and the old one goes into `_media-replaced/`.
Ask Immich to rescan that file if it had been marked offline.

## 5. A file the guard keeps holding (release it)

If a file was made smaller on purpose (re-compressed, trimmed), the guard will
hold it every night. Add its path (`Media/…`, one per line) to
`~/Services/media-backup/release.txt` on dorkyrobot2. Next run copies it, and
the old version lands in `_media-replaced/`. Remove the line afterwards.

## Stop and get a person when

- the same files are HELD night after night and you don't know why: something
  is damaging the primary;
- a run logs `prune: REFUSED` or `ABORT`;
- you'd have to restore more than a handful of files, or anything onto an
  empty or different drive.

## History

- 2026-09-30: written with the guard and the 90-day retention. The script is
  `~/Services/media-backup/backup.sh` on dorkyrobot2 (not in a repo).
