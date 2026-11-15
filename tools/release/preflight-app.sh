#!/usr/bin/env bash
# release/preflight-app: the real app through the release's build and team signing, without a tag, by the release's own code
# (.github/workflows/release-preflight.yml; docs/RELEASING.md). Until 2026-11-15 the app's team signing with its provisioning profile first ran
# when a v* tag was pushed, and a failure there withdrew the tag and cost a cycle. Each mode is a step of the preflight, in this order:
#
#   tools/release/preflight-app.sh --build   No key here, checked first. The release's build job as it is: `app/release-zip.sh --built-only`
#                                            (the full-LTO build, its checks and a quiet run drawing a PDF), then the app packed and unpacked
#                                            with ditto as release.yml's build and signing jobs hand it over.
#   tools/release/preflight-app.sh --sign    The keychain loaded, RECTO_SIGN_IDENTITY, RECTO_NOTARY_PROFILE and RECTO_PROVISION_PROFILE set as
#                                            the signing job sets them. The signing job's own command up to notarization,
#                                            `app/release-zip.sh --signed --already-built --no-notarize`: build.sh --sign-only (the profile
#                                            embedded, the main executable alone claiming the entitlements, the helper and the entitlements
#                                            checked) and the team checked on the app and its helper. Runs nothing of the app.
#   tools/release/preflight-app.sh --check   No key here, checked first, as the keychain and the profile file are gone. The signature verified
#                                            (--deep --strict, the team, the hardened runtime, a secure timestamp), the entitlements and the
#                                            helper checked, one launch that answers `--build-kind` (a profile that doesn't allow what the app
#                                            claims has the system stop it at launch), and the signing job's own after-keys checks,
#                                            `app/release-zip.sh --check-signed --no-notarize`: its quiet run drawing a PDF with walled workers.
#
# On CI only ($GITHUB_ACTIONS): nothing of the signing path runs on a person's Mac, and --check refuses where a signing identity is found.
# Each step is under a limit and names itself if it waits (tools/release/limited.sh). Prints no part of the profile or the certificate.
#
# What it doesn't try, as only a release does: notarization (the preflight asks Apple only whether the key is accepted), the stapled ticket,
# Gatekeeper on the stapled app, the zip, and the updater's own reader of it. Nor the Find key in the data-protection keychain under the app's
# real service name: a quiet run is always on a temp state, whose key is a file, and the app's own code for the keychain is run by
# tools/release/dp-keychain-probe.sh (its R build) under a probe's service name.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
# shellcheck source=tools/release/limited.sh
. "$root/tools/release/limited.sh"
APP="$root/app/build.noindex/RectoPDF.app"
mode="${1:-}"
case "$mode" in
  --build | --sign | --check) ;;
  *)
    sed -n '6,19p' "$0" >&2
    exit 2
    ;;
esac
[ -n "${GITHUB_ACTIONS:-}" ] || { echo "preflight-app: the release preflight's, on its runner only (GITHUB_ACTIONS is unset): nothing of the signing path runs on a person's Mac" >&2; exit 2; }
team=$(sed -n 's/^pub const TEAM_ID: &str = "\([A-Z0-9]*\)";$/\1/p' "$root/crates/recto-update/src/lib.rs")
[ -n "$team" ] || { echo "::error::the team's ID couldn't be read from crates/recto-update/src/lib.rs" >&2; exit 1; }

# The rule (release.yml's): nothing of the product is compiled or run while a signing key or the notarization key is loaded. So the steps
# that build and launch say first that there's none: no secret in the environment, and no signing identity in any keychain searched.
no_keys_here() {
  local s found
  for s in MACOS_CERT_P12 MACOS_CERT_PASSWORD NOTARY_KEY_P8 NOTARY_KEY_ID NOTARY_ISSUER_ID MACOS_PROVISION_PROFILE RECTO_SIGN_IDENTITY; do
    [ -z "${!s:-}" ] || { echo "::error::$s is set in this step, so nothing of the app may be built or run here" >&2; exit 1; }
  done
  found="$(perl -e 'alarm shift; exec @ARGV' 10 security find-identity -v -p codesigning 2>/dev/null | tail -1 || true)"
  case "$found" in
    *"0 valid identities found"*) echo "  ok  no signing identity here: $found" ;;
    *) echo "::error::a signing identity is here ($found), so nothing of the app may be built or run" >&2; exit 1 ;;
  esac
}

case "$mode" in
  --build)
    echo "The release's build job, with no key here"
    no_keys_here
    limited 1800 "the release's build job (app/release-zip.sh --built-only)" env MIN_FREE=0 "$root/app/release-zip.sh" --built-only
    # As release.yml's build job packs it and its signing job unpacks it, so what's signed here is what the release would get.
    zip="${RUNNER_TEMP:?}/preflight-built-app.zip"
    ditto -c -k --keepParent "$APP" "$zip"
    rm -rf "$APP"
    ditto -x -k "$zip" "$(dirname "$APP")"
    rm -f "$zip"
    [ -x "$APP/Contents/MacOS/recto-updater" ] || { echo "::error::the app didn't come back from ditto whole" >&2; exit 1; }
    echo "  ok  packed and unpacked with ditto, as the release hands it to its signing job"
    ;;
  --sign)
    echo "The real app signed by the team, as the release's signing job signs it, and stopped before notarization"
    : "${RECTO_SIGN_IDENTITY:?--sign needs RECTO_SIGN_IDENTITY, from tools/release/keychain.sh}"
    [ -f "${RECTO_PROVISION_PROFILE:-}" ] || { echo "::error::--sign needs RECTO_PROVISION_PROFILE, the decoded profile" >&2; exit 1; }
    limited 900 "the team signing the real app (app/release-zip.sh --signed --already-built --no-notarize)" \
      "$root/app/release-zip.sh" --signed --already-built --no-notarize
    ;;
  --check)
    echo "The signed app, checked and launched with no key here"
    no_keys_here
    [ ! -e "${RUNNER_TEMP:?}/recto.provisionprofile" ] || { echo "::error::the decoded profile is still at $RUNNER_TEMP/recto.provisionprofile" >&2; exit 1; }
    limited 120 "the signature verifying, deep and strict" codesign --verify --deep --strict --verbose=2 "$APP"
    facts="$(codesign -dv --verbose=2 "$APP" 2>&1)"
    grep -E '^(Authority=Developer ID Application: |TeamIdentifier=|Timestamp=)' <<< "$facts" | sed 's/^/  /' || true
    grep -qx "TeamIdentifier=$team" <<< "$facts" || { echo "::error::the app isn't signed by team $team" >&2; exit 1; }
    grep -q '^Timestamp=' <<< "$facts" || { echo "::error::the app's signature has no secure timestamp, which notarization requires" >&2; exit 1; }
    grep -Eq '^CodeDirectory .*flags=.*runtime' <<< "$facts" || { echo "::error::the app isn't signed with the hardened runtime, which notarization requires" >&2; exit 1; }
    echo "  ok  team $team, hardened runtime, secure timestamp"
    limited 60 "the entitlements: the main executable alone claims them, its profile embedded" "$root/tools/release/app-entitlements.sh" --check "$APP"
    limited 60 "the renderer helper, sandboxed and signed by the team" "$root/app/render-helper.sh" --check "$APP"
    # One launch of the signed main executable: the system checks its entitlements against the embedded profile before it runs, and stops
    # it (SIGKILL, exit 137) if the profile doesn't allow them. --build-kind answers before AppKit does anything, and ends.
    kind="$(mktemp "${RUNNER_TEMP:?}/build-kind.XXXXXX")"
    code=0
    started=$(now_ms)
    perl -e 'alarm shift; exec @ARGV' 60 "$APP/Contents/MacOS/RectoPDF" --build-kind > "$kind" 2>/dev/null || code=$?
    ms=$(($(now_ms) - started))
    answer="$(cat "$kind")"
    rm -f "$kind"
    if [ "$code" -eq 142 ]; then
      echo "::error::the signed app answering --build-kind didn't finish in 60 s: it hung"; keychains; exit 1
    elif [ "$code" -ge 128 ]; then
      echo "::error::the system stopped the signed app at launch (exit $code, after $ms ms): its entitlements aren't allowed by its embedded profile, or its signature isn't valid" >&2; exit 1
    elif [ "$code" -ne 0 ]; then
      echo "::error::the signed app answering --build-kind failed (exit $code, after $ms ms): $answer" >&2; exit 1
    fi
    case "$answer" in
      *'"release_build":true'*'"takes_test_hooks":false'*) echo "  ok  the signed app starts with its entitlements: $answer ($ms ms)" ;;
      *) echo "::error::the signed app isn't a release build that takes no test hooks: $answer" >&2; exit 1 ;;
    esac
    limited 300 "the signing job's checks once the keys are gone, and its quiet run drawing a PDF (app/release-zip.sh --check-signed --no-notarize)" \
      "$root/app/release-zip.sh" --check-signed --no-notarize
    ;;
esac
