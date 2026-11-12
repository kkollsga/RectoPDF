#!/usr/bin/env bash
# ratchet: structure that may only shrink. Sizes of files, functions, structs, enums, impls and the binding crate, and fixed sleeps in tests,
# against tools/ratchet/baseline.txt. Pure python3, reads the tree, builds nothing, a few seconds.
#
#   tools/ratchet.sh                 check (what check.sh runs first)
#   tools/ratchet.sh --update        rewrite the baseline downward after something shrank (refuses to raise any number)
#   tools/ratchet.sh --report <ref>  hub files this branch touched since <ref>, and lines added (report only)
#   --root DIR / --baseline FILE     other tree or baseline (the tool's own test uses these)
exec python3 -I "$(dirname "$0")/ratchet/ratchet.py" "$@"
