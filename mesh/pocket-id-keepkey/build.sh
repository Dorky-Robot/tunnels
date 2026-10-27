#!/bin/sh
# Builds keepkey inside Pocket ID's source at the release we upgrade to, so it links the very key
# provider that release boots with. Run where Go is (the mini). Prints the binary's path.
set -eu
TAG=${1:-v2.19.1}
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=${SRC:-$HOME/Projects/scratch/pocket-id-src-$TAG}
[ -d "$SRC" ] || git clone -q --depth 1 --branch "$TAG" https://github.com/pocket-id/pocket-id.git "$SRC"
[ "$(git -C "$SRC" describe --tags)" = "$TAG" ] || { echo "$SRC is not at $TAG" >&2; exit 1; }
mkdir -p "$SRC/backend/cmd/keepkey"
cp "$HERE/main.go" "$SRC/backend/cmd/keepkey/main.go"
(cd "$SRC/backend" && CGO_ENABLED=0 GOOS=darwin GOARCH=arm64 go build -o "$HERE/keepkey-$TAG" ./cmd/keepkey)
echo "$HERE/keepkey-$TAG"
