#!/usr/bin/env bash
# release/verdict: what an earlier release's own updater says of a newer release's latest.json and zip, by that release's own code.
#
#   tools/release/verdict.sh <tag> <latest.json> <zip> <unpacked folder> <running version> [<macOS version>]
#
# <unpacked folder> is where the earlier release's own `recto-updater check-zip` unpacked <zip> (check-draft.sh does that with the
# released binary). This builds tools/release/verdict/release-verdict.rs against <tag>'s own crates/recto-update, from `git archive <tag>`
# at its own Cargo.lock (--locked --offline), once per tag and program, into target/release-tools/<tag>/, through the build queue, and runs
# it: the feed as parse_latest reads it, whether it's on offer to <running version> on this Mac (or <macOS version>), the zip's size
# and checksum against the feed, and stage::verify_app on the unpacked app (bundle id, version, codesign --deep --strict, the team's
# Developer ID requirement, Gatekeeper). What it leaves out is the download itself, and where the app is (check_place).
# Prints the program's lines; exit 0 when it takes the release, 1 when it refuses, 2 on wrong use or a failed build.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
[ $# -ge 5 ] && [ $# -le 6 ] || { sed -n '2,3p' "$0" >&2; exit 2; }
tag="$1" feed="$2" zip="$3" unpacked="$4" running="$5" system="${6:-$(sw_vers -productVersion)}"
commit=$(git -C "$REPO" rev-parse --verify --quiet "refs/tags/$tag^{commit}") || { echo "verdict: there's no tag $tag here" >&2; exit 2; }
program="$REPO/tools/release/verdict/release-verdict.rs"
dir="$REPO/target/release-tools/$tag"
stamp="$commit $(shasum -a 256 "$program" | cut -d' ' -f1)"
bin="$dir/bin/release-verdict"
if [ ! -x "$bin" ] || [ "$(cat "$dir/stamp" 2>/dev/null)" != "$stamp" ]; then
  echo "verdict: building $tag's own updater code (once for this tag and program)" >&2
  rm -rf "$dir/src" "$dir/bin" "$dir/stamp"
  mkdir -p "$dir/src" "$dir/bin"
  git -C "$REPO" archive "$commit" | tar -x -C "$dir/src"
  cp "$program" "$dir/src/crates/recto-update/src/bin/release-verdict.rs"
  # The tag's own lockfile and toolchain file, run from inside its tree; nothing fetched.
  (cd "$dir/src" && CARGO_TARGET_DIR="$dir/target" "$REPO/tools/sysguard/heavy.sh" cargo build --locked --offline --quiet -p recto-update --bin release-verdict) >&2 \
    || { echo "verdict: $tag's code didn't build with the program (an API of $tag's that it doesn't fit?)" >&2; exit 2; }
  cp "$dir/target/debug/release-verdict" "$bin"
  echo "$stamp" > "$dir/stamp"
fi
"$bin" "$feed" "$zip" "$unpacked" "$running" "$system"
