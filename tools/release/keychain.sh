#!/usr/bin/env bash
# release/keychain: puts the release's Developer ID identity and notarization key into a throwaway keychain, from the `release`
# Environment's secrets (docs/RELEASING.md). Used by the release workflow's signing job and by the preflight, so the preflight tests
# exactly what a release does.
#
#   tools/release/keychain.sh <keychain path>     Reads MACOS_CERT_P12, MACOS_CERT_PASSWORD, NOTARY_KEY_P8, NOTARY_KEY_ID and
#                                                 NOTARY_ISSUER_ID from the environment. Prints the identity's name on stdout, and
#                                                 stores the notary profile `recto-notary` in that keychain.
#   tools/release/keychain.sh --remove <path>     Deletes the keychain and every file a secret was written to.
#
# The certificate's password goes in by the environment, never on a command line where `ps` could show it: openssl reads it from
# there. The keychain's own password is made here, for this job alone, and is all that's on a command line.
set -euo pipefail
umask 077
tmp="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
if [ "${1:-}" = "--remove" ]; then
  security delete-keychain "$2" 2>/dev/null || true
  rm -f "$tmp/developer-id.p12" "$tmp/identity.pem" "$tmp/notary.p8"
  exit 0
fi
kc="${1:?usage: tools/release/keychain.sh <keychain path>}"
for s in MACOS_CERT_P12 MACOS_CERT_PASSWORD NOTARY_KEY_P8 NOTARY_KEY_ID NOTARY_ISSUER_ID; do
  [ -n "${!s:-}" ] || { echo "::error::a release is Developer ID signed and notarized; set the secret $s (docs/RELEASING.md)" >&2; exit 1; }
done
root="$(cd "$(dirname "$0")/../.." && pwd)"
team=$(sed -n 's/^pub const TEAM_ID: &str = "\([A-Z0-9]*\)";$/\1/p' "$root/crates/recto-update/src/lib.rs")
kc_pass=$(openssl rand -hex 24)
security create-keychain -p "$kc_pass" "$kc"
security set-keychain-settings -lut 21600 "$kc"
security unlock-keychain -p "$kc_pass" "$kc"
# shellcheck disable=SC2046 # the runner's keychain paths have no spaces
security list-keychains -d user -s "$kc" $(security list-keychains -d user | tr -d '"')
printf '%s' "$MACOS_CERT_P12" | base64 --decode > "$tmp/developer-id.p12"
# What openssl writes is the identity unlocked, for the moment it takes to import.
if ! openssl pkcs12 -in "$tmp/developer-id.p12" -passin env:MACOS_CERT_PASSWORD -nodes -out "$tmp/identity.pem" 2>"$tmp/openssl.err"; then
  rm -f "$tmp/developer-id.p12"
  echo "::error::MACOS_CERT_P12 couldn't be opened with MACOS_CERT_PASSWORD: a wrong password, or a .p12 this openssl can't read (export it again with the older algorithms)" >&2
  sed 's/^/openssl: /' "$tmp/openssl.err" >&2; rm -f "$tmp/openssl.err"
  exit 1
fi
rm -f "$tmp/developer-id.p12" "$tmp/openssl.err"
security import "$tmp/identity.pem" -k "$kc" -t agg -f pemseq -T /usr/bin/codesign -T /usr/bin/security >/dev/null
rm -f "$tmp/identity.pem"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$kc_pass" "$kc" >/dev/null
# Apple's Developer ID intermediate, in case the runner lacks it: the chain must verify.
curl -fsSL -o "$tmp/DeveloperIDG2CA.cer" https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer \
  && security import "$tmp/DeveloperIDG2CA.cer" -k "$kc" >/dev/null 2>&1 || true
# The keychain search list just set, which is what codesign and app/render-helper.sh search too.
identity=$(security find-identity -v -p codesigning | sed -n "s/.*\"\(Developer ID Application: .*($team)\)\"\$/\1/p" | sort -u | head -1)
[ -n "$identity" ] || { echo "::error::MACOS_CERT_P12 holds no valid Developer ID Application identity of team $team" >&2; exit 1; }
printf '%s' "$NOTARY_KEY_P8" | base64 --decode > "$tmp/notary.p8"
xcrun notarytool store-credentials recto-notary --key "$tmp/notary.p8" --key-id "$NOTARY_KEY_ID" \
  --issuer "$NOTARY_ISSUER_ID" --keychain "$kc" >/dev/null
rm -f "$tmp/notary.p8"
echo "$identity"
