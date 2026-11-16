#!/usr/bin/env bash
# release/worker-env: a live render worker of this bundle was started with its allocator setting, on whatever signature the bundle has. Read-only.
#
#   tools/release/worker-env.sh <RectoPDF.app>
#
# Since 2026-11-16 a render worker is started with MallocLargeCache=0 in its otherwise empty environment
# (crates/recto-render-service/src/worker_env.rs), which took 100 to 175 MiB off heavy pages; every measurement of it was on ad hoc signed builds.
# Run by app/release-zip.sh's quiet run while it has a worker, so it holds on the local ad hoc build, the release's build job, the release
# preflight's team-signed app, and the release's stapled app after notarizing (--check-signed).
#   Required: every live worker of <app> (Contents/Helpers/RectoRender.app/Contents/MacOS/recto-render-service) has MallocLargeCache=0 in its
#             environment as `ps -E` reads it, and there is one.
#   Said:     what `vmmap --summary` lists as Malloc Large for the first, its cache of freed large blocks among them ("Malloc Large (empty)"),
#             or that it couldn't be read: vmmap may be refused a hardened process without the debugger's entitlement. A one-page smoke PDF
#             frees no large block, so "none listed" is what it says there; the environment line is the check.
# Exit 0 when it holds, 1 when it doesn't (with what was read), 2 on wrong use. Each read is under a limit (perl's alarm: there's no `timeout`).
set -euo pipefail
[ $# = 1 ] && [ -d "$1" ] || { sed -n '4p' "$0" >&2; exit 2; }
suffix=/Contents/Helpers/RectoRender.app/Contents/MacOS/recto-render-service
limit() { perl -e 'alarm shift; exec @ARGV' "$@"; }
real() { perl -MCwd=abs_path -e 'print abs_path(shift) // ""' "$1"; }
want=$(real "$1$suffix")
[ -n "$want" ] && [ -f "$want" ] || { echo "there's no render worker in $1 ($1$suffix)" >&2; exit 1; }
# Its processes: a command line that starts with a path ending in the worker's place in a bundle, which resolves to this bundle's worker (as the
# path is spelled however the app was started: /var or /private/var, a doubled slash), and never another checkout's.
pids=""
while read -r pid cmd; do
  case "$cmd" in *"$suffix"*) ;; *) continue ;; esac
  [ "$(real "${cmd%%"$suffix"*}$suffix")" = "$want" ] && pids="$pids $pid"
done < <(limit 10 ps -x -o pid=,command=)
[ -n "$pids" ] || { echo "no live render worker of $1 to read the environment of" >&2; exit 1; }
for pid in $pids; do
  env=$(limit 10 ps -E -p "$pid" -o command= 2>&1 || true)
  case " $env " in
    *" MallocLargeCache=0 "*) ;;
    *) echo "the render worker $pid wasn't started with MallocLargeCache=0 (crates/recto-render-service/src/worker_env.rs), or its environment can't be read here: ps -E says [$env]" >&2; exit 1 ;;
  esac
done
echo "    the render worker's environment has MallocLargeCache=0 (pid${pids})"
# shellcheck disable=SC2086 # the pids, one word each
set -- $pids
first=$1
if vm=$(limit 60 vmmap --summary "$first" 2>&1); then
  large=$(grep -E '^Malloc Large' <<< "$vm" | tr -s ' ' || true)
  echo "    vmmap --summary $first, Malloc Large: ${large:-none listed}"
else
  echo "    vmmap --summary $first couldn't be read: $(head -1 <<< "$vm")"
fi
