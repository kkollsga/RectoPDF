#!/usr/bin/env bash
# Every path and name the how-to pages name in backticks exists (tools/doc-paths.py says how it reads them). With no argument, the pages in
# DOC_PATH_FILES (tools/check-scope.sh). Run by tools/check.sh before anything is queued, in --quick and --full: reads the tree, builds nothing.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ "$#" -eq 0 ]; then
  # shellcheck source=tools/check-scope.sh
  . tools/check-scope.sh
  # shellcheck disable=SC2086
  set -- $DOC_PATH_FILES
fi
exec python3 -I tools/doc-paths.py "$@"
