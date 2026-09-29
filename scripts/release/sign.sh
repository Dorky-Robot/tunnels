#!/bin/sh
# Sign a tunnels binary with the Developer ID identity in $KEYCHAIN, then check
# the result is what the macOS firewall needs to keep trusting it.
#
#   KEYCHAIN=<path> sign.sh <binary> [expected-arch]
#
# Why: the application firewall keys an unsigned (or ad-hoc signed) binary by
# its path, and every `brew upgrade` installs to a new Cellar path, so the old
# allow rule stops matching and tailnet connections to the agent on :7630 are
# dropped. A Developer ID signature gives the binary the same identity in every
# version, and "automatically allow downloaded signed software" lets it through
# on a machine that has never seen it.
#
# The identity is looked up in the keychain by its name, never hardcoded, so a
# renewed certificate needs only new secrets.
#
# Knobs for rehearsing with a throwaway self-signed identity (never set in CI):
#   SIGN_IDENTITY_MATCH   default "Developer ID Application:"
#   SIGN_ALLOW_UNTRUSTED  1 to accept an identity the system doesn't trust
set -eu

die() { printf '::error::%s\n' "$*" >&2; exit 1; }

bin=${1:?usage: sign.sh <binary> [expected-arch]}
want_arch=${2:-}
[ -n "${KEYCHAIN:-}" ] || die "KEYCHAIN is not set; run keychain.sh setup first"
[ -f "$bin" ] || die "$bin: no such file"

# The identifier is part of the signature's designated requirement. Keep it the
# same forever, so even a firewall rule written by hand for one version keeps
# matching the next.
IDENTIFIER=com.dorkyrobot.tunnels
match=${SIGN_IDENTITY_MATCH:-Developer ID Application:}

valid=-v
[ "${SIGN_ALLOW_UNTRUSTED:-0}" = 1 ] && valid=
# Lines look like:   1) 0123ABCD... "Developer ID Application: Name (TEAMID)"
# shellcheck disable=SC2086
ids=$(security find-identity $valid -p codesigning "$KEYCHAIN" | grep -F "\"$match" || true)
count=$(printf '%s' "$ids" | grep -c . || true)
[ "$count" -ge 1 ] || die "no valid \"$match\" identity in $KEYCHAIN (expired, revoked, or the p12 holds a different kind of certificate)"
[ "$count" -eq 1 ] || die "$count \"$match\" identities in $KEYCHAIN; the p12 should hold exactly one"
hash=$(printf '%s\n' "$ids" | awk '{print $2}')
name=$(printf '%s\n' "$ids" | sed 's/^[^"]*"\([^"]*\)".*/\1/')
echo "signing $bin as \"$name\" ($hash)"

# Homebrew re-signs a binary ad-hoc whenever it rewrites its load commands,
# which would throw our signature away on install. It only does that for
# rpaths and non-absolute or keg-relative dylib paths, so refuse to ship a
# binary that has any.
if otool -l "$bin" | grep -q LC_RPATH; then
    die "$bin has an LC_RPATH; brew would rewrite it and re-sign ad-hoc"
fi
bad_libs=$(otool -L "$bin" | tail -n +2 | awk '{print $1}' | grep -v -E '^/(usr/lib|System/Library)/' || true)
[ -z "$bad_libs" ] || die "$bin links outside the system ($bad_libs); brew would relocate and re-sign it ad-hoc"

if [ -n "$want_arch" ]; then
    arch=$(lipo -archs "$bin")
    [ "$arch" = "$want_arch" ] || die "$bin is $arch, expected $want_arch"
fi

# --options runtime (hardened runtime) and --timestamp are what notarization
# requires. --force replaces the linker's ad-hoc signature.
codesign --force --options runtime --timestamp \
    --identifier "$IDENTIFIER" \
    --keychain "$KEYCHAIN" --sign "$hash" "$bin"

echo "--- codesign -dv --verbose=4"
details=$(codesign -dv --verbose=4 "$bin" 2>&1)
printf '%s\n' "$details"
echo "---"
codesign --verify --strict --verbose=2 "$bin"

printf '%s\n' "$details" | grep -q '^Signature=adhoc' && die "$bin is still ad-hoc signed"
printf '%s\n' "$details" | grep -qF "Authority=$name" || die "$bin is not signed by \"$name\""
printf '%s\n' "$details" | grep -qF "Identifier=$IDENTIFIER" || die "$bin identifier is not $IDENTIFIER"
printf '%s\n' "$details" | grep -q '^CodeDirectory .*(runtime)' || die "$bin lacks the hardened runtime"
printf '%s\n' "$details" | grep -q '^Timestamp=' || die "$bin has no secure timestamp"
if [ "${SIGN_ALLOW_UNTRUSTED:-0}" != 1 ]; then
    printf '%s\n' "$details" | grep -q '^TeamIdentifier=[A-Z0-9]\{10\}$' || die "$bin has no team identifier"
fi
echo "signed: $bin"
