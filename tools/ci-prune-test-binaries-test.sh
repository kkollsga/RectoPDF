#!/bin/bash
# Tests tools/ci-prune-test-binaries.sh on a fake deps dir with a stubbed `cargo`. Each case names the failure it catches: a wrong delete (a binary the
# workspace builds, or this job wrote) or a prune that stops the cache's save. Run: tools/ci-prune-test-binaries-test.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
script="${PRUNE_SCRIPT:-$here/ci-prune-test-binaries.sh}"
root=$(mktemp -d); trap 'rm -rf "$root"' EXIT
fails=0
A=aaaaaaaaaaaaaaaa B=bbbbbbbbbbbbbbbb C=cccccccccccccccc D=dddddddddddddddd
macho() { printf '\xcf\xfa\xed\xfe....' >"$1"; }
# case NAME: fresh deps with live (listed), dead (old, unlisted), plus extras per case; sets $deps, $bin (the stub's list) and $marker.
setup() {
  deps="$root/$1/deps"; mkdir -p "$deps" "$root/$1/bin"; marker="$root/$1/marker"
  macho "$deps/live-$A"; macho "$deps/dead-$B"
  touch -t 202001010000 "$deps/live-$A" "$deps/dead-$B"; touch "$marker"
  # the listed path is spelled with a different case in the directory part where the case asks for it
  listed="$deps/live-$A"
  stub '{"rust-suites":[{"binary-path":"'"$listed"'"}]}' 0
}
stub() { # json, exit code
  printf '%s' "$1" >"$root/$case/list.json"
  printf '#!/bin/sh\ncat "%s/list.json"\nexit %s\n' "$root/$case" "$2" >"$root/$case/bin/cargo"; chmod +x "$root/$case/bin/cargo"
}
run() { PATH="$root/$case/bin:$PATH" "$script" "$@" "$deps" 2>&1; }
check() { # description, condition result
  if [ "$2" = ok ]; then echo "ok   $case: $1"; else echo "FAIL $case: $1"; fails=$((fails+1)); fi
}
has() { [ -e "$deps/$1" ] && echo ok || echo gone; }
expect_kept() { for n in "$@"; do check "$n kept" "$([ "$(has "$n")" = ok ] && echo ok)"; done; }
expect_gone() { for n in "$@"; do check "$n removed" "$([ "$(has "$n")" = gone ] && echo ok)"; done; }
exit0() { check "exit 0" "$([ "$1" = 0 ] && echo ok)"; }

case=basic; setup $case
out=$(run --newer-than "$marker"); exit0 $?
expect_kept live-$A; expect_gone dead-$B

case=dry; setup $case
out=$(run --dry-run); exit0 $?; expect_kept live-$A dead-$B
check "dry run says it would remove" "$(echo "$out" | grep -q 'would remove 1 ' && echo ok)"

case=marker; setup $case
macho "$deps/new-$C"   # written after the marker, not listed: this job wrote it
out=$(run --newer-than "$marker"); exit0 $?; expect_kept new-$C live-$A; expect_gone dead-$B

case=not-macho; setup $case
printf 'text' >"$deps/text-$D"; touch -t 202001010000 "$deps/text-$D"
out=$(run); expect_kept text-$D; expect_gone dead-$B

case=extension; setup $case
macho "$deps/lib-$D.rlib"; macho "$deps/dead-$D.d"; touch -t 202001010000 "$deps/lib-$D.rlib" "$deps/dead-$D.d"
out=$(run); expect_kept lib-$D.rlib dead-$D.d; expect_gone dead-$B

case=case-differs; setup $case
stub '{"rust-suites":[{"binary-path":"'"$deps"'/../deps/live-'$A'"}]}' 0
out=$(run); exit0 $?; expect_kept live-$A; expect_gone dead-$B

case=jq-missing; setup $case
out=$(PRUNE_JQ=no-such-jq run); exit0 $?; expect_kept live-$A dead-$B

case=list-fails; setup $case
stub '' 3; out=$(run); exit0 $?; expect_kept live-$A dead-$B

case=list-empty; setup $case
stub '{"rust-suites":[]}' 0; out=$(run); exit0 $?; expect_kept live-$A dead-$B

case=null-paths; setup $case
stub '{"rust-suites":[{"binary_path":"/x"}]}' 0; out=$(run); exit0 $?; expect_kept live-$A dead-$B

case=none-match; setup $case
macho "$root/$case/other-$D"
stub '{"rust-suites":[{"binary-path":"'"$root/$case/other-$D"'"}]}' 0; out=$(run); exit0 $?; expect_kept live-$A dead-$B

case=hardlink; setup $case
ln "$deps/dead-$B" "$root/$case/linked"; out=$(run); exit0 $?; expect_kept dead-$B

[ $fails = 0 ] && echo "all ok" || { echo "$fails failed"; exit 1; }
