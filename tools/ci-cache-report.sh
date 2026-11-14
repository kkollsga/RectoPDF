#!/usr/bin/env bash
# ci-cache-report: what the CI Rust cache holds, as sizes. Read-only, builds nothing, a few seconds; every listing is cut at 30 lines.
#
#   tools/ci-cache-report.sh <label>      (run from the repository root; CI prints it after the restore and before the save)
#
# Why (2026-11-14): the cache is 5.8 GB and restoring it takes 118 to 214 s a run; this says what the bytes are, so a change to what is saved is
# judged by numbers read from CI's own log (lines start with "cache-report"). Paths are the ones the cache steps in .github/workflows/ci.yml list.
set -uo pipefail
export LC_ALL=C
label="${1:-now}"
started=$(date +%s)
top() { # du of the given paths, biggest first, 30 lines, in MiB
  du -sk "$@" 2>/dev/null | sort -rn | head -30 | awk '{printf "cache-report   %9.1f MiB  %s\n", $1/1024, $2}'
}
echo "cache-report == $label"
echo "cache-report total, KiB: $(du -sk target "$HOME/.cargo/registry" "$HOME/.cargo/git" 2>/dev/null | awk '{printf "%s=%s ", $2, $1}')"
echo "cache-report -- target/*"
top target/* 
echo "cache-report -- target/*/* (top 30)"
top target/*/*
for deps in target/*/deps; do
  [ -d "$deps" ] || continue
  # By kind: rlib/rmeta are libraries, a file with no extension and a hash is a test or build executable, dSYM is the packed debug info beside it.
  find "$deps" -maxdepth 1 -mindepth 1 \( -type f -o -type d \) -print0 2>/dev/null | xargs -0 du -sk 2>/dev/null | awk -v d="$deps" '
    { name=$2; sub(/.*\//,"",name); kind="other"
      if (name ~ /\.rlib$/) kind="rlib"; else if (name ~ /\.rmeta$/) kind="rmeta"; else if (name ~ /\.dSYM$/) kind="dSYM"
      else if (name ~ /\.(d|o|a)$/) kind="d-o-a"; else if (name ~ /\.(dylib|so)$/) kind="dylib"; else if (name !~ /\./) kind="executable"
      n[kind]++; k[kind]+=$1 }
    END { for (kind in n) printf "cache-report   %s  %-10s %6d entries %9.1f MiB\n", d, kind, n[kind], k[kind]/1024 }' | sort
done
echo "cache-report -- other cached paths"
top target/debug/incremental target/debug/build target/debug/.fingerprint target/nextest target/recto-evidence target/pdfium "$HOME/.cargo/registry" "$HOME/.cargo/git"
echo "cache-report $label took $(($(date +%s) - started)) s"
exit 0
