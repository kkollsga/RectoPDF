#!/usr/bin/env bash
# Draws the same pages with every layout through `recto render`, and compares the PNGs' bytes with the plain worker's. Wrap in tools/sysguard/heavy.sh.
#
#   tools/sysguard/heavy.sh spikes/sandbox/compare_pixels.sh <layouts> <out> <fonts.pdf>
set -euo pipefail
layouts="$1"; out="$2"; fonts="$3"
corpus=/Volumes/EksternalHome/Koding/Rust/distillPDF/benchmarking/bench100
state="$out/state"
mkdir -p "$state"; chmod 700 "$state"
printf '{"level":"full","set_at_ms":0}' > "$state/level"; chmod 600 "$state/level"
export RECTO_STATE_DIR="$state"
recto_of() {
  case "$1" in
    plain|selfbox) echo "$layouts/$1/recto" ;;
    *) echo "$layouts/$1/Spike.app/Contents/MacOS/recto" ;;
  esac
}
differ=0; same=0
while read -r file page; do
  name="$(basename "$file" .pdf)-$page"
  rm -f "$out/plain-$name.png"
  "$(recto_of plain)" local "$file" render --page "$page" --out "$out/plain-$name.png" > /dev/null
  for variant in entitled dropmach selfbox; do
    rm -f "$out/$variant-$name.png"
    "$(recto_of "$variant")" local "$file" render --page "$page" --out "$out/$variant-$name.png" > /dev/null
    if cmp -s "$out/plain-$name.png" "$out/$variant-$name.png"; then same=$((same + 1)); else differ=$((differ + 1)); echo "DIFFERS: $variant $name"; fi
  done
done <<LIST
$fonts 1
$fonts 2
$fonts 3
$corpus/standards_ietf_rfc9110.pdf 10
$corpus/standards_ietf_rfc9110.pdf 110
$corpus/gov_usgs_usgs70277338.pdf 1
$corpus/gov_nasa_ntrs20240002628.pdf 50
$corpus/gov_nasa_ntrs20220005969.pdf 175
$corpus/cs_CV_2607.28624.pdf 1
$corpus/cs_CV_2607.28624.pdf 3
$corpus/stat_ML_2607.28567.pdf 2
$corpus/quant-ph_2607.28621.pdf 1
LIST
echo "same $same, differ $differ"
