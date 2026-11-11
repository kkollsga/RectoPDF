#!/usr/bin/env bash
# Real screenshots of generated contents, light and dark, drawn inside the app in a quiet run: the offer for a file with none, the offer over contents that look bad, the list while it's made, the generated list labelled, and the file's own with the generated ones a click away.
#
#   app/qa/generated-shots.sh OUT_DIR MISSING.pdf BAD.pdf
#
# MISSING has no contents of its own, and BAD has some that look bad, by `recto contents`'s `verdict`.
set -euo pipefail
out="$1"; missing="$2"; bad="$3"
recto="$(cd "$(dirname "$0")/../.." && pwd)/target/debug/recto"
[ -n "${RECTO_BIN:-}" ] && recto="$RECTO_BIN"
mkdir -p "$out"
root="$(mktemp -d)"
trap '"$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
# A quiet run this starts ends with this script, however it ends.
export RECTO_QA_OWNER_PID=$$
export RECTO_STATE_DIR="$root/s"
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
shot() { rm -f "$out/$1.png"; r qa shot --out "$out/$1.png"; }
both() { for look in light dark; do r qa appearance "$look"; r view --paper auto; settle; shot "$1-$look"; done; r qa appearance light; }
made() { until "$recto" wait contents-generated --timeout 300s >/dev/null; do [ $? -eq 7 ] || exit 1; done; settle; }

r qa start --time-limit 10m
r qa scroller-style overlay
r open "$bad"
r open "$missing"
settle
both offer-missing
r qa click-row note
settle
shot generating-light
made
r goto 30
settle
both generated-missing
r tap "#1"
settle
both offer-bad
r qa click-row note
made
both generated-bad
r qa click-row note
settle
both own-with-generated-on-offer
"$recto" qa state --json > "$out/state.json"
