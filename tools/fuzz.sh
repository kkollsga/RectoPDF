#!/usr/bin/env bash
# fuzz: runs the libFuzzer targets in fuzz/, each under the shared heavy lock and released between them.
#
#   tools/fuzz.sh [--seconds N] [--work DIR] [target ...]
#
# Default: 300 seconds each, every target, keeping its corpus in target/fuzz-work. Run it again and it carries on from that corpus.
# A crash leaves its input in <work>/artifacts/<target>/. It needs nightly and cargo-fuzz, and neither is used by anything else here.
# FUZZ_TOOLCHAIN picks the nightly, FUZZ_SANITIZER a sanitizer (address for the code with `unsafe` around it; the default none is several times faster, and all of this is safe Rust).
set -euo pipefail
cd "$(dirname "$0")/.."
seconds=300
work=target/fuzz-work
targets=()
while [ $# -gt 0 ]; do
  case "$1" in
    --seconds) seconds="$2"; shift 2 ;;
    --work) work="$2"; shift 2 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) targets+=("$1"); shift ;;
  esac
done
[ ${#targets[@]} -gt 0 ] || targets=(pdf_bytes pdf_tree proto index agent control edits record)
toolchain="${FUZZ_TOOLCHAIN:-nightly}"
sanitizer="${FUZZ_SANITIZER:-none}"
mkdir -p "$work"
work="$(cd "$work" && pwd)"
heavy="$PWD/tools/sysguard/heavy.sh"
# Short, because a socket's path has a limit, and removed at the end whatever happens. It's this run's own.
state="$(mktemp -d /tmp/rf.XXXXXX)"
trap 'rm -rf "$state"' EXIT
export RECTO_FUZZ_STATE="$state/s"

"$heavy" cargo build --quiet -p recto-fuzz --bin fuzz-seeds
seeder="$PWD/target/debug/fuzz-seeds"
# Its own folder: instrumented builds share nothing with the normal ones.
export CARGO_TARGET_DIR="$work/target"
"$heavy" cargo "+$toolchain" fuzz build --sanitizer "$sanitizer"

max_len() {
  case "$1" in
    pdf_bytes) echo 1048576 ;;
    pdf_tree) echo 16384 ;;
    proto) echo 262144 ;;
    index) echo 65536 ;;
    agent) echo 65536 ;;
    control) echo 262144 ;;
    record) echo 262144 ;;
    edits) echo 8192 ;;
  esac
}

failed=()
for target in "${targets[@]}"; do
  corpus="$work/corpus/$target"
  artifacts="$work/artifacts/$target"
  mkdir -p "$corpus" "$artifacts"
  "$seeder" "$target" "$corpus"
  extra=()
  case "$target" in pdf_bytes|pdf_tree) extra+=("-dict=$PWD/fuzz/dict/pdf.dict") ;; esac
  # A check's own limits come first, so what hangs says which it was. These are the backstop, for what never returns at all.
  # A crash in one target doesn't stop the others. Its whole log is in <work>/<target>.log.
  # No leak check between runs: the control target keeps a server and its threads across inputs on purpose, and under a sanitizer two runs with it on ended with the server not answering, which none has without it.
  if ! "$heavy" cargo "+$toolchain" fuzz run --sanitizer "$sanitizer" "$target" "$corpus" -- \
    -max_total_time="$seconds" -timeout=90 -rss_limit_mb=4096 -max_len="$(max_len "$target")" \
    -artifact_prefix="$artifacts/" -print_final_stats=1 -detect_leaks=0 ${extra[@]+"${extra[@]}"} >"$work/$target.log" 2>&1; then
    failed+=("$target")
    grep -E "panicked|recto-fuzz:|ERROR|SUMMARY" -A1 "$work/$target.log" || true
  fi
  grep -E "^stat::(number_of_executed_units|average_exec_per_sec|peak_rss_mb)" "$work/$target.log" | sort -u | sed "s/^stat::/$target: /"
  rm -rf "$RECTO_FUZZ_STATE"
done
if [ ${#failed[@]} -gt 0 ]; then
  echo "failed: ${failed[*]}, with inputs in $work/artifacts/" >&2
  exit 1
fi
