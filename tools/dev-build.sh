#!/usr/bin/env bash
# dev-build: a development build of the app, for the owner's one RectoPDF in /Applications (SPEC decision 24, "Development builds"; Seedwell's tools/dev-build.sh is the
# model). Built as a release is (RECTO_RELEASE=1: no override is read, and it takes releases by itself), numbered X.Y.Z.N: Cargo.toml's version and the next development
# number from 1 for each version, one count for every worktree: the next after the larger of what's installed (/Applications/RectoPDF.app's RectoDevVersion)
# and the count kept beside the repository's own git folder (`git rev-parse --git-common-dir`). A release never has a fourth part.
#
#   tools/sysguard/heavy.sh tools/dev-build.sh                  Builds, and makes app/build.noindex/dev/RectoPDF-X.Y.Z.N.zip. Prints its path and how to install it.
#   tools/dev-build.sh --package <RectoPDF.app> [--out <folder>] [--version X.Y.Z.N]
#                                                               Only the last part, on an app already built: numbers, signs, checks and zips a copy. Seconds.
#   tools/sysguard/heavy.sh tools/dev-build.sh --first-install  Once, to start: builds, and puts it in /Applications as the owner's one RectoPDF (below).
#   … --first-install --replace-release                          The same where /Applications holds a release signed by the team, which never takes a dev
#                                                               build by itself: it's kept aside as .RectoPDF-update/previous/RectoPDF.kept.
#   tools/dev-build.sh --place <zip> [--replace-release]        Only --first-install's last part, on a zip already made.
#   tools/dev-build.sh --next-version                           The number the next build would have. --old-copy-pids: what --first-install would end. Both change nothing.
#
# Installing one needs Settings › Allow development builds on in the app, and agent access Full, as anyone can sign a build ad hoc:
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

usage() { sed -n '2,15p' "$0"; exit 2; }
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

# The one old copy --first-install may end: the owner's dev copy in the main checkout's target/dev, and nothing else. Not a quiet run, which is a test's.
COMMON="$(cd "$REPO" && git rev-parse --path-format=absolute --git-common-dir)"
OLD_COPY="${RECTO_DEV_OLD_COPY:-$(dirname "$COMMON")/target/dev/RectoPDF.app}"
INSTALLED="${RECTO_DEV_INSTALLED:-/Applications/RectoPDF.app}"
COUNTER="${RECTO_DEV_COUNTER:-$COMMON/recto-dev-counter}"
APPLICATIONS="${RECTO_DEV_APPLICATIONS:-/Applications}"
# The processes running exactly this program, and not as a quiet run.
running_as() {
  local pid command exe="$1"
  for pid in $(pgrep -x RectoPDF || true); do
    # -ww: without it, ps cuts a long command at 80 columns when it isn't writing to a terminal.
    command="$(ps -ww -o command= -p "$pid" || true)"
    case "$command" in *" --qa"*) continue ;; esac
    [ "$command" = "$exe" ] || [ "${command#"$exe "}" != "$command" ] || continue
    echo "$pid"
  done
}
# The team that signed an app, or nothing for ad hoc. codesign -dv reads the signature only: no keychain. RECTO_DEV_TEAM_OF stands in for a team in a test.
app_team() {
  if [ -n "${RECTO_DEV_TEAM_OF:-}" ]; then echo "$RECTO_DEV_TEAM_OF"; return; fi
  codesign -dv "$1" 2>&1 | sed -n 's/^TeamIdentifier=//p' | grep -v '^not set$' || true
}
# What --first-install may do where it's going: put the first in, or replace a release signed by the team when told to. Anything else is refused.
install_target() {
  local replace=$1 target="$APPLICATIONS/RectoPDF.app" team
  if [ ! -e "$target" ]; then
    [ "$replace" = 1 ] && fail "there's no release in $APPLICATIONS to replace: leave out --replace-release"
    echo fresh; return
  fi
  team="$(app_team "$target")"
  if [ -z "$team" ]; then fail "$target is a development build already. From here on: tools/dev-build.sh, then recto update install <zip>."; fi
  [ "$replace" = 1 ] || fail "$target is a release signed by team $team, which never takes a development build by itself. To put one in its place, add --replace-release: it's kept aside."
  echo replace
}
old_copy_running() {
  running_as "$OLD_COPY/Contents/MacOS/RectoPDF"
}

# The next development number for this version: one after the larger of what's installed and what's been counted, so worktrees never hand out the same one.
next_dev() {
  local version=$1 counted=0 last="" n installed
  { read -r last n < "$COUNTER"; } 2>/dev/null || true
  [ "$last" = "$version" ] && counted=${n:-0}
  installed="$(/usr/libexec/PlistBuddy -c 'Print :RectoDevVersion' "$INSTALLED/Contents/Info.plist" 2>/dev/null || true)"
  if [ "${installed%.*}" = "$version" ] && [[ "${installed##*.}" =~ ^[0-9]+$ ]] && [ "${installed##*.}" -gt "$counted" ]; then counted=${installed##*.}; fi
  echo "$version.$((counted + 1))"
}

mode=build built="" out="$OUT" dev="" replace=0 zip=""
while [ $# -gt 0 ]; do
  case "$1" in
    --package) mode=package; built="${2:?--package needs the built app}"; shift ;;
    --out) out="${2:?--out needs a folder}"; shift ;;
    --version) dev="${2:?--version needs X.Y.Z.N}"; shift ;;
    --first-install) mode=first ;;
    --replace-release) replace=1 ;;
    --place) mode=place; zip="${2:?--place needs a zip}"; shift ;;
    --next-version) echo "$(next_dev "$(release_version)")"; exit 0 ;;
    --old-copy-pids) old_copy_running; exit 0 ;;
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

# place <zip> <fresh|replace>: the first dev build into Applications. A copy running is swapped only after 5 minutes with no use of the Mac (owner, 2026-11-11),
# then told to end with SIGTERM: the old dev copy in the main checkout's target/dev, and with --replace-release the release being replaced, and nothing else.
# A copy built from `build/update-gaps` on takes SIGTERM as a quit, writing everything as any quit does. An older one, such as the owner's copy at 11276a3,
# just ends, and the 5 idle minutes are what cover it: by then everything's on disk, edits within 5 s and the place within a minute (SPEC decision 25).
place() {
  local zip=$1 how=$2 target="$APPLICATIONS/RectoPDF.app" was_running="" pid unpacked kept
  for pid in $(old_copy_running) $([ "$how" = replace ] && running_as "$target/Contents/MacOS/RectoPDF"); do
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
  ditto -x -k "$zip" "$unpacked"
  if [ "$how" = replace ]; then
    # Kept where the updater keeps the way back, private, and not as an app, so nothing registers it.
    kept="$APPLICATIONS/.RectoPDF-update/previous/RectoPDF.kept"
    mkdir -p "$(dirname "$kept")"
    chmod 700 "$APPLICATIONS/.RectoPDF-update"
    rm -rf "$kept"
    mv "$target" "$kept"
    echo "==> the release is kept aside at $kept"
  fi
  ditto "$unpacked/RectoPDF.app" "$target"
  rm -rf "$unpacked"
  echo "==> $target is $(/usr/libexec/PlistBuddy -c 'Print :RectoDevVersion' "$target/Contents/Info.plist"), the one install. The old dev copy is left where it was, for the coordinator to retire."
  # It was running, so it comes back, behind whatever's in front.
  [ -z "$was_running" ] || [ -n "${RECTO_DEV_NO_REOPEN:-}" ] || open -g "$target"
}

if [ "$mode" = place ]; then
  how="$(install_target "$replace")"
  place "$zip" "$how"
  exit 0
fi
if [ "$mode" = first ]; then
  # Before the build: a build nothing would take isn't worth making.
  how="$(install_target "$replace")"
fi

VERSION="$(release_version)"
mkdir -p "$OUT"
DEV="$(next_dev "$VERSION")"
COMMIT="$(git -C "$REPO" rev-parse --short HEAD)"
[ -z "$(git -C "$REPO" status --porcelain --untracked-files=no)" ] || COMMIT="$COMMIT-dirty"
echo "==> RectoPDF $DEV, a development build of $COMMIT"

# Release code in the faster profile where there is one, into a folder of its own, so the tests' app (a local build) is left as it is.
profile=--release
grep -q -- '--dev-release)' "$REPO/app/build.sh" && profile=--dev-release
(unset RECTO_SIGN_IDENTITY; RECTO_RELEASE=1 RECTO_APP_BUILD="$OUT/app" "$REPO/app/build.sh" "$profile")
ZIP="$(package "$OUT/app/RectoPDF.app" "$OUT" "$DEV")"
# Counted only once it's made: a failed build doesn't use up a number.
echo "$VERSION ${DEV##*.}" > "$COUNTER"

if [ "$mode" = first ]; then
  place "$ZIP" "$how"
  exit 0
fi
echo "$ZIP"
echo "install: recto update install \"$ZIP\"    (add --now to go at once, not after 5 minutes without use)"
