#!/usr/bin/env bash
# worktree: the way to start a worker's worktree, with a warm build folder.
#
#   tools/worktree.sh new <branch> <path> [<start>]   A worktree at <path> on <branch>, new from <start> (default main) unless it exists,
#                                                     its target/ seeded from the main checkout's
#   tools/worktree.sh seed <path>                     Seeds an existing worktree's target/ the same way, if it has none yet
#   tools/worktree.sh remove <path>                   Removes the worktree and its target/ (the branch stays)
#
# The seed is an APFS clone (`cp -c`): near-instant, and no disk is used until one side writes a file. Without APFS it seeds nothing,
# rather than copying tens of gigabytes. What it takes: every registry and git dependency already built (debug, release, the Windows
# check), the build scripts' output and PDFium. What it leaves: the workspace's own object files and incremental caches, which the
# worktree rebuilds anyway, as its sources are newer than anything in the seed. That is what keeps a seed correct: the clone keeps the
# main checkout's times, and the worktree's files are checked out after them, so cargo sees every workspace crate as changed and only
# the dependencies as built. Leaving out the object files and caches is most of the files and makes the clone a few seconds, not a minute.
set -euo pipefail
usage() { sed -n '4,6p' "$0"; exit 2; }
[ $# -ge 1 ] || usage
common=$(git rev-parse --path-format=absolute --git-common-dir)
main_checkout=$(dirname "$common")
from="${RECTO_SEED_TARGET:-$main_checkout/target}"

# Clones $1's entries into $2, except what the filter in $3 (a find expression) leaves out.
clone_dir() {
  local src=$1 dst=$2
  shift 2
  mkdir -p "$dst"
  # -p keeps the times: the seed must look older than the worktree's sources (above).
  find "$src" -mindepth 1 -maxdepth 1 "$@" -print0 | xargs -0 -J % cp -c -p -R % "$dst/"
}

seed() {
  local to=$1/target started
  if [ -e "$to" ]; then
    echo "worktree: $to already exists, so it's left as it is." >&2
    return 0
  fi
  if [ ! -d "$from/debug" ]; then
    echo "worktree: nothing to seed from at $from, so the first build is cold." >&2
    return 0
  fi
  mkdir -p "$to"
  # A clone works only within one APFS volume. Anything else would be a real copy of tens of gigabytes, so it seeds nothing.
  local probe
  probe=$(find "$from/debug" -maxdepth 1 -type f | head -1)
  if [ -z "$probe" ] || ! cp -c "$probe" "$to/.probe" 2>/dev/null; then
    rm -rf "$to"
    echo "worktree: can't clone from $from into $to (not one APFS volume), so it isn't seeded. The first build is cold." >&2
    return 0
  fi
  rm -rf "$to/.probe"
  started=$(date +%s)
  for profile in "$from"/debug "$from"/release "$from"/*/debug "$from"/*/release; do
    [ -d "$profile" ] || continue
    local rel=${profile#"$from"/}
    case "$rel" in tmp/* | dev*/*) continue ;; esac
    # Top level: binaries, rlibs and their dep files, but not the incremental caches.
    clone_dir "$profile" "$to/$rel" ! -name incremental ! -name deps ! -name examples ! -name '.cargo-*lock'
    # The workspace's object files, kept for line numbers in backtraces, are almost all the files and are rebuilt here anyway.
    [ -d "$profile/deps" ] && clone_dir "$profile/deps" "$to/$rel/deps" ! -name '*.o'
  done
  [ -d "$from/pdfium" ] && cp -c -p -R "$from/pdfium" "$to/"
  [ -f "$from/CACHEDIR.TAG" ] && cp -c -p "$from/CACHEDIR.TAG" "$to/"
  echo "worktree: seeded $to from $from in $(($(date +%s) - started)) s ($(find "$to" -type f | wc -l | tr -d ' ') files, cloned, so no disk until written)."
}

case "$1" in
  new)
    [ $# -ge 3 ] && [ $# -le 4 ] || usage
    branch=$2 path=$3 start=${4:-main}
    if git show-ref --verify -q "refs/heads/$branch"; then
      git worktree add "$path" "$branch"
    else
      # No upstream, even from origin/main: a worker's branch never pushes to main by accident.
      git worktree add --no-track -b "$branch" "$path" "$start"
    fi
    # After the checkout, never before: see the top.
    seed "$(cd "$path" && pwd)"
    ;;
  seed)
    [ $# -eq 2 ] || usage
    seed "$(cd "$2" && pwd)"
    ;;
  remove)
    [ $# -eq 2 ] || usage
    path=$(cd "$2" && pwd)
    rm -rf "$path/target"
    git worktree remove "$path"
    ;;
  *) usage ;;
esac
