#!/bin/bash
# Deletes test executables in target/debug/deps that this workspace no longer builds, so dead generations don't pile up in CI's saved Rust cache
# (audit 2026-11-26: 993 test executables, 7.7 GB, for about 95 real test binaries).
#
# Kept: every binary `cargo nextest list` names (what the workspace builds now), anything with another hard link (cargo links a built bin or example
# into target/debug/), anything written since the marker file given by --newer-than (this job's start), and everything that is not a Mach-O file named
# `<crate>-<16 hex digits>` with no extension. So .d, .rlib, .rmeta, .dylib, build scripts and incremental/ are never touched.
# A wrong delete costs a relink: cargo finds the output missing and builds it again.
#
#   tools/ci-prune-test-binaries.sh [--dry-run] [--newer-than FILE] [DEPS_DIR]
set -euo pipefail
dry=0 marker="" deps="target/debug/deps"
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry=1 ;;
    --newer-than) marker="$2"; shift ;;
    *) deps="$1" ;;
  esac
  shift
done
command -v jq >/dev/null || { echo "ci-prune: jq is needed; nothing removed" >&2; exit 0; }
live=$(mktemp); trap 'rm -f "$live"' EXIT
# Fail closed: if the list can't be made, remove nothing.
if ! cargo nextest list --locked --workspace --message-format json 2>/dev/null \
    | jq -r '.["rust-suites"][]."binary-path"' >"$live" || [ ! -s "$live" ]; then
  echo "ci-prune: could not list the workspace's test binaries; nothing removed" >&2
  exit 0
fi
kept=0 removed=0 kept_bytes=0 removed_bytes=0
while IFS= read -r f; do
  name=${f##*/}
  [[ $name =~ ^[A-Za-z0-9_]+-[0-9a-f]{16}$ ]] || continue
  [ -f "$f" ] || continue
  [ "$(head -c4 "$f" | od -An -tx1 | tr -d ' ')" = "cffaedfe" ] || continue
  size=$(stat -f%z "$f")
  keep=0
  grep -qxF "$(cd "$(dirname "$f")" && pwd -P)/$name" "$live" && keep=1
  [ "$(stat -f%l "$f")" -gt 1 ] && keep=1
  if [ -n "$marker" ] && [ "$f" -nt "$marker" ]; then keep=1; fi
  if [ $keep = 1 ]; then kept=$((kept+1)); kept_bytes=$((kept_bytes+size)); continue; fi
  removed=$((removed+1)); removed_bytes=$((removed_bytes+size))
  if [ $dry = 1 ]; then echo "would remove $f ($size)"; else rm -f "$f"; fi
done < <(find "$deps" -maxdepth 1 -type f ! -name '*.*')
echo "ci-prune: $([ $dry = 1 ] && echo would\ remove || echo removed) $removed test executables, $((removed_bytes/1048576)) MiB; kept $kept, $((kept_bytes/1048576)) MiB"
