#!/usr/bin/env bash
# ratchet: structure that may only shrink. Sizes of files, functions, structs, enums, impls, types (all the inherent impl blocks of one name in a crate,
# wherever their files are: budget 60 methods, RATCHET_TYPE_METHODS) and the binding crate, fixed sleeps in tests, and the
# number of tests that launch the real app,
# against tools/ratchet/baseline.txt. Pure python3, reads the tree, builds nothing, a few seconds.
#
#   tools/ratchet.sh                 check (what check.sh runs first)
#   tools/ratchet.sh --update        rewrite the baseline downward after something shrank (refuses to raise any number)
#   tools/ratchet.sh --report <ref>  hub files this branch touched since <ref>, and lines added (report only)
#   tools/ratchet.sh --moved <old> <new>  code moved unchanged: rename its baseline entries, a file's path (all under it) or one path::Item (a `type` entry is per crate, not per file, so a move leaves it; rename it by hand if the type or crate is renamed);
#                                    refused if the old one still holds the offender (a copy) or a number at the new place is higher. Never raises one
#     --kind K                       with --moved: only that kind's entries (sleep, file, fn, ...), as when a file's sleeps go to one new file and its functions to others
#   --root DIR / --baseline FILE     other tree or baseline (the tool's own test uses these)
exec python3 -I "$(dirname "$0")/ratchet/ratchet.py" "$@"
