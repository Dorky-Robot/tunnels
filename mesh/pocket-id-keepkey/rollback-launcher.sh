#!/bin/sh
# Back to v1 on a kickstart-only box: copy this over bin/pocket-id, then
#   launchctl kickstart -k gui/$(id -u)/<label>
# Its first start (v2 already stopped) sets v2's data aside and puts the cold copy launcher.sh took
# back, so v1 returns with its own key. Anything changed since the upgrade (a passkey, a client)
# stays only in the set-aside data, so decide within the hour.
D=$(cd "$(dirname "$0")/.." && pwd)
export PATH=/usr/bin:/bin:/usr/sbin:/sbin
cd "$D" || exit 1
if [ ! -e v1-restored ]; then
  COLD=$(ls -1d data.prev-v1-* | tail -1)
  STAMP=$(date +%Y%m%d-%H%M%S)
  [ -d "$COLD" ] || { echo "no data.prev-v1-* to roll back to" >&2; exit 1; }
  rm -rf data.restoring && cp -Rp "$COLD" data.restoring &&
    mv data "data.v2-rolled-back-$STAMP" && mv data.restoring data && touch v1-restored
  rm -f v2-key-kept
  echo "[$(date '+%F %T')] rolled back to v1 on $COLD; v2's data is data.v2-rolled-back-$STAMP" | tee -a cutover.log
fi
exec "$D/bin/pocket-id-1.16.0"
