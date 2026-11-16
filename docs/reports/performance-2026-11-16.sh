#!/usr/bin/env bash
# The runs behind docs/reports/performance-2026-11-16.md, one part at a time, each through the build queue (tools/sysguard/heavy.sh) but
# `coldopen` (one small `recto` process at a time), with the load and the queue's state written beside it. On the release build:
# `tools/sysguard/heavy.sh app/build.sh --release` first. Every number in the report comes from one of these parts' output files.
#
#   docs/reports/performance-2026-11-16.sh OUT_DIR PART
#
# PARTs:
#   a          recto qa budget --file rfc9110 --heavy NASA ntrs20240002628 --runs 11 (the old report's run A)
#   b          recto qa budget --file <made 2 GiB, 10,000 pages> --heavy USGS 70277338 --runs 11 (run B)
#   b-again    qa budget --file <made> --heavy rfc9110 --runs 5: run B's hand-off and starts again, the other stages short
#   tabs       recto qa budget --file rfc9110 --heavy USGS 70277338 --runs 1, for more readings of 1,000 tabs of the map and the map's zoom
#   colour     a quiet run on each colour page, at rest and 4 s later: NASA ntrs20240002628 p50, NASA ntrs20220005969 p175
#   wake       5 wakes of a sleeping tab: rfc9110 p40 put to sleep with `session sleep`, NASA showing, then `tap`; `last_wake` read
#   sweeps     six `qa sweep 6 --for 5s` in a row on rfc9110 from page 64, from rest
#   closed     the idle app, window open then closed: `top` over 20 s for CPU and idle wake-ups, `qa state` for memory, threads, handles
#   coldopen   `recto local F facts` on fresh APFS clones of the made file (not in the cache), then the same clone again, with a `sample` of one
#   index      `recto budget index` on rfc9110, NASA and the made file: Find over the encrypted index against `--scan`
#   probe-restore   a start from the record with the made file (an edit and none) and rfc9110 (an edit): page drawn, apart from animations
#   probe-handoff   the same after an update's hand-off (`qa handoff now`), with `qa scroll 120` and an edit, as `qa budget` does it
#   probe-signatures   that hand-off on the made file with an edit, the app's memory through the look at its signatures, sampled
#   probe-made      the cold-start probe on the made file (its first start is the file out of the cache only if it has left it)
#   probe-cold, probe-sidebar   what qa budget's cold start and sidebar times are made of (performance-2026-11-16-probe.py)
#   rust       the release measurements of recto-app's headless kit: input_to_frame, wake_costs::waking_an_idle_tab, flick_cost
#
# MADE is the made file (docs/reports/performance-2026-11-16-bigpdf.py writes it), and BENCH a folder of copies of the bench100 files, both in a temp folder.
set -euo pipefail
out="$1"; part="$2"
here="$(cd "$(dirname "$0")/../.." && pwd)"
recto="$here/target/release/recto"
app="$here/app/build.noindex/RectoPDF.app/Contents/MacOS/RectoPDF"
# A quiet run opens only files in the system's temp folders, so BENCH is a copy there of the four bench100 files named below.
bench="${BENCH:?set BENCH to a temp folder holding copies of the bench100 files used here}"
made="${MADE:?set MADE to the made 2 GiB file}"
rfc="$bench/standards_ietf_rfc9110.pdf"
nasa="$bench/gov_nasa_ntrs20240002628.pdf"
shaded="$bench/gov_nasa_ntrs20220005969.pdf"
map="$bench/gov_usgs_usgs70277338.pdf"
heavy="$here/tools/sysguard/heavy.sh"
mkdir -p "$out"

stamp() { { echo "== $1 $(date '+%Y-%m-%d %H:%M:%S')"; uptime; "$heavy" --status | head -6; } >> "$out/load.txt"; }
queued() { "$heavy" "$@"; }

quiet_begin() {
    root="$(mktemp -d)"
    mkdir -p "$root/in"
    export RECTO_QA_OWNER_PID=$$ RECTO_STATE_DIR="$root/s"
    trap '"$recto" stop >/dev/null 2>&1 || true; rm -rf "${root:?}"' EXIT
    "$recto" qa start --app "$app" --time-limit 10m >/dev/null
}
inside() { local copy="$root/in/$(basename "$1")"; [ -e "$copy" ] || cp -c "$1" "$copy"; printf '%s\n' "$copy"; }
r() { "$recto" "$@" >/dev/null; }
settle() { r qa wait-settled --timeout 60s; }
state() { "$recto" qa state --json; }

case "$part" in
a)
    stamp a
    queued "$recto" qa budget --app "$app" --file "$rfc" --heavy "$nasa" --runs 11 --json > "$out/a.json"
    stamp a-end ;;
b)
    stamp b
    queued "$recto" qa budget --app "$app" --file "$made" --heavy "$map" --runs 11 --json > "$out/b.json"
    stamp b-end ;;
b-again)
    # Run B's hand-off on the made file again, with rfc9110 as the heavy file so the other stages are short.
    stamp b-again
    queued "$recto" qa budget --app "$app" --file "$made" --heavy "$rfc" --runs 5 --json > "$out/b-again.json"
    stamp b-again-end ;;
tabs)
    for n in 1 2; do
        stamp "tabs-$n"
        queued "$recto" qa budget --app "$app" --file "$rfc" --heavy "$map" --runs 1 --json > "$out/tabs-$n.json"
    done
    stamp tabs-end ;;
colour|wake|sweeps|closed)
    # One quiet run through the queue: this script calls itself inside it.
    stamp "$part"
    queued "$0" "$out" "inner-$part"
    stamp "$part-end" ;;
inner-colour)
    quiet_begin
    for pair in "$nasa:50" "$shaded:175"; do
        file="${pair%:*}"; page="${pair##*:}"; name="$(basename "$file" .pdf)"
        r open "$(inside "$file")"; settle
        r goto "$page"; settle
        state > "$out/colour-$name-p$page-settled.json"
        sleep 4
        state > "$out/colour-$name-p$page-4s.json"
    done ;;
inner-wake)
    quiet_begin
    r open "$(inside "$rfc")"; settle; r goto 40; settle
    r open "$(inside "$nasa")"; settle
    for n in 1 2 3 4 5; do
        r tap "#2"; settle
        r session sleep --tab "#1"; settle
        sleep 1
        r tap "#1"; settle
        state > "$out/wake-$n.json"
    done ;;
inner-sweeps)
    quiet_begin
    r open "$(inside "$rfc")"; settle
    for n in 1 2 3 4 5 6; do
        r goto 64; settle; sleep 1.5
        "$recto" qa sweep 6 --for 5s --json > "$out/sweep-$n.json"
    done ;;
inner-closed)
    quiet_begin
    r open "$(inside "$rfc")"; settle
    pid="$(pgrep -n -f "$app")"
    echo "$pid" > "$out/closed-pid.txt"
    sleep 5
    state > "$out/closed-open-state.json"
    top -l 2 -s 20 -pid "$pid" -stats pid,cpu,idlew,th,ports,mem > "$out/closed-open-top.txt"
    r qa window close
    sleep 5
    state > "$out/closed-state.json"
    top -l 2 -s 20 -pid "$pid" -stats pid,cpu,idlew,th,ports,mem > "$out/closed-top.txt"
    state > "$out/closed-state-after.json" ;;
coldopen)
    # A fresh clone has no pages in the file cache (as the old report's "cold"): each of three, then the same clone again, warm.
    stamp coldopen
    dir="$(dirname "$made")"
    for n in 1 2 3; do
        clone="$dir/cold-$n-$$.pdf"; cp -c "$made" "$clone"
        if [ "$n" = 1 ]; then
            # The first, sampled for its whole open (1 ms apart) from its own pid.
            "$recto" local "$clone" facts --json > "$out/coldopen-$n.json" &
            sample $! 2 1 -mayDie -file "$out/coldopen-sample.txt" >/dev/null 2>&1 || true
            wait
            rm -f "$clone"; clone="$dir/cold-$n-b-$$.pdf"; cp -c "$made" "$clone"
        fi
        /usr/bin/time -p "$recto" local "$clone" facts --json > "$out/coldopen-$n.json" 2> "$out/coldopen-$n-time.txt"
        /usr/bin/time -p "$recto" local "$clone" facts --json > /dev/null 2> "$out/coldopen-$n-warm-time.txt"
        rm -f "$clone"
    done
    stamp coldopen-end ;;
index)
    # `budget index` needs Control: a state folder of its own at Full, as `qa start` leaves one.
    export RECTO_STATE_DIR="$(mktemp -d)/s"
    "$recto" qa start --app "$app" >/dev/null; "$recto" qa quit now >/dev/null || true
    for f in "$rfc" "$nasa" "$made"; do
        stamp "index-$(basename "$f")"
        queued "$recto" budget index --file "$f" --json > "$out/index-$(basename "$f" .pdf).json"
    done ;;
probe-cold)
    stamp probe-cold
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" coldstart "$recto" "$app" "$rfc" 11 "$out/probe-cold.json"
    stamp probe-cold-end ;;
probe-sidebar)
    stamp probe-sidebar
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" sidebar "$recto" "$app" "$rfc" 5 "$out/probe-sidebar.json"
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" sidebar "$recto" "$app" "$rfc" 5 "$out/probe-sidebar-1000.json" 1000
    stamp probe-sidebar-end ;;
probe-restore)
    stamp probe-restore
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" restore "$recto" "$app" "$made" 3 "$out/probe-restore-made-edit.json" edit "$out/probe-restore-made-edit-sample.txt"
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" restore "$recto" "$app" "$made" 3 "$out/probe-restore-made-plain.json" plain
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" restore "$recto" "$app" "$rfc" 3 "$out/probe-restore-rfc-edit.json" edit
    stamp probe-restore-end ;;
probe-handoff)
    stamp probe-handoff
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" restore "$recto" "$app" "$made" 3 "$out/probe-handoff-made-edit.json" handoff-edit "$out/probe-handoff-made-edit-sample.txt"
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" restore "$recto" "$app" "$rfc" 3 "$out/probe-handoff-rfc-edit.json" handoff-edit
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" restore "$recto" "$app" "$made" 3 "$out/probe-handoff-made-plain.json" handoff-plain
    stamp probe-handoff-end ;;
probe-signatures)
    # The look at an edited file's signatures after a hand-off, with the app's footprint and threads at each half-second round, and a `sample` of the app.
    stamp probe-signatures
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" restore "$recto" "$app" "$made" 2 "$out/probe-signatures.json" handoff-edit "$out/probe-signatures-sample.txt"
    stamp probe-signatures-end ;;
probe-made)
    stamp probe-made
    queued /usr/bin/python3 -I "$here/docs/reports/performance-2026-11-16-probe.py" coldstart "$recto" "$app" "$made" 11 "$out/probe-cold-made.json"
    stamp probe-made-end ;;
rust)
    stamp rust
    ( cd "$here" && queued cargo nextest run --release --locked -p recto-app -p recto-render-service --features recto-app/testkit --run-ignored only --no-capture \
        -E 'test(=core::headless::input_to_frame::input_to_frame) | test(=core::headless::wake_costs::waking_an_idle_tab) | test(=core::headless::flick_cost::flicking_through_ten_large_tabs)' \
        > "$out/rust.txt" 2>&1 )
    stamp rust-end ;;
*) sed -n '2,20p' "$0"; exit 2 ;;
esac
