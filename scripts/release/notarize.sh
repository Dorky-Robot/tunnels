#!/bin/sh
# Send a signed tunnels binary to Apple's notary service and wait for the
# verdict. Fails unless it is Accepted, and prints Apple's log either way.
#
#   notarize.sh <binary>
#   needs APPLE_API_KEY_P8_BASE64, APPLE_API_KEY_ID, APPLE_API_ISSUER_ID
#   (an App Store Connect API key; see README, Releases)
#
# Why notarize at all: the firewall's "automatically allow downloaded signed
# software" and Gatekeeper both want a Developer ID binary that Apple has
# scanned. A bare Mach-O can't carry a stapled ticket (stapler only takes
# apps, pkgs and dmgs), so the ticket stays with Apple and macOS looks it up
# online the first time it assesses the binary. That is expected, not a gap.
set -eu

die() { printf '::error::%s\n' "$*" >&2; exit 1; }

bin=${1:?usage: notarize.sh <binary>}
[ -f "$bin" ] || die "$bin: no such file"
missing=
[ -n "${APPLE_API_KEY_P8_BASE64:-}" ] || missing="$missing APPLE_API_KEY_P8_BASE64"
[ -n "${APPLE_API_KEY_ID:-}" ] || missing="$missing APPLE_API_KEY_ID"
[ -n "${APPLE_API_ISSUER_ID:-}" ] || missing="$missing APPLE_API_ISSUER_ID"
[ -z "$missing" ] || die "notarization secrets not set:$missing (see README, Releases)"

dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/tunnels-notary.XXXXXX")
trap 'rm -rf "$dir"' EXIT
key="$dir/AuthKey_${APPLE_API_KEY_ID}.p8"
( umask 077; printf '%s' "$APPLE_API_KEY_P8_BASE64" | base64 --decode > "$key" ) \
    || die "APPLE_API_KEY_P8_BASE64 is not base64"
grep -q 'BEGIN PRIVATE KEY' "$key" || die "APPLE_API_KEY_P8_BASE64 does not decode to a .p8 private key"

# notarytool takes zip, pkg or dmg; a zip of the one binary is the smallest.
zip="$dir/$(basename "$bin").zip"
ditto -c -k --keepParent "$bin" "$zip"

auth="--key $key --key-id $APPLE_API_KEY_ID --issuer $APPLE_API_ISSUER_ID"
echo "submitting $bin to the notary service"
# shellcheck disable=SC2086
if ! xcrun notarytool submit "$zip" $auth --wait --timeout 30m \
        --output-format json > "$dir/submit.json" 2> "$dir/submit.err"; then
    cat "$dir/submit.err" >&2
    # A rejected submission also exits non-zero; fall through to fetch its log.
fi
cat "$dir/submit.json"; echo

id=$(plutil -extract id raw -o - "$dir/submit.json" 2>/dev/null || true)
status=$(plutil -extract status raw -o - "$dir/submit.json" 2>/dev/null || true)
[ -n "$id" ] || die "notarytool gave no submission id (bad API key, issuer or key id?)"

echo "--- notarytool log $id"
# shellcheck disable=SC2086
xcrun notarytool log "$id" $auth || echo "(could not fetch the log)"
echo "---"

[ "$status" = Accepted ] || die "notarization of $bin: ${status:-no status} (submission $id)"
echo "notarized: $bin (submission $id)"
