#!/usr/bin/env bash
# Phase 3 of docs/reports/build-performance-2026-11-11.md, ready but not applied: one integration test binary per crate.
#
#   docs/reports/build-performance-consolidate.sh <crate>...     Moves crates/<crate>/tests/*.rs into tests/it/ as modules of one binary
#
# Kept as binaries of their own: memory, scale and handle-count tests and anything with its own allocator (AGENTS.md: memory tests run in their own
# binary), named by KEEP, a regex on the file's name. Their helper module (`common`, `support`) moves with the rest and they reach it by path.
# What it fixes itself: the helper modules, `include_str!` paths out of tests/, CARGO_MANIFEST_DIR paths into a moved folder, and the tests that
# run their own binary with `--exact <name>`, whose names gain the module's. What's left by hand, the report's "Steps when it's applied": the
# `--test <file>` names in tools/check.sh and the docs, and the binaries named in .config/nextest.toml's priority, which nextest refuses to start without.
set -euo pipefail
cd "$(dirname "$0")/../.."
KEEP="${KEEP:-memory|scale|handles}"
for crate in "$@"; do
  dir=crates/$crate/tests
  [ -d "$dir" ] || { echo "no $dir" >&2; exit 1; }
  [ ! -e "$dir/it" ] || { echo "$dir/it already exists" >&2; exit 1; }
  mkdir -p "$dir/it"
  # Helper folders (common/, support/, app/, restore/, fixtures/) move with the files that use them.
  for helper in "$dir"/*/; do
    name=$(basename "$helper")
    case "$name" in it) continue ;; esac
    git mv "$helper" "$dir/it/$name"
    moved_helpers="${moved_helpers:-} $name"
  done
  modules=()
  for file in "$dir"/*.rs; do
    name=$(basename "$file" .rs)
    if printf '%s' "$name" | grep -Eq "$KEEP" || grep -q 'global_allocator' "$file"; then
      # Stays its own binary: its helper module is now under it/.
      sed -i '' -E 's|^mod (common\|support);|#[path = "it/\1/mod.rs"]\nmod \1;|' "$file"
      continue
    fi
    git mv "$file" "$dir/it/$name.rs"
    # The helper module is the binary's now, declared once in main.rs.
    sed -i '' -E 's/^mod (common|support);/#[allow(unused_imports)]\nuse crate::\1;/' "$dir/it/$name.rs"
    # A file one folder deeper reaches out of tests/ with one more `../` (session.rs and socket.rs read other crates' sources this way).
    sed -i '' -E 's#(include_(str|bytes)!\(")\.\./#\1../../#g' "$dir/it/$name.rs"
    # A test that runs its own binary as a stand-in worker names itself to the harness, and its name now starts with the module's.
    sed -i '' -E "s#(\"--exact\"\\)?, (OsStr::new\\()?\")([a-z0-9_]+)\"#\\1$name::\\3\"#g" "$dir/it/$name.rs"
    modules+=("$name")
  done
  # Paths built from CARGO_MANIFEST_DIR into a helper folder that moved (restore.rs reads tests/restore/known/ that way).
  for name in ${moved_helpers:-}; do
    { grep -rl "\"tests/$name" "$dir" 2>/dev/null || true; } | xargs -r sed -i '' "s|\"tests/$name|\"tests/it/$name|g"
    # And a kept binary's own include of one.
    for kept in "$dir"/*.rs; do
      if [ -f "$kept" ]; then sed -i '' -E "s#(include_(str|bytes)!\(\")$name/#\1it/$name/#g" "$kept"; fi
    done
  done
  moved_helpers=""
  {
    echo "//! Every integration test of $crate in one binary: one link instead of one per file (docs/reports/build-performance-2026-11-11.md)."
    for helper in "$dir"/it/*/; do
      name=$(basename "$helper")
      if [ -f "$helper/mod.rs" ]; then printf '#[allow(dead_code)]\nmod %s;\n' "$name"; fi
    done
    for name in "${modules[@]}"; do echo "mod $name;"; done
  } > "$dir/it/main.rs"
  git add "$dir/it/main.rs"
  echo "$crate: ${#modules[@]} files in tests/it, kept: $(cd "$dir" && ls *.rs 2>/dev/null | tr '\n' ' ')"
done
