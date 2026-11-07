#!/usr/bin/env bash
# Real screenshots of every state, light and dark, drawn inside the app in quiet runs. Nothing reaches a screen.
#
#   app/qa/shots.sh OUT_DIR READER.pdf HEAVY.pdf [more PDFs for tabs...]
#
# OUT_DIR gets home, reader, sidebar-open, sidebar-folded, sidebar-hover, focus, heavy and low-res, each -light and -dark with the paper that goes with the window, and dark-white-paper and light-dark-paper.
# Given more PDFs, the first of them onward are other tabs for sidebar-long-contents.
set -euo pipefail
out="$1"; reader="$2"; heavy="$3"; shift 3
recto="$(cd "$(dirname "$0")/../.." && pwd)/target/debug/recto"
[ -n "${RECTO_BIN:-}" ] && recto="$RECTO_BIN"
mkdir -p "$out"
root="$(mktemp -d)"
trap 'rm -rf "${root:?}"' EXIT
export RECTO_STATE_DIR="$root/s"
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
# A shot is never written over anything, so what an earlier run left goes first.
shot() { rm -f "$out/$1.png"; r qa shot --out "$out/$1.png"; }

r qa start --time-limit 5m
for look in light dark; do
    r qa appearance "$look"
    shot "home-$look"
done
r open "$reader"
for extra in "$@"; do r open "$extra"; done
r tap "#1"
settle
for look in light dark; do
    r qa appearance "$look"
    r view --paper auto
    settle
    r goto 12
    settle
    shot "reader-$look"
    r qa hover 0
    shot "sidebar-hover-$look"
    r qa hover none
    r qa click-row 0
    settle
    shot "sidebar-folded-$look"
    r qa click-row 0
    settle
    shot "sidebar-open-$look"
    r view --focus on
    settle
    shot "focus-$look"
    r view --focus off
    settle
done
r qa appearance dark
r view --paper white
settle
shot dark-white-paper
r qa appearance light
r view --paper dark
settle
shot light-dark-paper
r view --paper auto
# A long outline with other tabs, and at a short window. The contents scroll by themselves, and the tabs and footer stay. Tab 1 is active by now.
for look in light dark; do
    r qa appearance "$look"
    settle
    r goto 90
    settle
    # Long enough for a drift to end. Time is the scenario.
    sleep 1.5
    shot "sidebar-long-contents-$look"
    r qa resize 1100x460
    settle
    shot "sidebar-long-contents-short-window-$look"
    r qa resize 1440x900
    settle
done
r stop
sleep 1

# The same with one tab, a shortened name revealed, and warm paper on a text page.
export RECTO_STATE_DIR="$root/o"
r qa start --time-limit 5m
r open "$reader"
settle
for look in light dark; do
    r qa appearance "$look"
    r goto 90
    settle
    sleep 1.5
    shot "sidebar-long-contents-one-tab-$look"
    r goto 1
    settle
    sleep 1.5
    # Row 2 is the second contents row, a long title.
    r qa reveal 2
    shot "reveal-long-name-$look"
    r qa reveal none
    r view --paper warm
    settle
    shot "warm-paper-text-$look"
    r view --paper auto
    settle
done
r stop
sleep 1

export RECTO_STATE_DIR="$root/h"
r qa start --time-limit 5m
r open "$heavy"
for look in light dark; do
    r qa appearance "$look"
    settle
    shot "heavy-$look"
    r view --paper warm
    settle
    shot "warm-paper-map-$look"
    r view --paper auto
    settle
done
r stop
sleep 1

# A limit under what this page needs, and the larger limit off, so it takes the path a page past 1 GiB takes.
export RECTO_STATE_DIR="$root/l"
r qa start --memory-limit-mib "${LOW_RES_LIMIT_MIB:-350}" --time-limit 5m
r open "$heavy"
for look in light dark; do
    r qa appearance "$look"
    settle
    shot "low-res-$look"
done
r stop
echo "shots are in $out"
