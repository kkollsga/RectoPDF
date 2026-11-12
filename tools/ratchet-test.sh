#!/usr/bin/env bash
# ratchet-test: proves tools/ratchet.sh on throwaway trees with tiny budgets. Seconds, nothing builds. Run it after any change to tools/ratchet/.
#
#   tools/ratchet-test.sh
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
X=$(mktemp -d "${TMPDIR:-/tmp}/ratchet-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
export RATCHET_FILE_LINES=20 RATCHET_FN_LINES=6 RATCHET_STRUCT_FIELDS=3 RATCHET_ENUM_VARIANTS=3 RATCHET_IMPL_METHODS=3
fails=0
check() { if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi; }
has() { case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$3]"; fails=$((fails + 1)) ;; esac; }
R() { "$SOURCE/tools/ratchet.sh" --root "$X/t" --baseline "$X/t/base.txt" "$@" 2>&1; }
lines() { local n=$1; for ((i = 0; i < n; i++)); do echo "// line $i"; done; }
fresh() { rm -rf "$X/t"; mkdir -p "$X/t/crates/a/src" "$X/t/crates/a/tests" "$X/t/crates/recto-ffi/src" "$X/t/app/qa"; : > "$X/t/crates/recto-ffi/src/lib.rs"; echo "ffi crates/recto-ffi/src 0" > "$X/t/base.txt"; }
boot() { rm -f "$X/t/base.txt"; RATCHET_BOOTSTRAP=1 R >/dev/null; }
fn_of() { local n=$1; echo "fn $2() {"; for ((i = 0; i < n - 2; i++)); do echo "    let _x$i = $i;"; done; echo "}"; }

fresh; echo 'fn tiny() {}' > "$X/t/crates/a/src/lib.rs"
R --update >/dev/null; out=$(R); rc=$?
check "a small tree passes" "0" "$rc"
has "and the baseline holds the binding total" "ffi crates/recto-ffi/src 0" "$(cat "$X/t/base.txt")"

# files
fresh; lines 30 > "$X/t/crates/a/src/big.rs"; boot
has "an over-budget file goes in the baseline" "file crates/a/src/big.rs 30" "$(cat "$X/t/base.txt")"
lines 31 > "$X/t/crates/a/src/big.rs"; out=$(R); rc=$?
check "a grown file fails" "1" "$rc"; has "naming file, old, new and budget" "crates/a/src/big.rs grew: 31 production lines, baseline 30, budget 20" "$out"
has "and what to do" "guide 2.8" "$out"
lines 25 > "$X/t/crates/a/src/big.rs"; out=$(R); rc=$?
check "a shrunk file passes" "0" "$rc"; has "and says how to lock it in" "tools/ratchet.sh --update" "$out"
R --update >/dev/null; has "update lowers it" "file crates/a/src/big.rs 25" "$(cat "$X/t/base.txt")"
lines 10 > "$X/t/crates/a/src/big.rs"; R --update >/dev/null
check "under budget the entry is dropped" "" "$(grep 'big.rs' "$X/t/base.txt")"
lines 30 > "$X/t/crates/a/src/new.rs"; out=$(R); rc=$?
check "a new over-budget file fails" "1" "$rc"; has "as a new offender" "new offender" "$out"
before=$(cat "$X/t/base.txt"); out=$(R --update); rc=$?
check "update refuses to raise" "1" "$rc"; check "and leaves the baseline alone" "$before" "$(cat "$X/t/base.txt")"

# test modules are not production lines
fresh; { echo 'fn a() {}'; echo '#[cfg(test)]'; echo 'mod tests {'; lines 40; echo '}'; } > "$X/t/crates/a/src/lib.rs"
out=$(R); rc=$?; check "an inline test module doesn't count" "0" "$rc"
fresh; { echo '#[cfg(test)]'; echo 'mod checks;'; } > "$X/t/crates/a/src/lib.rs"; lines 40 > "$X/t/crates/a/src/checks.rs"
out=$(R); rc=$?; check "an out-of-line test module doesn't count" "0" "$rc"

# functions
fresh; { fn_of 10 long; } > "$X/t/crates/a/src/lib.rs"; out=$(R); rc=$?
check "a new over-budget function fails" "1" "$rc"; has "naming it with both numbers" "fn crates/a/src/lib.rs::long is a new offender: 10 lines, budget 6" "$out"
fresh; fn_of 10 long > "$X/t/crates/a/src/lib.rs"; echo 'fn s() { let _a = "}{ fn nope() {"; } // fn x() {' >> "$X/t/crates/a/src/lib.rs"
out=$(R); check "braces in strings and comments are ignored" "1" "$(echo "$out" | grep -c 'FAILED (1)')"
fresh; fn_of 5 ok > "$X/t/crates/a/src/lib.rs"; out=$(R); rc=$?; check "a function within budget passes" "0" "$rc"
fresh; { echo 'struct S;'; echo 'impl S {'; fn_of 10 grown; echo '}'; } > "$X/t/crates/a/src/lib.rs"; boot
{ echo 'struct S;'; echo 'impl S {'; fn_of 11 grown; echo '}'; } > "$X/t/crates/a/src/lib.rs"; out=$(R); rc=$?
check "a grown function fails" "1" "$rc"; has "naming impl, function and numbers" "fn crates/a/src/lib.rs::S::grown grew: 11 lines, baseline 10" "$out"

# structs, enums, impls
fresh; printf 'pub struct W {\n    a: u8,\n    #[x] pub b: Vec<(u8, u8)>,\n    c: fn(u8) -> u8,\n    d: u8,\n}\n' > "$X/t/crates/a/src/lib.rs"; out=$(R); rc=$?
check "a struct over the field budget fails" "1" "$rc"; has "with its count" "struct crates/a/src/lib.rs::W is a new offender: 4 fields, budget 3" "$out"
fresh; printf 'enum E {\n    A,\n    B(u8, u8),\n    C { x: u8 },\n    D = 4,\n}\n' > "$X/t/crates/a/src/lib.rs"; out=$(R); rc=$?
check "an enum over the variant budget fails" "1" "$rc"; has "with its count" "enum crates/a/src/lib.rs::E is a new offender: 4 variants, budget 3" "$out"
fresh; printf 'struct T;\nimpl T {\n fn a(&self) {}\n fn b(&self) {}\n fn c(&self) {}\n fn d(&self) {}\n}\nimpl Tr for T {\n fn e(&self) {}\n}\n' > "$X/t/crates/a/src/lib.rs"; out=$(R); rc=$?
check "an impl over the method budget fails" "1" "$rc"; has "with its count, trait impls left out" "impl crates/a/src/lib.rs::T is a new offender: 4 methods, budget 3" "$out"

# the binding crate
fresh; lines 5 > "$X/t/crates/recto-ffi/src/lib.rs"; boot
lines 6 > "$X/t/crates/recto-ffi/src/lib.rs"; out=$(R); rc=$?
check "the binding crate growing fails" "1" "$rc"; has "naming it" "the binding crate crates/recto-ffi/src grew: 6 production lines, baseline 5" "$out"
lines 4 > "$X/t/crates/recto-ffi/src/lib.rs"; R --update >/dev/null; has "and shrinking lowers it" "ffi crates/recto-ffi/src 4" "$(cat "$X/t/base.txt")"

# fixed sleeps
fresh; echo 'fn t() { std::thread::sleep(D); }' > "$X/t/crates/a/tests/t.rs"; out=$(R); rc=$?
check "a fixed sleep in a new test file fails" "1" "$rc"; has "naming the file" "crates/a/tests/t.rs: 1 fixed sleeps" "$out"
R --update >/dev/null 2>&1; check "update refuses to take it in" "" "$(grep sleep "$X/t/base.txt")"
printf 'fn t() { std::thread::sleep(D); }\n' > "$X/t/crates/a/tests/t.rs"; printf "ffi crates/recto-ffi/src 0\nsleep crates/a/tests/t.rs 1\n" > "$X/t/base.txt"
printf 'fn t() { std::thread::sleep(D); }\nfn u() { std::thread::sleep(D); }\n' > "$X/t/crates/a/tests/t.rs"; out=$(R); rc=$?
check "a second sleep in a file that has one fails" "1" "$rc"; has "with old and new" "2 fixed sleeps, baseline 1" "$out"
printf 'fn t() { std::thread::sleep(D); } // ratchet: poll\n// std::thread::sleep(D)\n' > "$X/t/crates/a/tests/t.rs"; out=$(R); rc=$?
check "a poll marker and a comment don't count" "0" "$rc"; has "and the entry shrank" "sleep crates/a/tests/t.rs: 1 -> 0" "$out"
echo 'sleep 1' > "$X/t/app/qa/x.sh"; out=$(R); rc=$?
check "a sleep in a qa script fails" "1" "$rc"
echo 'sleep 1 # ratchet: poll' > "$X/t/app/qa/x.sh"; out=$(R); rc=$?; check "and the marker exempts it there too" "0" "$rc"

# the baseline itself
fresh; printf 'file b 1\nfile a 1\n' > "$X/t/base.txt"; out=$(R); rc=$?
check "an unsorted baseline fails" "1" "$rc"; has "saying so" "out of order" "$out"
fresh; printf 'file a 1\nfile a 1\n' > "$X/t/base.txt"; out=$(R); rc=$?
check "a duplicate fails" "1" "$rc"; has "saying so" "duplicate" "$out"
fresh; printf 'file a 1\nfile a 2\n' > "$X/t/base.txt"; out=$(R); rc=$?
check "a key twice with two numbers fails" "1" "$rc"
check "a missing baseline fails" "1" "$(R --baseline "$X/nope" >/dev/null; echo $?)"

# report
fresh; git -C "$X/t" init -q -b main; git -C "$X/t" config user.email t@t; git -C "$X/t" config user.name t
mkdir -p "$X/t/crates/recto-session/src"; echo one > "$X/t/crates/recto-session/src/lib.rs"; echo one > "$X/t/crates/a/src/other.rs"
git -C "$X/t" add . && git -C "$X/t" commit -qm base; git -C "$X/t" checkout -qb work
printf 'two\nthree\n' >> "$X/t/crates/recto-session/src/lib.rs"; echo two >> "$X/t/crates/a/src/other.rs"; git -C "$X/t" commit -qam work
out=$(R --report main); rc=$?
check "the report never fails" "0" "$rc"; has "it lists the hub with lines added" "crates/recto-session/src/lib.rs" "$out"; has "and the count" "+2 -0" "$out"
check "and leaves other files out" "0" "$(echo "$out" | grep -c other.rs)"

# the real tree is ok
out=$(env -u RATCHET_FILE_LINES -u RATCHET_FN_LINES -u RATCHET_STRUCT_FIELDS -u RATCHET_ENUM_VARIANTS -u RATCHET_IMPL_METHODS "$SOURCE/tools/ratchet.sh"); rc=$?; check "the checked-in baseline holds for this tree" "0" "$rc"

[ "$fails" -eq 0 ] && echo "ratchet-test: all passed" || { echo "ratchet-test: $fails failed"; exit 1; }
