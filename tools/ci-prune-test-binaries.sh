#!/bin/bash
# Deletes test executables in target/debug/deps that this workspace no longer builds, so dead generations don't pile up in CI's saved Rust cache
# (audit 2026-11-26: 993 test executables, 7.7 GB, for about 95 real test binaries).
#
# What makes it safe on CI: the checkout comes before the cache restore, so every workspace unit is rebuilt each run and its executables are written after
# the job-start marker (--newer-than). Those, and every binary `cargo nextest list` names, are kept. What is left is a generation no run builds any more.
# Also kept, as a harmless extra: a file with another hard link (cargo links some bins into target/debug/; on this project none has, 2026-11-26).
# Only a Mach-O regular file named `<crate>-<16 hex digits>` with no extension is a candidate, so .d, .rlib, .rmeta, .dylib, build scripts and
# incremental/ are never touched. Names are matched by basename (the volume is case-insensitive, so a directory may be spelled two ways).
# A wrong delete costs a relink. A failure of any kind removes nothing and exits 0: a prune must never skip the cache's save or turn main red.
#
#   tools/ci-prune-test-binaries.sh [--dry-run] [--newer-than FILE] [DEPS_DIR]
# PRUNE_JQ names the jq to use (a test sets it to something missing).
set -u
dry=0 marker="" deps="target/debug/deps" jq="${PRUNE_JQ:-jq}"
trap 'exit 0' EXIT
abort() { echo "ci-prune: $1; nothing removed" >&2; exit 0; }
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) dry=1 ;;
    --newer-than) marker="${2:-}"; shift ;;
    *) deps="$1" ;;
  esac
  shift
done
command -v "$jq" >/dev/null || abort "jq is missing"
[ -z "$marker" ] || [ -e "$marker" ] || abort "the marker $marker is missing"
[ -d "$deps" ] || abort "$deps is not a directory"
live=$(mktemp) listing=$(mktemp); trap 'rm -f "$live" "$listing"; exit 0' EXIT
start=$SECONDS
cargo nextest list --locked --workspace --message-format json >"$listing" || abort "cargo nextest list failed"
"$jq" -r '.["rust-suites"][]."binary-path"' <"$listing" >"$live" || abort "the list could not be read"
echo "ci-prune: the list took $((SECONDS-start)) s, $(wc -l <"$live" | tr -d ' ') binaries"
[ -s "$live" ] || abort "the list is empty"
while IFS= read -r p; do
  case "$p" in /*) [ -e "$p" ] || abort "listed path $p does not exist" ;; *) abort "listed path '$p' is not an absolute path" ;; esac
done <"$live"
sed 's|.*/||' "$live" | sort -u >"$live.names"
trap 'rm -f "$live" "$live.names" "$listing"; exit 0' EXIT
candidates=() matched=0
while IFS= read -r f; do
  name=${f##*/}
  [[ $name =~ ^[A-Za-z0-9_]+-[0-9a-f]{16}$ ]] || continue
  [ -f "$f" ] || continue
  [ "$(head -c4 "$f" | od -An -tx1 | tr -d ' ')" = "cffaedfe" ] || continue
  candidates+=("$f")
  grep -qxF "$name" "$live.names" && matched=$((matched+1))
done < <(find "$deps" -maxdepth 1 -type f ! -name '*.*')
[ "${#candidates[@]}" -eq 0 ] || [ "$matched" -gt 0 ] || abort "no candidate is in the list, so the list is not trusted"
kept=0 removed=0 kept_bytes=0 removed_bytes=0
for f in "${candidates[@]+"${candidates[@]}"}"; do
  name=${f##*/}
  size=$(stat -f%z "$f")
  keep=0
  grep -qxF "$name" "$live.names" && keep=1
  [ "$(stat -f%l "$f")" -gt 1 ] && keep=1
  # Kept unless older than the marker (find -newer: a file with the marker's own second is kept; bash's -nt is whole seconds).
  if [ -n "$marker" ] && [ -z "$(find "$marker" -newer "$f")" ]; then keep=1; fi
  if [ $keep = 1 ]; then kept=$((kept+1)); kept_bytes=$((kept_bytes+size)); continue; fi
  removed=$((removed+1)); removed_bytes=$((removed_bytes+size))
  if [ $dry = 1 ]; then echo "would remove $f ($size)"; else rm -f "$f"; fi
done
echo "ci-prune: $([ $dry = 1 ] && echo would\ remove || echo removed) $removed test executables, $((removed_bytes/1048576)) MiB; kept $kept, $((kept_bytes/1048576)) MiB"
