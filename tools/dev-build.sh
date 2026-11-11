#!/usr/bin/env bash
# dev-build: a development build of the app, for the owner's one RectoPDF in /Applications (SPEC decision 24, "Development builds"; Seedwell's tools/dev-build.sh is the
# model). Built as a release is (RECTO_RELEASE=1: no override is read, and it takes releases by itself), numbered X.Y.Z.N: Cargo.toml's version and the next development
# number, counted in app/build.noindex/dev/counter from 1 for each version. A release never has a fourth part.
#
#   tools/sysguard/heavy.sh tools/dev-build.sh                  Builds, and makes app/build.noindex/dev/RectoPDF-X.Y.Z.N.zip. Prints its path and how to install it.
#   tools/dev-build.sh --package <RectoPDF.app> [--out <folder>] [--version X.Y.Z.N]
#                                                               Only the last part, on an app already built: numbers, signs, checks and zips a copy. Seconds.
#   tools/sysguard/heavy.sh tools/dev-build.sh --first-install  Once, to start: builds, and puts it in /Applications as the owner's one RectoPDF (below).
#
# Installing one needs Settings › Allow development builds on in the app (the owner's switch, which is also their consent to this one command at any agent level):
#   recto update install app/build.noindex/dev/RectoPDF-X.Y.Z.N.zip          goes in once the app hasn't been used for 5 minutes
#   recto update install app/build.noindex/dev/RectoPDF-X.Y.Z.N.zip --now    at once
#
# **Never the keychain** (owner, 2026-11-11: "I need to approve the use of the keychain, that in some cases takes hours"). No Developer ID, no `security`, no notarizing and no
# timestamp: ad hoc, with a designated requirement of the bundle identifier alone, so macOS (TCC) takes every development build as the same app and keeps the folders it was
# given. The renderer helper the same, with its own identifier. Developer ID and notarizing are only for a release, in CI, on a pushed tag (docs/RELEASING.md).
#
# Uncommitted changes are allowed; the commit is noted as <sha>-dirty. Run the build through tools/sysguard/heavy.sh.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
APP_ID=io.github.kkollsga.RectoPDF
OUT="$REPO/app/build.noindex/dev"

usage() { sed -n '2,10p' "$0"; exit 2; }
fail() { echo "dev-build: $*" >&2; exit 1; }

release_version() { sed -n '/^\[workspace.package\]/,/^\[/{s/^version *= *"\(.*\)"/\1/p;}' "$REPO/Cargo.toml" | head -1; }

# package <app> <folder> <X.Y.Z.N>: a copy of the app in <folder>, numbered, signed ad hoc with the identifier as its requirement, checked, and zipped beside it.
package() {
  local built=$1 out=$2 dev=$3 app id release
  release="${dev%.*}"
  [[ "$dev" =~ ^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.[1-9][0-9]{0,8}$ ]] || fail "$dev isn't X.Y.Z.N with N from 1"
  [ -x "$built/Contents/MacOS/RectoPDF" ] || fail "no app at $built"
  [ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$built/Contents/Info.plist")" = "$release" ] || fail "the app at $built isn't $release"
  # A folder of its own, so the app it's made from can be anywhere, the folder it's zipped into included.
  local staging="$out/.packaging"
  rm -rf "$staging"
  mkdir -p "$staging"
  app="$staging/RectoPDF.app"
  # A clone where the disk does them: no copy of the bytes.
  cp -c -R "$built" "$app" 2>/dev/null || cp -R "$built" "$app"
  /usr/libexec/PlistBuddy -c "Delete :RectoDevVersion" "$app/Contents/Info.plist" >/dev/null 2>&1 || true
  /usr/libexec/PlistBuddy -c "Add :RectoDevVersion string $dev" "$app/Contents/Info.plist"
  id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")"
  # From the inside out, never --deep, which would sign the helper again without its sandbox. The programs beside the app keep build.sh's ad hoc signatures.
  "$REPO/app/render-helper.sh" --sign-dev "$app/Contents" >/dev/null
  codesign --force --sign - -r="designated => identifier \"$id\"" "$app" 2>/dev/null
  codesign --verify --deep --strict "$app" || fail "the build's signature doesn't verify"
  codesign -d -r- "$app" 2>&1 | grep -qxF "designated => identifier \"$id\"" || fail "the build's designated requirement isn't its identifier"
  "$REPO/app/render-helper.sh" --check "$app" >/dev/null
  rm -f "$out"/RectoPDF-*.zip
  # Without resource forks or extended attributes: the updater's reader would write one out as a file the signature doesn't cover.
  ditto -c -k --norsrc --noextattr --keepParent "$app" "$out/RectoPDF-$dev.zip"
  rm -rf "$staging"
  echo "$out/RectoPDF-$dev.zip"
}

# The Mac's own idle time, in seconds: how long since anyone touched a key or the pointer.
hid_idle() { ioreg -c IOHIDSystem | awk '/HIDIdleTime/ { print int($NF / 1000000000); exit }'; }

# The owner's RectoPDF running from anywhere but /Applications: not a quiet run, which is a test's, on its own state.
running_elsewhere() {
  local pid command
  for pid in $(pgrep -x RectoPDF || true); do
    command="$(ps -o command= -p "$pid" || true)"
    case "$command" in /Applications/RectoPDF.app/*|*" --qa"*) continue ;; esac
    echo "$pid"
  done
}

mode=build built="" out="$OUT" dev=""
while [ $# -gt 0 ]; do
  case "$1" in
    --package) mode=package; built="${2:?--package needs the built app}"; shift ;;
    --out) out="${2:?--out needs a folder}"; shift ;;
    --version) dev="${2:?--version needs X.Y.Z.N}"; shift ;;
    --first-install) mode=first ;;
    -h|--help) usage ;;
    *) echo "dev-build: unknown argument $1" >&2; usage ;;
  esac
  shift
done

if [ "$mode" = package ]; then
  [ -n "$dev" ] || dev="$(release_version).1"
  package "$built" "$out" "$dev"
  exit 0
fi

if [ "$mode" = first ] && [ -e /Applications/RectoPDF.app ]; then
  fail "RectoPDF is already in /Applications. From here on: tools/dev-build.sh, then recto update install <zip>."
fi

VERSION="$(release_version)"
mkdir -p "$OUT"
read -r last n < "$OUT/counter" 2>/dev/null || { last=""; n=0; }
[ "$last" = "$VERSION" ] || n=0
DEV="$VERSION.$((n + 1))"
COMMIT="$(git -C "$REPO" rev-parse --short HEAD)"
[ -z "$(git -C "$REPO" status --porcelain --untracked-files=no)" ] || COMMIT="$COMMIT-dirty"
echo "==> RectoPDF $DEV, a development build of $COMMIT"

# Release code in the faster profile where there is one, into a folder of its own, so the tests' app (a local build) is left as it is.
profile=--release
grep -q -- '--dev-release)' "$REPO/app/build.sh" && profile=--dev-release
(unset RECTO_SIGN_IDENTITY; RECTO_RELEASE=1 RECTO_APP_BUILD="$OUT/app" "$REPO/app/build.sh" "$profile")
ZIP="$(package "$OUT/app/RectoPDF.app" "$OUT" "$DEV")"
# Counted only once it's made: a failed build doesn't use up a number.
echo "$VERSION $((n + 1))" > "$OUT/counter"

if [ "$mode" = first ]; then
  # The one-time move from the old dev copy to /Applications. It's swapped while running only after 5 minutes with no use of the Mac (owner, 2026-11-11), and then
  # told to end with SIGTERM. A copy built from `build/update-gaps` on takes SIGTERM as a quit, writing everything as any quit does. An older one, such as the owner's
  # copy at 11276a3, just ends, and the 5 idle minutes are what cover it: by then everything's on disk, edits within 5 s and the place within a minute (SPEC decision 25).
  was_running=""
  for pid in $(running_elsewhere); do
    was_running=1
    while [ "$(hid_idle)" -lt 300 ]; do
      echo "==> RectoPDF is in use (process $pid): waiting for 5 minutes with no use of this Mac"
      sleep 30
    done
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
    kill -0 "$pid" 2>/dev/null && fail "RectoPDF (process $pid) didn't end, so nothing was installed"
  done
  unpacked="$(mktemp -d "${TMPDIR:-/tmp}/recto-first-install.XXXXXX")"
  ditto -x -k "$ZIP" "$unpacked"
  ditto "$unpacked/RectoPDF.app" /Applications/RectoPDF.app
  rm -rf "$unpacked"
  echo "==> /Applications/RectoPDF.app is RectoPDF $DEV, the one install. The old dev copy is left where it was, for the coordinator to retire."
  # It was running, so it comes back, behind whatever's in front.
  [ -z "$was_running" ] || open -g /Applications/RectoPDF.app
  exit 0
fi
echo "$ZIP"
echo "install: recto update install \"$ZIP\"    (add --now to go at once, not after 5 minutes without use)"
