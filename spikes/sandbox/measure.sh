#!/usr/bin/env bash
# Measures every layout from bundle.sh with the production `recto budget` commands, taking turns so a noisy machine hits them all alike, and prints one line per number with the median.
# Wrap in tools/sysguard/heavy.sh. Temp state only: the level file below is written the way the CLI's own tests do.
#
#   tools/sysguard/heavy.sh spikes/sandbox/measure.sh <layouts> <raw-output-folder> [rounds]
set -euo pipefail
layouts="$1"; raw="$2"; rounds="${3:-5}"
corpus=/Volumes/EksternalHome/Koding/Rust/distillPDF/benchmarking/bench100
rfc="$corpus/standards_ietf_rfc9110.pdf"
usgs="$corpus/gov_usgs_usgs70277338.pdf"
bomb=/Volumes/EksternalHome/Koding/Rust/distillPDF/tests/fixtures_pdf/adversarial/form_bomb.pdf
state="$raw/state"
mkdir -p "$state"; chmod 700 "$state"
printf '{"level":"full","set_at_ms":0}' > "$state/level"; chmod 600 "$state/level"
export RECTO_STATE_DIR="$state"
recto_of() {
  case "$1" in
    plain|selfbox) echo "$layouts/$1/recto" ;;
    *) echo "$layouts/$1/Spike.app/Contents/MacOS/recto" ;;
  esac
}
for round in $(seq 1 "$rounds"); do
  for variant in plain entitled dropmach selfbox; do
    r="$(recto_of "$variant")"
    "$r" local "$rfc" budget render --count 200 --samples --retire-mib 4000 --limit-mib 4096 --json > "$raw/$variant.$round.pages.json"
    "$r" local "$rfc" budget render --count 40 --fresh --json > "$raw/$variant.$round.fresh.json"
    "$r" local "$usgs" budget viewport --viewport 3024x1964 --json > "$raw/$variant.$round.usgs.json"
    start=$(python3 -I -c 'import time; print(time.time())')
    "$r" local "$bomb" budget render --count 1 --json > "$raw/$variant.$round.bomb.json"
    python3 -I -c "import time; print(round((time.time()-$start)*1000))" > "$raw/$variant.$round.bomb.ms"
  done
done
