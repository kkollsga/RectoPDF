#!/usr/bin/env bash
# worktree: the way to start a worker's worktree, with a warm build folder.
#
#   tools/worktree.sh new <branch> <path> [<start>]   A worktree at <path> on <branch>, new from <start> (default main) unless it exists,
#                                                     its target/ seeded from the main checkout's
#   tools/worktree.sh seed <path>                     Seeds an existing worktree's target/ the same way, if it has none yet
#   tools/worktree.sh remove <path>                   Removes the worktree and its target/ (the branch stays)
#
# The seed is an APFS clone (`cp -c`): near-instant, and no disk is used until one side writes a file. Without APFS it seeds nothing,
# rather than copying tens of gigabytes. What it takes: every registry and git dependency already built (debug, release, dev-release, the Windows
# check), the build scripts' output and PDFium. PDFium goes first, and a file that can't be cloned (one a build is writing or removing as
# the seed runs) is said and skipped, so the worker's library is always there to draw with; with none in the main checkout, `tools/pdfium.sh`
# is named and the seed goes on. What it leaves: the workspace's own object files and incremental caches, which the
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
  # -p keeps the times: the seed must look older than the worktree's sources (above). One at a time, so a file that can't be cloned costs
  # only itself: it's said, and the build makes it again.
  find "$src" -mindepth 1 -maxdepth 1 "$@" -print0 | while IFS= read -r -d '' entry; do
    cp -c -p -R "$entry" "$dst/" || echo "worktree: couldn't clone $entry, so it's left for the build." >&2
  done
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
  probe=$(find "$from/debug" -maxdepth 1 -type f -perm -u+r | head -1)
  if [ -z "$probe" ] || ! cp -c "$probe" "$to/.probe" 2>/dev/null; then
    rm -rf "$to"
    echo "worktree: can't clone from $from into $to (not one APFS volume), so it isn't seeded. The first build is cold." >&2
    return 0
  fi
  rm -rf "$to/.probe"
  started=$(date +%s)
  # First, so nothing below can keep it out: without it no page can be read or drawn in this worktree.
  if [ -d "$from/pdfium" ]; then
    cp -c -p -R "$from/pdfium" "$to/" || echo "worktree: couldn't clone $from/pdfium: run tools/pdfium.sh here before any test that draws a page." >&2
  else
    echo "worktree: the main checkout has no PDFium at $from/pdfium to seed: run tools/pdfium.sh here before any test that draws a page." >&2
  fi
  for profile in "$from"/debug "$from"/release "$from"/dev-release "$from"/*/debug "$from"/*/release; do
    [ -d "$profile" ] || continue
    local rel=${profile#"$from"/}
    case "$rel" in tmp/* | dev*/*) continue ;; esac
    # Top level: binaries, rlibs and their dep files, but not the incremental caches.
    clone_dir "$profile" "$to/$rel" ! -name incremental ! -name deps ! -name examples ! -name '.cargo-*lock'
    # The workspace's object files, kept for line numbers in backtraces, are almost all the files and are rebuilt here anyway.
    [ -d "$profile/deps" ] && clone_dir "$profile/deps" "$to/$rel/deps" ! -name '*.o'
  done
  if [ -f "$from/CACHEDIR.TAG" ]; then cp -c -p "$from/CACHEDIR.TAG" "$to/" || true; fi
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
    path=$(cd "$2" && pwd)
    had_target=""
    [ ! -e "$path/target" ] || had_target=1
    seed "$path"
    # An existing worktree was checked out before the main checkout's last build, so its sources are older than the clone, and cargo
    # would take the seed's workspace crates as built from them: a worktree behind main then links main's newer crates and fails to
    # compile (2026-11-11, recto-ffi against a recto-update with a method it didn't have). Bringing the tracked files' times forward
    # makes the workspace rebuild, as in a new worktree.
    if [ -z "$had_target" ] && [ -d "$path/target" ]; then (cd "$path" && git ls-files -z | xargs -0 touch -c); fi
    ;;
  remove)
    [ $# -eq 2 ] || usage
    path=$(cd "$2" && pwd)
    # Launch Services keeps every app copy that was ever opened, and a deleted folder's copies stay in Open With as more RectoPDFs
    # (2026-11-11: three left by removed worktrees). Forget each bundle in the worktree, nested ones too, before it goes.
    lsregister=${WORKTREE_LSREGISTER:-/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister}
    if [ -x "$lsregister" ]; then
      find "$path" -path "$path/target" -prune -o -path "$path/.git" -prune -o -type d -name '*.app' -print 2>/dev/null |
        while IFS= read -r bundle; do "$lsregister" -u "$bundle" >/dev/null 2>&1 || true; done
    fi
    rm -rf "$path/target"
    git worktree remove "$path"
    ;;
  *) usage ;;
esac
