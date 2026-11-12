#!/usr/bin/env bash
# target-hygiene: keeps a target folder's `deps` from filling with object files. Seconds, builds nothing.
#
#   tools/target-hygiene.sh [target-dir]       (default: $CARGO_TARGET_DIR or ./target)
#
# Why (2026-11-12): the first IOSurface a process makes lists its executable's folder (CoreFoundation looking for .lproj folders), and every test is a process
# of its own, with its binary in target/<profile>/deps. The main checkout's had 408,000 entries, 404,000 of them `.o` files that cargo leaves there on macOS
# after every link (one per codegen unit, under a new hash per rebuild, never removed). About 2.2 microseconds an entry, so ~0.9 s a test alone, and 10 s each when
# a hundred tests start at once: 84 tests, and the engine tests with their bounds, failed or crawled. `[profile.dev] split-debuginfo = "off"` stops dev builds
# leaving them; this removes what is already there, and what other profiles leave. Only `*.o` in `deps` go: cargo's fingerprints don't depend on them
# (checked: a rebuild after deleting them is fresh), and the debug info they'd serve is line tables only.
# HYGIENE_MAX_ENTRIES (default 30000) is the size of a `deps` folder past which they go.
set -uo pipefail
target="${1:-${CARGO_TARGET_DIR:-target}}"
max="${HYGIENE_MAX_ENTRIES:-30000}"
[ -d "$target" ] || exit 0
absolute=$(cd "$target" && pwd)
# Never under a build: rustc is run with `--out-dir <this target>/<profile>/deps`, so a process naming the folder is a build in it.
if ps -axo command | grep -v grep | grep -e "--out-dir $absolute/" -e "$absolute/[a-z-]*/deps/.* --test" >/dev/null; then
  echo "target-hygiene: a build is running in $absolute, so nothing was swept."
  exit 0
fi
for deps in "$absolute"/*/deps; do
  [ -d "$deps" ] || continue
  entries=$(find "$deps" -maxdepth 1 -mindepth 1 | wc -l | tr -d ' ')
  if [ "$entries" -le "$max" ]; then
    echo "target-hygiene: $deps holds $entries entries, $(du -sk "$deps" | awk '{printf "%.1f", $1/1048576}') GiB (limit $max)."
    continue
  fi
  started=$(date +%s)
  objects=$(find "$deps" -maxdepth 1 -type f -name '*.o' | wc -l | tr -d ' ')
  find "$deps" -maxdepth 1 -type f -name '*.o' -delete
  echo "target-hygiene: $deps held $entries entries (over $max), $objects of them object files; removed those in $(($(date +%s) - started)) s, and holds $(find "$deps" -maxdepth 1 -mindepth 1 | wc -l | tr -d ' ') entries, $(du -sk "$deps" | awk '{printf "%.1f", $1/1048576}') GiB now. Each test is a process whose first IOSurface lists that folder."
done
exit 0
