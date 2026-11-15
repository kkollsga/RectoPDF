#!/usr/bin/env bash
# release/check-carry: the state a person has, carried from the previous release to the new build and back, in quiet runs on one temp
# state folder, with the real binaries: the previous release as published, the new one as built (ad hoc signed is fine; a quiet run
# doesn't check the team). tools/release/carry/check_carry.py says the six runs and what each compares.
#
#   tools/release/check-carry.sh <RectoPDF-P.zip as published> <RectoPDF.app built for the new version> [<evidence folder>]
#
# Quiet runs only: off every screen, no Dock icon, never active, never the keyboard; state, PDFs and both copies in a folder of the
# system's temp folder, removed at the end, the copies forgotten by Launch Services first. Evidence (each run's answers) goes to
# <evidence folder>, by default target/release-tools/check-carry/ here. What may differ across this pair, by design and each with its
# reason, is tools/release/carry/<P>-to-<X.Y.Z>.txt, beside the restore tests' own crates/recto-cli/tests/restore/exceptions/.
# Exit 0 when everything that should carry did; 1 with each difference named.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
[ $# -ge 2 ] && [ $# -le 3 ] && [ -f "$1" ] && [ -d "$2" ] || { sed -n '6p' "$0" >&2; exit 2; }
evidence="${3:-$REPO/target/release-tools/check-carry}"
rm -rf "$evidence"; mkdir -p "$evidence"
# The temp folder a quiet run may reach (0.1.1's file wall): the system's own, whatever this shell has set.
export TMPDIR="$(getconf DARWIN_USER_TEMP_DIR)"
exec python3 -I "$REPO/tools/release/carry/check_carry.py" "$REPO" "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")" "$(cd "$2" && pwd)" "$evidence"
