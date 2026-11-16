#!/usr/bin/env bash
# The coordinator's run of the performance review's missed and close lines with nothing else running (HEAVY_SOLO=1),
# on the review's own release build in .worktrees/performance-review (main at 50bd47d5).
set -uo pipefail
wt=/Volumes/EksternalHome/Koding/Rust/PDF-reader/.worktrees/performance-review
src=/Volumes/EksternalHome/Koding/Rust/distillPDF/benchmarking/bench100
out="$1"
tmp="$(mktemp -d)"
trap 'rm -rf "${tmp:?}"' EXIT
mkdir -p "$tmp/bench" "$out"
for f in standards_ietf_rfc9110.pdf gov_nasa_ntrs20240002628.pdf gov_nasa_ntrs20220005969.pdf gov_usgs_usgs70277338.pdf; do
    cp -c "$src/$f" "$tmp/bench/$f" 2>/dev/null || cp "$src/$f" "$tmp/bench/$f"
done
export BENCH="$tmp/bench" MADE="$tmp/made.pdf" HEAVY_SOLO=1
python3 -I "$wt/docs/reports/performance-2026-11-16-bigpdf.py" "$MADE" > "$out/made.txt" 2>&1
cd "$wt"
for part in probe-cold probe-made coldopen colour wake a tabs probe-signatures rust; do
    echo "== $part $(date '+%H:%M:%S') $(uptime | sed 's/.*load/load/')" >> "$out/parts.txt"
    docs/reports/performance-2026-11-16.sh "$out" "$part" > "$out/$part.stdout.txt" 2>&1
    echo "   exit $? $(date '+%H:%M:%S')" >> "$out/parts.txt"
done
echo done >> "$out/parts.txt"
