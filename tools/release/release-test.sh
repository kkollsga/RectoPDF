#!/usr/bin/env bash
# release-test: proves the release scripts' refusals and their output in throwaway folders, with made files and no build, network,
# signing identity or keychain. Seconds. Run after any change to tools/release/ or app/release-zip.sh (tools/check.sh --full does, as
# tools/check-scope.sh maps them to "release").
#
#   tools/release/release-test.sh
#
# It runs tools/release/ci-gate-test.sh (the release gate's) and tools/release/carry/expect_test.py (check-carry's expectations), so
# tools/check.sh runs them when tools/release/ changes.
# What it doesn't do: check-draft.sh's codesign, Gatekeeper and stapler checks of a real signed bundle, and 0.1.0's own verdict, which need
# the downloaded 0.1.0 zip and a build. `tools/release/check-draft.sh --self-test <RectoPDF-0.1.0.zip>` runs those (docs/RELEASING.md).
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/../.." && pwd)"
X=$(mktemp -d "${TMPDIR:-/tmp}/release-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
unset NOTES_ROOT BUMP_ROOT
fails=0
check() { # name, expected, actual
  if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi
}
has() { # name, text wanted, in what
  case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$3]"; fails=$((fails + 1)) ;; esac
}
hasnt() { # name, text not wanted, in what
  case "$3" in *"$2"*) echo "FAIL  $1: [$2] in [$3]"; fails=$((fails + 1)) ;; *) echo "ok    $1" ;; esac
}
NOTES="$SOURCE/tools/release/notes.sh"
BUMP="$SOURCE/tools/release/bump.sh"
SUM=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

# ---- notes.sh: the draft's body from docs/releases/X.Y.Z.md ----
N="$X/notes"; mkdir -p "$N/docs/releases"
notes() { NOTES_ROOT="$N" "$NOTES" "$@" 2>&1; }
good_notes() {
  cat > "$N/docs/releases/1.2.3.md" <<EOF
RectoPDF 1.2.3 fixes things found in 1.2.2.

## Install

macOS 15 or later, Apple silicon. Download [RectoPDF-1.2.3.zip](https://github.com/kkollsga/RectoPDF/releases/download/v1.2.3/RectoPDF-1.2.3.zip), open it.

## Fixes

- A fix, which 1.2.2 got wrong.

## Checksums

\`\`\`
(written by the release workflow)
\`\`\`
EOF
}
printf '%s  RectoPDF-1.2.3.zip\n' "$SUM" > "$X/SHA256SUMS"

# Failure: a release goes out with no notes at all, or the draft step's body is made some other way and the tag is withdrawn an hour in.
out=$(notes --check 1.2.3); rc=$?
check "notes: no file for the version is refused" 1 "$rc"
has "notes: and it says which version has none" "no notes for 1.2.3" "$out"

# Failure: the body is the notes with the workflow's stand-in where the checksums go, or the Install part twice.
good_notes
body=$(NOTES_ROOT="$N" "$NOTES" 1.2.3 "$X/SHA256SUMS" 2>/dev/null); rc=$?
check "notes: good notes make a body" 0 "$rc"
has "notes: the checksums are filled in" "$SUM  RectoPDF-1.2.3.zip" "$body"
hasnt "notes: the stand-in is gone" "written by the release workflow" "$body"
check "notes: one Install part" 1 "$(printf '%s\n' "$body" | grep -c '^## Install')"
has "notes: the rest is the file's" "A fix, which 1.2.2 got wrong." "$body"
out=$(notes --check 1.2.3); rc=$?
check "notes: --check passes good notes" 0 "$rc"

# Failure: the notes of another version are published as this one's (a copied file, a link to the old zip that every reader would download).
sed 's/^RectoPDF 1.2.3 fixes/RectoPDF 1.2.2 fixes/' "$N/docs/releases/1.2.3.md" > "$X/t" && mv "$X/t" "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: a first line naming another version is refused" 1 "$rc"
has "notes: and says so" "first line doesn't say it's RectoPDF 1.2.3" "$out"
good_notes
sed 's#download/v1.2.3/RectoPDF-1.2.3.zip#download/v1.2.2/RectoPDF-1.2.2.zip#' "$N/docs/releases/1.2.3.md" > "$X/t" && mv "$X/t" "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: a link to another version's zip is refused" 1 "$rc"
has "notes: and names it" "1.2.2" "$out"

# Failure: a paragraph marked to be put right is published as it stands. The marker is an HTML comment, which the page wouldn't show.
good_notes
printf '\n<!-- PLACEHOLDER: the next line is to be confirmed -->\n- A limit.\n' >> "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: the placeholder marker is refused" 1 "$rc"
has "notes: and its line is named" "placeholder, line" "$out"
good_notes
printf -- '- [Placeholder, to be confirmed] A limit.\n' >> "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: a bracketed placeholder is refused too" 1 "$rc"

# Failure: the body is over GitHub's limit, so the draft step fails after the signing and the tag is withdrawn.
good_notes
python3 -I -c 'import sys; open(sys.argv[1], "a").write("- x" * 34000 + "\n")' "$N/docs/releases/1.2.3.md"
out=$(notes --check 1.2.3); rc=$?
check "notes: a body over the bound is refused" 1 "$rc"
has "notes: and says the bound" "over the 100,000" "$out"

# Failure: the checksums block names another file, or two, and the body tells people to check the wrong thing.
good_notes
printf '%s  RectoPDF-1.2.2.zip\n' "$SUM" > "$X/SUMS-other"
out=$(NOTES_ROOT="$N" "$NOTES" 1.2.3 "$X/SUMS-other" 2>&1 >/dev/null); rc=$?
check "notes: SHA256SUMS of another zip is refused" 1 "$rc"
cat "$X/SHA256SUMS" "$X/SHA256SUMS" > "$X/SUMS-two"
out=$(NOTES_ROOT="$N" "$NOTES" 1.2.3 "$X/SUMS-two" 2>&1 >/dev/null); rc=$?
check "notes: SHA256SUMS of two lines is refused" 1 "$rc"

# Failure: notes written without an Install or a Checksums part publish a body with no way to download it or check it.
printf 'RectoPDF 1.2.3 is a release.\n\n## Fixes\n\n- One.\n' > "$N/docs/releases/1.2.3.md"
body=$(NOTES_ROOT="$N" "$NOTES" 1.2.3 "$X/SHA256SUMS" 2>/dev/null); rc=$?
check "notes: notes with neither part still make a body" 0 "$rc"
has "notes: with 0.1.0's Install part added" "Download [RectoPDF-1.2.3.zip](https://github.com/kkollsga/RectoPDF/releases/download/v1.2.3/RectoPDF-1.2.3.zip)" "$body"
has "notes: and its Checksums part" '```'$'\n'"$SUM  RectoPDF-1.2.3.zip" "$body"
check "notes: the opening paragraph stays first" "RectoPDF 1.2.3 is a release." "$(printf '%s\n' "$body" | head -1)"

# ---- bump.sh: every file that names the version ----
B="$X/bump"
tree() { # a tree as the repository has it, at version $1, with every file the bump knows
  rm -rf "$B"; mkdir -p "$B/crates/recto-core" "$B/crates/recto-headings" "$B/fuzz" "$B/spikes/sandbox"
  printf '[workspace]\nmembers = ["crates/*"]\n\n[workspace.package]\nversion = "%s"\nedition = "2024"\n\n[workspace.dependencies]\nserde = { version = "1.0.0" }\n' "$1" > "$B/Cargo.toml"
  printf '[package]\nname = "recto-core"\nversion.workspace = true\n' > "$B/crates/recto-core/Cargo.toml"
  printf '[package]\nname = "recto-headings"\nversion = "%s"\n' "$1" > "$B/crates/recto-headings/Cargo.toml"
  lock() { printf '# @generated\nversion = 4\n\n[[package]]\nname = "byteorder-lite"\nversion = "%s"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n\n[[package]]\nname = "recto-core"\nversion = "%s"\ndependencies = [\n "byteorder-lite",\n]\n\n[[package]]\nname = "%s"\nversion = "0.0.0"\n' "$1" "$1" "$2"; }
  lock "$1" recto-fuzz-targets > "$B/Cargo.lock"
  { lock "$1" recto-fuzz-targets; printf '\n[[package]]\nname = "recto-headings"\nversion = "%s"\n' "$1"; } > "$B/crates/recto-headings/Cargo.lock"
  lock "$1" recto-fuzz-targets > "$B/fuzz/Cargo.lock"
  lock "$1" recto-sandbox-spike > "$B/spikes/sandbox/Cargo.lock"
  printf 'RectoPDF %s: third-party software notices\n\nships byteorder-lite %s\n' "$1" "$1" > "$B/THIRD-PARTY-NOTICES.txt"
  printf '# Privacy\n\nThis describes RectoPDF %s.\n' "$1" > "$B/PRIVACY.md"
  printf '1. Download `RectoPDF-%s.zip` from the latest release.\n- In %s, deleting pages is done with recto.\n' "$1" "$1" > "$B/README.md"
}
bump() { BUMP_ROOT="$B" "$BUMP" "$@" 2>&1; }

# Failure: --check calls a tree consistent that isn't, or not one that is.
tree 0.1.0
out=$(bump --check); rc=$?
check "bump: --check passes a tree at one version" 0 "$rc"
has "bump: and names it" "names 0.1.0" "$out"

# Failure: a bump misses a file (the headings lockfile, built --locked, fails the release build), or changes a third-party package of
# the same version, or the fuzz or spike workspace's own 0.0.0.
out=$(bump 0.1.1 --no-cargo); rc=$?
check "bump: 0.1.0 to 0.1.1 succeeds" 0 "$rc"
out=$(bump --check 0.1.1); rc=$?
check "bump: and the tree is then at 0.1.1 ($out)" 0 "$rc"
check "bump: the headings lockfile's own packages follow" 2 "$(grep -c '^version = "0.1.1"' "$B/crates/recto-headings/Cargo.lock")"
check "bump: a third-party package of the same version stays" 'version = "0.1.0"' "$(grep -A1 'name = "byteorder-lite"' "$B/Cargo.lock" | tail -1)"
check "bump: the fuzz workspace's own package stays" 'version = "0.0.0"' "$(grep -A1 'name = "recto-fuzz-targets"' "$B/fuzz/Cargo.lock" | tail -1)"
check "bump: the spike's own package stays" 'version = "0.0.0"' "$(grep -A1 'name = "recto-sandbox-spike"' "$B/spikes/sandbox/Cargo.lock" | tail -1)"
check "bump: a dependency's version in Cargo.toml stays" 'serde = { version = "1.0.0" }' "$(grep '^serde' "$B/Cargo.toml")"
check "bump: the notices' third-party line stays" "ships byteorder-lite 0.1.0" "$(sed -n 3p "$B/THIRD-PARTY-NOTICES.txt")"
check "bump: README's prose about a version stays" "- In 0.1.0, deleting pages is done with recto." "$(sed -n 2p "$B/README.md")"
check "bump: README's zip follows" '1. Download `RectoPDF-0.1.1.zip` from the latest release.' "$(sed -n 1p "$B/README.md")"

# Failure: a tree bumped by hand in part is called consistent, or a bump goes ahead over it and hides which file was behind.
tree 0.1.0
sed 's/^version = "0.1.0"$/version = "0.1.1"/' "$B/Cargo.toml" > "$X/t" && mv "$X/t" "$B/Cargo.toml"
out=$(bump --check); rc=$?
check "bump: --check refuses a tree bumped in part" 1 "$rc"
has "bump: and names each file behind" "crates/recto-headings/Cargo.lock: names 0.1.0 where 0.1.1 is wanted" "$out"
before=$(cat "$B/PRIVACY.md")
out=$(bump 0.1.2 --no-cargo); rc=$?
check "bump: a bump over a tree in part is refused" 1 "$rc"
check "bump: and changes nothing" "$before" "$(cat "$B/PRIVACY.md")"

# Failure: a version is bumped backwards or to itself (a version once published is spent), or to something the updater won't read.
tree 0.1.1
out=$(bump 0.1.1 --no-cargo); rc=$?
check "bump: to the same version is refused" 1 "$rc"
out=$(bump 0.1.0 --no-cargo); rc=$?
check "bump: to an older version is refused" 1 "$rc"
has "bump: and says why" "spent" "$out"
for bad in v0.1.2 0.1.2.1 01.1.2 0.1; do
  out=$(bump "$bad" --no-cargo); rc=$?
  check "bump: $bad isn't a release version" 2 "$rc"
done
check "bump: 0.1.10 is newer than 0.1.9 (by number, not text)" 0 "$(tree 0.1.9; bump 0.1.10 --no-cargo >/dev/null; echo $?)"

# Failure: a file the bump knows is gone or renamed, and --check passes with it unchecked.
tree 0.1.0
rm "$B/PRIVACY.md"
out=$(bump --check); rc=$?
check "bump: a missing file fails --check" 1 "$rc"
has "bump: and is named" "PRIVACY.md: missing" "$out"
tree 0.1.0
printf '# Privacy\n' > "$B/PRIVACY.md"
out=$(bump --check); rc=$?
check "bump: a file that no longer names the version fails --check" 1 "$rc"

# Failure: this repository's own tree isn't at one version, or the bump's table no longer fits its files.
out=$("$BUMP" --check 2>&1); rc=$?
check "bump: this repository is at one version ($out)" 0 "$rc"

# ---- app-entitlements.sh: the release's entitlements, and a signed bundle checked against them ----
ENT="$SOURCE/tools/release/app-entitlements.sh"
E="$X/entitlements"; mkdir -p "$E"
"$ENT" --write "$E/claims.plist"
# Failure: the entitlements a release signs with aren't a plist codesign reads, or name another App ID than the profile's.
check "entitlements: --write makes a plist" 0 "$(plutil -lint "$E/claims.plist" >/dev/null 2>&1; echo $?)"
has "entitlements: with the App ID and the group" "<string>6GND8Z2C7Z.io.github.kkollsga.RectoPDF</string>" "$(cat "$E/claims.plist")"
bundle() { # path: a bundle of two made programs, signed ad hoc, which is all `--check` reads
  mkdir -p "$1/Contents/MacOS"
  cp /usr/bin/true "$1/Contents/MacOS/RectoPDF"
  cp /usr/bin/true "$1/Contents/MacOS/recto"
  printf '<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>io.github.kkollsga.RectoPDF</string><key>CFBundleExecutable</key><string>RectoPDF</string></dict></plist>' > "$1/Contents/Info.plist"
}
# Failure: a release whose bundle has no provisioning profile passes, though its main executable can't then launch with the entitlements.
bundle "$E/no-profile.app"
codesign --force --sign - "$E/no-profile.app/Contents/MacOS/recto" 2>/dev/null
codesign --force --sign - --entitlements "$E/claims.plist" "$E/no-profile.app" 2>/dev/null
out=$("$ENT" --check "$E/no-profile.app" 2>&1); rc=$?
check "entitlements: a bundle with no profile is refused" 1 "$rc"
has "entitlements: and named" "the bundle has no Contents/embedded.provisionprofile" "$out"
hasnt "entitlements: while its main executable's claims are right" "the app's main executable claims" "$out"
# Failure: a release whose profile isn't one, whose main executable claims nothing, or whose other programs claim the app's group passes.
bundle "$E/wrong.app"
echo "not a profile" > "$E/wrong.app/Contents/embedded.provisionprofile"
codesign --force --sign - --entitlements "$E/claims.plist" "$E/wrong.app/Contents/MacOS/recto" 2>/dev/null
codesign --force --sign - "$E/wrong.app" 2>/dev/null
out=$("$ENT" --check "$E/wrong.app" 2>&1); rc=$?
check "entitlements: a wrong bundle is refused" 1 "$rc"
has "entitlements: a profile that isn't one" "isn't a signed provisioning profile" "$out"
has "entitlements: a main executable that claims nothing" "the app's main executable claims com.apple.application-identifier = None" "$out"
has "entitlements: another program that claims the group" "Contents/MacOS/recto claims the app's identity or keychain group" "$out"
# A profile as the file keychain's tools read one: a plist in signed CMS, made here with LibreSSL and a throwaway certificate (security cms -D
# decodes it without asking who signed it, which is all --check reads it for).
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -keyout "$E/k.pem" -out "$E/c.pem" -days 1 -subj "/CN=release-test" >/dev/null 2>&1
printf '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>Entitlements</key><dict><key>com.apple.application-identifier</key><string>6GND8Z2C7Z.io.github.kkollsga.RectoPDF</string><key>keychain-access-groups</key><array><string>6GND8Z2C7Z.*</string></array></dict></dict></plist>' > "$E/profile.plist"
/usr/bin/openssl cms -sign -nodetach -binary -outform DER -in "$E/profile.plist" -signer "$E/c.pem" -inkey "$E/k.pem" -out "$E/made.provisionprofile" 2>/dev/null
# Failure: the made profile isn't one `security cms -D` decodes, so the cases below pass or fail on that rather than on what they name.
has "entitlements: the made profile decodes, with its App ID" "6GND8Z2C7Z.io.github.kkollsga.RectoPDF" "$(security cms -D -i "$E/made.provisionprofile" 2>&1)"
# Failure: the checker refuses a bundle as a release makes it, so its refusals of the others mean nothing.
bundle "$E/right.app"
cp "$E/made.provisionprofile" "$E/right.app/Contents/embedded.provisionprofile"
codesign --force --sign - "$E/right.app/Contents/MacOS/recto" 2>/dev/null
codesign --force --sign - --entitlements "$E/claims.plist" "$E/right.app" 2>/dev/null
out=$("$ENT" --check "$E/right.app" 2>&1); rc=$?
check "entitlements: a bundle as a release makes it passes ($out)" 0 "$rc"
# Failure: a main executable claiming one entitlement more than the three passes, so a release could ship a power nobody decided on.
bundle "$E/more.app"
cp "$E/made.provisionprofile" "$E/more.app/Contents/embedded.provisionprofile"
sed 's#</dict></plist>#  <key>com.apple.security.get-task-allow</key><true/>\n</dict></plist>#' "$E/claims.plist" > "$E/more.plist"
# Failure: the plist with one more is malformed (a sed that writes `\n` as it is), so codesign signs nothing and the case fails for another reason.
check "entitlements: the plist with one more is valid" 0 "$(plutil -lint "$E/more.plist" >/dev/null 2>&1; echo $?)"
codesign --force --sign - --entitlements "$E/more.plist" "$E/more.app" 2>/dev/null
out=$("$ENT" --check "$E/more.app" 2>&1); rc=$?
check "entitlements: one entitlement more is refused" 1 "$rc"
has "entitlements: and named" "claims com.apple.security.get-task-allow as well" "$out"
# Failure: code in Frameworks (or PlugIns, XPCServices, Library) claiming the app's group passes, as only MacOS and Helpers were looked at.
bundle "$E/framework.app"
cp "$E/made.provisionprofile" "$E/framework.app/Contents/embedded.provisionprofile"
mkdir -p "$E/framework.app/Contents/Frameworks"
cp /usr/bin/true "$E/framework.app/Contents/Frameworks/claims"
codesign --force --sign - --entitlements "$E/claims.plist" "$E/framework.app/Contents/Frameworks/claims" 2>/dev/null
codesign --force --sign - --entitlements "$E/claims.plist" "$E/framework.app" 2>/dev/null
out=$("$ENT" --check "$E/framework.app" 2>&1); rc=$?
check "entitlements: code in Frameworks claiming the group is refused" 1 "$rc"
has "entitlements: and named" "Contents/Frameworks/claims claims the app's identity" "$out"

# ---- release-zip.sh --no-notarize: the preflight stops the release's own path, and the release's path is as it was ----
# The real release-zip.sh in a made repository, with the tools that would reach a keychain, Apple or a signature (codesign, security, xcrun,
# spctl) and the scripts it calls replaced by ones that write down what they were asked; so no key, network or real signing.
R="$X/zip"; RB="$R/bin"; CALLS="$R/calls"
mkdir -p "$RB" "$R/repo/app" "$R/repo/tools/release" "$R/repo/crates/recto-update/src"
# As release-zip.sh names its paths (cd and pwd), so the calls written down compare: TMPDIR ends in a slash here.
R="$(cd "$R" && pwd -P)"; RB="$R/bin"; CALLS="$R/calls"
export CALLS
logger() { # name, then the script's body after the line that writes the call down
  printf '#!/bin/sh\necho "%s $*" >> "$CALLS"\n%s\n' "$1" "${2:-}" > "$3"; chmod +x "$3"
}
logger codesign 'case "$1" in -dv) echo "TeamIdentifier=TEAMID1" >&2 ;; esac' "$RB/codesign"
# In one write, as the real one's few lines are: release-zip.sh reads it with `grep -q` under pipefail, which a second write could meet closed.
logger security 'printf "  1) ABCDEF \"Developer ID Application: Test (TEAMID1)\"\n     1 valid identities found\n"' "$RB/security"
logger xcrun 'case "$1 $2" in "notarytool submit") echo "{\"status\":\"Accepted\",\"id\":\"made\"}" ;; esac' "$RB/xcrun"
logger spctl '' "$RB/spctl"
cp "$SOURCE/app/release-zip.sh" "$R/repo/app/release-zip.sh"
logger build.sh '' "$R/repo/app/build.sh"
logger render-helper.sh '' "$R/repo/app/render-helper.sh"
logger app-entitlements.sh '' "$R/repo/tools/release/app-entitlements.sh"
logger notes.sh 'echo "made notes"' "$R/repo/tools/release/notes.sh"
logger worker-env.sh '' "$R/repo/tools/release/worker-env.sh"
printf '[workspace.package]\nversion = "9.9.9"\n' > "$R/repo/Cargo.toml"
printf 'pub const TEAM_ID: &str = "TEAMID1";\npub const REPO: &str = "https://example.invalid/made";\n' > "$R/repo/crates/recto-update/src/lib.rs"
echo "made notices" > "$R/repo/THIRD-PARTY-NOTICES.txt"
printf '/app/build.noindex\n' > "$R/repo/.gitignore"
MA="$R/repo/app/build.noindex/RectoPDF.app"
mkdir -p "$MA/Contents/MacOS" "$MA/Contents/Resources"
cp "$R/repo/THIRD-PARTY-NOTICES.txt" "$MA/Contents/Resources/Licenses.txt"
printf '<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleShortVersionString</key><string>9.9.9</string><key>LSMinimumSystemVersion</key><string>15.0</string></dict></plist>' > "$MA/Contents/Info.plist"
# The app's `recto` answers a quiet run as a release that drew does, and refuses a test's verbs; its installer unpacks a zip.
logger recto 'case "$1 $2" in
  "qa state") echo "{\"data\":{\"app\":{\"pages\":{\"tile_layers\":1}},\"reader\":{\"settled\":true,\"pages\":1,\"tiles\":{\"attached\":1}},\"memory\":{\"workers_walled\":1},\"update\":{\"build\":\"release\"}}}" ;;
  "qa update-clock"|"qa as-person"|"qa owner-edit"|"qa save-as"|"qa save-from") exit 1 ;;
esac' "$MA/Contents/MacOS/recto"
logger recto-updater 'mkdir -p "$3" && ditto -x -k "$2" "$3"' "$MA/Contents/MacOS/recto-updater"
git -C "$R/repo" init -q && git -C "$R/repo" add -A && git -C "$R/repo" -c user.name=t -c user.email=t@t commit -qm made
signing() { # the signing job's settings, the made tools first on PATH, and release-zip.sh's arguments
  : > "$CALLS"
  env PATH="$RB:$PATH" RECTO_SIGN_IDENTITY="Developer ID Application: Test (TEAMID1)" RECTO_NOTARY_PROFILE=made \
    RECTO_PROVISION_PROFILE="$R/repo/Cargo.toml" "$R/repo/app/release-zip.sh" "$@" 2>&1
}
OUTDIR="$R/repo/app/build.noindex/release"
# Failure: --no-notarize is taken where it would make an unnotarized release (the full local --signed path, the build job's), or alone.
for args in "--no-notarize" "--signed --no-notarize" "--built-only --no-notarize"; do
  # shellcheck disable=SC2086 # the arguments, split on purpose
  out=$(signing $args); rc=$?
  check "release-zip: $args is refused" 2 "$rc"
  has "release-zip: $args, and why" "--no-notarize goes with --signed --already-built or with --check-signed" "$out"
done
check "release-zip: a refused --no-notarize built and signed nothing" "" "$(cat "$CALLS")"
# Failure: the preflight's signing notarizes (minutes of Apple's service, and a submission of a build nobody released), staples or zips,
# or doesn't sign by the release's own build.sh --sign-only and check the team.
out=$(signing --signed --already-built --no-notarize); rc=$?
calls=$(cat "$CALLS")
check "release-zip: the preflight's signing passes ($out)" 0 "$rc"
has "release-zip: the preflight signs as the release does" "build.sh --sign-only" "$calls"
has "release-zip: and checks the team" "codesign -dv --verbose=2 $MA/Contents/Helpers/RectoRender.app" "$calls"
hasnt "release-zip: the preflight doesn't notarize" "notarytool" "$calls"
hasnt "release-zip: nor staple" "stapler" "$calls"
check "release-zip: nor zip" "no" "$([ -e "$OUTDIR" ] && echo yes || echo no)"
# Failure: the release's own signing, the same command without the flag, no longer notarizes, staples, checks Gatekeeper and zips.
out=$(signing --signed --already-built); rc=$?
calls=$(cat "$CALLS")
check "release-zip: the release's signing passes ($out)" 0 "$rc"
has "release-zip: the release signs" "build.sh --sign-only" "$calls"
has "release-zip: the release notarizes" "xcrun notarytool submit" "$calls"
has "release-zip: the release staples" "xcrun stapler staple $MA" "$calls"
has "release-zip: the release asks Gatekeeper" "spctl --assess --type execute --verbose=2 $MA" "$calls"
check "release-zip: the release zips" "yes" "$([ -f "$OUTDIR/RectoPDF-9.9.9.zip" ] && [ -f "$OUTDIR/latest.json" ] && echo yes || echo no)"
# Failure: the preflight's run of the signed app asks for a ticket it can't have or a zip that isn't there, or skips the release's checks
# of the signed app and its quiet run.
out=$(signing --check-signed --no-notarize); rc=$?
calls=$(cat "$CALLS")
check "release-zip: the preflight's checks of the signed app pass ($out)" 0 "$rc"
has "release-zip: the preflight checks the entitlements" "app-entitlements.sh --check $MA" "$calls"
has "release-zip: the preflight checks the helper" "render-helper.sh --check $MA" "$calls"
has "release-zip: the preflight runs the signed app drawing a PDF" "recto qa state --json" "$calls"
has "release-zip: and reads its worker's allocator setting" "worker-env.sh $MA" "$calls"
hasnt "release-zip: the preflight asks for no ticket" "stapler" "$calls"
hasnt "release-zip: nor checks a zip" "check-zip" "$calls"
# Failure: the release's own run of the stapled app no longer validates its ticket or checks the zip as the updater will.
out=$(signing --check-signed); rc=$?
calls=$(cat "$CALLS")
check "release-zip: the release's checks of the stapled app pass ($out)" 0 "$rc"
has "release-zip: the release validates the ticket" "xcrun stapler validate $MA" "$calls"
has "release-zip: the release runs the stapled app" "recto qa state --json" "$calls"
has "release-zip: and reads its worker's allocator setting" "worker-env.sh $MA" "$calls"
has "release-zip: the release checks the zip as the updater will" "recto-updater check-zip $OUTDIR/RectoPDF-9.9.9.zip" "$calls"

# ---- preflight-app.sh and limited.sh: the preflight's real-app steps ----
# Failure: the preflight's signing path runs on a person's Mac, where a team signing would reach their keychain.
for mode in --build --sign --check; do
  out=$(env -u GITHUB_ACTIONS "$SOURCE/tools/release/preflight-app.sh" "$mode" 2>&1); rc=$?
  check "preflight-app: $mode is refused outside CI" 2 "$rc"
  has "preflight-app: $mode, and why" "on its runner only (GITHUB_ACTIONS is unset)" "$out"
done
# Failure: a signing or a launch that waits (as on a keychain prompt) holds the preflight for its whole job instead of failing at its limit
# with its name; or a failure under the limit is taken as a pass. `security` is a made one, so nothing reads this Mac's keychains.
limits() { env PATH="$RB:$PATH" bash -c '. "$1"; shift; limited "$@"' _ "$SOURCE/tools/release/limited.sh" "$@" 2>&1; }
started=$(date +%s)
out=$(limits 1 "a made wait" sleep 30); rc=$?
check "limited: a command past its limit fails" 1 "$rc"
has "limited: and names itself" "a made wait didn't finish in 1 s: it hung" "$out"
has "limited: with the keychains as they are" "signing identities:" "$out"
check "limited: at its limit, not the command's end" yes "$([ $(($(date +%s) - started)) -lt 10 ] && echo yes || echo no)"
out=$(limits 10 "a made failure" false); rc=$?
check "limited: a failure under the limit fails" 1 "$rc"
has "limited: and says so" "a made failure failed (exit 1" "$out"
out=$(limits 10 "a made pass" true); rc=$?
check "limited: a pass passes" 0 "$rc"
has "limited: with its time" "ok  a made pass (" "$out"

# ---- worker-env.sh: a live render worker was started with MallocLargeCache=0 (read by `ps -E`, as release-zip.sh's quiet run reads the real one) ----
WA="$X/Made.app"
WORKER="$WA/Contents/Helpers/RectoRender.app/Contents/MacOS/recto-render-service"
mkdir -p "$(dirname "$WORKER")"
# sleep, signed ad hoc: the system hides the environment of its own (platform) programs from ps -E, and not of a program like the worker, ad hoc
# or with the hardened runtime (seen 2026-11-16); a Developer ID worker is read by the release preflight.
cp /bin/sleep "$WORKER" && codesign -f -s - "$WORKER" 2>/dev/null
# The made worker, started by its path as the made folder spells it ($X may hold a doubled slash, and /var for /private/var), with the environment given.
# It returns once the process is the worker (env has exec'd it), polling for at most 5 s.
worker_like() {
  local pid n=0
  env -i "$@" "$WORKER" 30 > /dev/null 2>&1 &
  pid=$!
  until case "$(ps -p "$pid" -o command= 2>/dev/null)" in "$WORKER"*) true ;; *) false ;; esac || [ $n -ge 50 ]; do sleep 0.1; n=$((n + 1)); done
  echo "$pid"
}
# Failure: the check passes with no worker to read, so a run whose worker ended early says nothing of its setting.
out=$("$SOURCE/tools/release/worker-env.sh" "$WA" 2>&1); rc=$?
check "worker-env: no live worker is refused" 1 "$rc"
has "worker-env: and says so" "no live render worker" "$out"
# Failure: a worker started without the setting (a host that clears it, or a signature it doesn't reach) passes, and the 100 to 175 MiB come back unseen.
bare=$(worker_like LANG=C)
out=$("$SOURCE/tools/release/worker-env.sh" "$WA" 2>&1); rc=$?
check "worker-env: a worker without MallocLargeCache=0 is refused" 1 "$rc"
has "worker-env: naming it" "the render worker $bare wasn't started with MallocLargeCache=0" "$out"
kill "$bare" 2>/dev/null; wait "$bare" 2>/dev/null
# Failure: the check refuses the real setting, and every release run fails on it; or it misses the worker because the bundle is named by
# another spelling of its path than the one the worker was started by (here a link; on a Mac, /var and /private/var).
set_one=$(worker_like MallocLargeCache=0)
ln -s "$X" "$X/link"
out=$("$SOURCE/tools/release/worker-env.sh" "$X/link/Made.app" 2>&1); rc=$?
check "worker-env: a worker with MallocLargeCache=0 passes" 0 "$rc"
has "worker-env: and says the vmmap line or why not" "vmmap --summary $set_one" "$out"
kill "$set_one" 2>/dev/null; wait "$set_one" 2>/dev/null

# ---- carry/expect.py: what check-carry expects of a pair, by what its older release can read, on made states ----
out=$(python3 -I "$SOURCE/tools/release/carry/expect_test.py" 2>&1); rc=$?
check "carry expectations: their own test passes ($(printf '%s\n' "$out" | sed -n 's/^Ran \([0-9]*\) tests.*/\1/p') cases)" 0 "$rc"
[ "$rc" = 0 ] || printf '%s\n' "$out" | tail -30

# ---- ci-gate.sh: the release's gate on CI, by its own test (made runs and a made repository) ----
out=$("$SOURCE/tools/release/ci-gate-test.sh" 2>&1); rc=$?
check "ci-gate: its own test passes ($(printf '%s\n' "$out" | grep -c '^ok') checks)" 0 "$rc"
[ "$rc" = 0 ] || printf '%s\n' "$out" | grep -v '^ok'

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
