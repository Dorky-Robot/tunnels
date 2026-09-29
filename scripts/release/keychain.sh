#!/bin/sh
# Put the Developer ID certificate in a keychain of its own for one release
# job, and take it away again afterwards.
#
#   keychain.sh setup     needs MACOS_CERT_P12_BASE64, MACOS_CERT_P12_PASSWORD
#   keychain.sh cleanup
#
# setup prints `KEYCHAIN=<path>` and, on GitHub Actions, also writes it to
# $GITHUB_ENV so sign.sh finds it. The keychain lives under $RUNNER_TEMP (or a
# fresh temp dir) with a random password nobody keeps.
#
# Why a keychain of its own: codesign only signs with identities it can find in
# a keychain, and the runner's login keychain is shared with whatever else runs
# there. A private one, unlocked only for this job and deleted at the end, is
# the smallest thing that works.
set -eu

die() { printf '::error::%s\n' "$*" >&2; exit 1; }

# The Developer ID G2 intermediate. codesign embeds the whole chain, so it has
# to find this one; runner images usually have it, but a missing intermediate
# fails the signing step with "unable to build chain", so bring our own and pin
# it. Expires 2031-09-17.
G2_URL=https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer
G2_SHA256=f16cd3c54c7f83cea4bf1a3e6a0819c8aaa8e4a1528fd144715f350643d2df3a

state_dir() { printf '%s\n' "${RUNNER_TEMP:-${TMPDIR:-/tmp}}"; }

setup() {
    missing=
    [ -n "${MACOS_CERT_P12_BASE64:-}" ] || missing="$missing MACOS_CERT_P12_BASE64"
    [ -n "${MACOS_CERT_P12_PASSWORD:-}" ] || missing="$missing MACOS_CERT_P12_PASSWORD"
    # A tag release never ships unsigned again: an unsigned binary gets a new
    # firewall identity at every brew upgrade and the agent drops off the mesh.
    [ -z "$missing" ] || die "signing secrets not set:$missing (see README, Releases)"

    dir=$(mktemp -d "$(state_dir)/tunnels-signing.XXXXXX")
    kc="$dir/signing.keychain-db"
    kc_pw=$(openssl rand -hex 24)
    p12="$dir/cert.p12"
    ( umask 077; printf '%s' "$MACOS_CERT_P12_BASE64" | base64 --decode > "$p12" ) \
        || die "MACOS_CERT_P12_BASE64 is not base64"

    security create-keychain -p "$kc_pw" "$kc"
    # No auto-lock during the job (6 h), and unlock it now.
    security set-keychain-settings -lut 21600 "$kc"
    security unlock-keychain -p "$kc_pw" "$kc"
    security import "$p12" -k "$kc" -f pkcs12 -P "$MACOS_CERT_P12_PASSWORD" -T /usr/bin/codesign \
        || die "could not import the p12: wrong MACOS_CERT_P12_PASSWORD, or a p12 macOS can't read"
    rm -f "$p12"
    # Let codesign use the key without a GUI prompt nobody is there to answer.
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$kc_pw" "$kc" >/dev/null

    g2="$dir/DeveloperIDG2CA.cer"
    if curl -fsSL --retry 3 "$G2_URL" -o "$g2" \
        && [ "$(shasum -a 256 "$g2" | cut -d' ' -f1)" = "$G2_SHA256" ]; then
        security import "$g2" -k "$kc" >/dev/null 2>&1 || true
    else
        echo "warning: could not fetch the Developer ID G2 intermediate; relying on the system's copy" >&2
    fi

    # Remember the search list so cleanup can put it back, then add ours in
    # front: codesign builds the certificate chain from the search list.
    security list-keychains -d user | sed 's/^[[:space:]]*"//; s/"$//' > "$dir/search-list"
    # shellcheck disable=SC2046
    security list-keychains -d user -s "$kc" $(cat "$dir/search-list")

    echo "$dir" > "$(state_dir)/tunnels-signing.dir"
    echo "KEYCHAIN=$kc"
    [ -z "${GITHUB_ENV:-}" ] || echo "KEYCHAIN=$kc" >> "$GITHUB_ENV"
}

cleanup() {
    marker="$(state_dir)/tunnels-signing.dir"
    [ -f "$marker" ] || { echo "nothing to clean up"; return 0; }
    dir=$(cat "$marker")
    if [ -f "$dir/search-list" ]; then
        # shellcheck disable=SC2046
        security list-keychains -d user -s $(cat "$dir/search-list") || true
    fi
    security delete-keychain "$dir/signing.keychain-db" 2>/dev/null || true
    rm -rf "$dir" "$marker"
    echo "signing keychain removed"
}

case "${1:-}" in
    setup) setup ;;
    cleanup) cleanup ;;
    *) echo "usage: $0 setup|cleanup" >&2; exit 2 ;;
esac
