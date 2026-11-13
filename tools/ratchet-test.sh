#!/usr/bin/env bash
# ratchet-test: proves tools/ratchet.sh on throwaway trees with tiny budgets. Seconds, nothing builds. Run it after any change to tools/ratchet/.
#
#   tools/ratchet-test.sh
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
X=$(mktemp -d "${TMPDIR:-/tmp}/ratchet-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
export RATCHET_FILE_LINES=20 RATCHET_FN_LINES=6 RATCHET_STRUCT_FIELDS=3 RATCHET_ENUM_VARIANTS=3 RATCHET_IMPL_METHODS=3 RATCHET_TYPE_METHODS=5
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
# a restatement for UniFFI of a type that lives in another crate
fresh; mkdir -p "$X/t/crates/b/src"; printf 'pub use b::W;\n/// Says so.\n#[uniffi::remote(Record)]\npub struct W {\n    a: u8,\n    b: u8,\n    c: u8,\n    d: u8,\n}\n#[uniffi::remote(Enum)]\npub enum E {\n    A,\n    B,\n    C,\n    D,\n}\n' > "$X/t/crates/recto-ffi/src/lib.rs"
printf 'pub struct W {\n    a: u8,\n    b: u8,\n    c: u8,\n    d: u8,\n}\n' > "$X/t/crates/b/src/lib.rs"; R --update >/dev/null 2>&1; out=$(R)
check "a remote struct or enum over budget isn't an offender" "" "$(echo "$out" | grep 'recto-ffi/src/lib.rs::')"
has "the plain type in the other crate is" "struct crates/b/src/lib.rs::W is a new offender: 4 fields, budget 3" "$out"
has "and the restatement's lines still count toward the bridge" "the binding crate crates/recto-ffi/src grew" "$out"
fresh; printf '#[derive(uniffi::Record)]\npub struct W {\n    a: u8,\n    b: u8,\n    c: u8,\n    d: u8,\n}\n' > "$X/t/crates/a/src/lib.rs"; out=$(R)
has "the same struct derived, not remote, is an offender" "struct crates/a/src/lib.rs::W is a new offender: 4 fields" "$out"
fresh; mkdir -p "$X/t/crates/b/src"; printf 'pub struct W {\n    a: u8,\n    b: u8,\n    c: u8,\n    d: u8,\n}\n' > "$X/t/crates/recto-ffi/src/lib.rs"; printf "ffi crates/recto-ffi/src 6\nstruct crates/recto-ffi/src/lib.rs::W 4\n" > "$X/t/base.txt"
printf '#[uniffi::remote(Record)]\npub struct W {\n    a: u8,\n    b: u8,\n    c: u8,\n    d: u8,\n}\n' > "$X/t/crates/recto-ffi/src/lib.rs"; printf 'pub struct W {\n    a: u8,\n    b: u8,\n    c: u8,\n    d: u8,\n}\n' > "$X/t/crates/b/src/lib.rs"
out=$(R --moved crates/recto-ffi/src/lib.rs::W crates/b/src/lib.rs::W); rc=$?
check "--moved carries a struct from its restatement to the plain type" "0" "$rc"; has "renamed" "struct crates/b/src/lib.rs::W 4" "$(cat "$X/t/base.txt")"
fresh; printf 'struct T;\nimpl T {\n fn a(&self) {}\n fn b(&self) {}\n fn c(&self) {}\n fn d(&self) {}\n}\nimpl Tr for T {\n fn e(&self) {}\n}\n' > "$X/t/crates/a/src/lib.rs"; out=$(R); rc=$?
check "an impl over the method budget fails" "1" "$rc"; has "with its count, trait impls left out" "impl crates/a/src/lib.rs::T is a new offender: 4 methods, budget 3" "$out"

# types: every inherent impl of a name in a crate is one type, wherever its blocks are
# Failure (independent review, 2026-11-13): Reader (138 methods), Inner (119) and Session (193) left the per-impl baseline by spreading their methods over
# files as impl blocks of 39 or fewer, though each is still one type. Methods are counted per type across the crate, and the files are named.
impls() { local n=$1 t=$2; echo "impl $t {"; for ((i = 0; i < n; i++)); do echo " fn $3$i(&self) {}"; done; echo "}"; }
fresh; { echo 'struct T;'; impls 3 T m; } > "$X/t/crates/a/src/lib.rs"; out=$(R); rc=$?
check "a type under the budget in one file passes" "0" "$rc"
fresh; { echo 'struct T;'; impls 3 T a; } > "$X/t/crates/a/src/lib.rs"; impls 3 T b > "$X/t/crates/a/src/b.rs"; impls 3 T c > "$X/t/crates/a/src/c.rs"
out=$(R); rc=$?
check "the same type spread over three files fails though no block is over the per-impl budget" "1" "$rc"
has "naming the type with its count and the budget" "type crates/a/src::T is a new offender: 9 methods in all its inherent impl blocks, budget 5" "$out"
has "listing each file with its count" "crates/a/src/b.rs (3), crates/a/src/c.rs (3), crates/a/src/lib.rs (3)" "$out"
has "and what to do" "an \`impl\` block in another file is the same type" "$out"
check "no per-impl entry fires for it" "0" "$(echo "$out" | grep -c '^  - impl ')"
R --update >/dev/null 2>&1; check "update refuses to take a new offender in" "" "$(grep '^type' "$X/t/base.txt")"
printf 'ffi crates/recto-ffi/src 0\ntype crates/a/src::T 9\n' > "$X/t/base.txt"; out=$(R); rc=$?
check "at its baseline the type passes" "0" "$rc"
impls 1 T d > "$X/t/crates/a/src/d.rs"; out=$(R); rc=$?
check "one more method in any file fails" "1" "$rc"; has "with old and new" "type crates/a/src::T grew: 10 methods in all its inherent impl blocks, baseline 9, budget 5" "$out"
rm "$X/t/crates/a/src/c.rs" "$X/t/crates/a/src/d.rs"; out=$(R); rc=$?
check "a fall passes" "0" "$rc"; has "and is offered to update" "type crates/a/src::T: 9 -> 6" "$out"
R --update >/dev/null; has "update locks it in" "type crates/a/src::T 6" "$(cat "$X/t/base.txt")"
rm "$X/t/crates/a/src/b.rs"; R --update >/dev/null; check "under the budget the entry is dropped" "" "$(grep '^type' "$X/t/base.txt")"
printf 'ffi crates/recto-ffi/src 0\ntype crates/a/src::Gone 8\ntype crates/a/src::T 9\n' > "$X/t/base.txt"; R --update >/dev/null
check "an entry for a type that no longer exists is dropped" "" "$(grep '^type' "$X/t/base.txt")"
printf 'ffi crates/recto-ffi/src 0\ntype crates/a/src::T 9\n' > "$X/t/base.txt"; { echo 'struct T;'; impls 3 T a; echo "impl Tr for T {"; for i in 1 2 3 4 5 6; do echo " fn t$i(&self) {}"; done; echo "}"; } > "$X/t/crates/a/src/lib.rs"
out=$(R); check "trait impls are not counted" "0" "$(echo "$out" | grep -c 'type crates/a/src::T grew')"
fresh; { echo 'struct T;'; impls 3 T a; echo '#[cfg(test)]'; echo 'mod tests {'; impls 6 T t; echo '}'; } > "$X/t/crates/a/src/lib.rs"
out=$(R); rc=$?; check "a test module's impl is not counted" "0" "$rc"
fresh; mkdir -p "$X/t/crates/b/src"; { echo 'struct T;'; impls 2 T a; } > "$X/t/crates/a/src/lib.rs"; impls 2 T b > "$X/t/crates/a/src/b.rs"
{ echo 'struct T;'; impls 2 T a; } > "$X/t/crates/b/src/lib.rs"; impls 2 T b > "$X/t/crates/b/src/b.rs"
out=$(R); rc=$?
check "types of one name in two crates are two types, together they would be over the budget" "0" "$rc"
impls 3 T c > "$X/t/crates/b/src/c.rs"; out=$(R); rc=$?
check "and growing one doesn't hide in the other" "1" "$rc"; has "naming the crate's type" "type crates/b/src::T is a new offender: 7 methods" "$out"
# --moved leaves the per-type count alone: it is per type, not per file
fresh; { echo 'struct T;'; impls 3 T a; } > "$X/t/crates/a/src/old.rs"; impls 3 T b > "$X/t/crates/a/src/new.rs"
printf 'ffi crates/recto-ffi/src 0\ntype crates/a/src::T 6\n' > "$X/t/base.txt"; before=$(cat "$X/t/base.txt")
out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs); rc=$?
check "--moved of a file leaves the type entry where it is" "1" "$rc"; check "and the baseline unchanged" "$before" "$(cat "$X/t/base.txt")"

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
# Failure (owner, 2026-11-12: "Add a rule so the ci doesnt regress"): app-launching tests were 10% of the tests and 75% of the test time, and nothing
# stopped one more. Their count only goes down; a test elsewhere, or in a binary that doesn't launch the app, isn't counted.
fresh; mkdir -p "$X/t/crates/recto-cli/tests/app" "$X/t/crates/recto-cli/src"
printf '#[test]\nfn a() {}\n// #[test]\n#[test]\nfn b() {}\n' > "$X/t/crates/recto-cli/tests/app.rs"
printf '#[test]\nfn c() {}\n' > "$X/t/crates/recto-cli/tests/app/quiet.rs"
printf '#[test]\nfn d() {}\n#[test]\nfn e() {}\n' > "$X/t/crates/recto-cli/tests/socket.rs"
out=$(R); rc=$?
check "app-launching tests with no baseline entry fail" "1" "$rc"; has "saying the line to add, counting the binary's modules and no comment" "apptest crates/recto-cli/tests 3" "$out"
echo "apptest crates/recto-cli/tests 3" > "$X/t/base.txt"; echo "ffi crates/recto-ffi/src 0" >> "$X/t/base.txt"
check "at the baseline it passes" "0" "$(R >/dev/null; echo $?)"
printf '#[test]\nfn f() {}\n' >> "$X/t/crates/recto-cli/tests/socket.rs"
check "a new test that doesn't launch the app is free" "0" "$(R >/dev/null; echo $?)"
printf '#[test]\nfn g() {}\n' >> "$X/t/crates/recto-cli/tests/app/quiet.rs"; out=$(R); rc=$?
check "one more app-launching test fails" "1" "$rc"; has "with old, new and what to do instead" "tests that launch the app grew: 4 in crates/recto-cli/tests" "$out"
has "naming the headless app" "headless app" "$out"
R --update >/dev/null 2>&1; has "update refuses to raise it" "apptest crates/recto-cli/tests 3" "$(cat "$X/t/base.txt")"
printf '#[test]\nfn c() {}\n' > "$X/t/crates/recto-cli/tests/app/quiet.rs"; : > "$X/t/crates/recto-cli/tests/app.rs"; out=$(R); rc=$?
check "fewer passes" "0" "$rc"; has "and says it shrank" "apptest crates/recto-cli/tests: 3 -> 1" "$out"
R --update >/dev/null; has "and update locks the lower number in" "apptest crates/recto-cli/tests 1" "$(cat "$X/t/base.txt")"
echo 'sleep 1' > "$X/t/app/qa/x.sh"; out=$(R); rc=$?
check "a sleep in a qa script fails" "1" "$rc"
echo 'sleep 1 # ratchet: poll' > "$X/t/app/qa/x.sh"; out=$(R); rc=$?; check "and the marker exempts it there too" "0" "$rc"
# Failure (R3.7): `Inner`'s methods move out to free functions that still take the whole of it, so the method count falls and the hub stays the same
# size. Every function in recto-app's src/ with `Inner` in a parameter counts, however it's spelled, and the count only goes down; one in a test module,
# a test module's folder, a comment, a `fn` type, or naming another type doesn't count.
fresh; mkdir -p "$X/t/crates/recto-app/src/headless"
cat > "$X/t/crates/recto-app/src/lib.rs" <<'RS'
pub struct Inner {
    view: InnerView,
}
fn takes(inner: &Inner) {}
fn generic<'a, T: Into<u8>>(_: &'a Arc<super::Inner>, x: T) -> u8 {
    x.into()
}
fn multi(
    a: u8,
    inner: &crate::core::Inner,
) {
}
fn not_it(view: &InnerView) {}
fn alias() {
    let f: fn(&Inner) = takes;
    let s = "fn quoted(inner: &Inner)";
}
// fn commented(inner: &Inner) {}
#[cfg(test)]
mod tests {
    fn helper(inner: &Inner) {}
}
#[cfg(test)]
mod headless;
RS
echo 'fn helper(inner: &Inner) {}' > "$X/t/crates/recto-app/src/headless/x.rs"
out=$(R); rc=$?
check "functions that take the whole Inner with no baseline entry fail" "1" "$rc"; has "saying the line to add, three of them" 'whole crates/recto-app/src::Inner 3' "$out"
echo "ffi crates/recto-ffi/src 0" > "$X/t/base.txt"; echo "whole crates/recto-app/src::Inner 3" >> "$X/t/base.txt"
check "at the baseline it passes" "0" "$(R >/dev/null; echo $?)"
echo 'fn another(inner: &std::sync::Arc<Inner>) {}' >> "$X/t/crates/recto-app/src/lib.rs"; out=$(R); rc=$?
check "one more function that takes the whole Inner fails" "1" "$rc"; has "with old, new and what to do instead" "functions that take the whole Inner grew: 4 in crates/recto-app/src::Inner, baseline 3. Pass a function the parts it reads" "$out"
R --update >/dev/null 2>&1; has "update refuses to raise it" "whole crates/recto-app/src::Inner 3" "$(cat "$X/t/base.txt")"
sed -i '' -e 's/^fn takes(inner: &Inner) {}$/fn takes(view: \&InnerView) {}/' -e '/^fn another/d' "$X/t/crates/recto-app/src/lib.rs"; out=$(R); rc=$?
check "fewer passes" "0" "$rc"; has "and says it shrank" "whole crates/recto-app/src::Inner: 3 -> 2" "$out"
R --update >/dev/null; has "and update locks the lower number in" "whole crates/recto-app/src::Inner 2" "$(cat "$X/t/base.txt")"

# a move
fresh; { lines 30; echo 'fn s() { std::thread::sleep(D); }'; fn_of 10 long; } > "$X/t/crates/a/src/old.rs"
printf "ffi crates/recto-ffi/src 0\nfile crates/a/src/old.rs 41\nfn crates/a/src/old.rs::long 10\nsleep crates/a/src/old.rs 1\n" > "$X/t/base.txt"
cp "$X/t/crates/a/src/old.rs" "$X/t/crates/a/src/new.rs"; out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs); rc=$?
check "a move is refused while the old file is still there" "1" "$rc"; has "saying so" "still exists" "$out"
rm "$X/t/crates/a/src/old.rs"; out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs); rc=$?
check "a moved file's entries are renamed" "0" "$rc"
check "all of them, numbers kept, sorted" "ffi crates/recto-ffi/src 0|file crates/a/src/new.rs 41|fn crates/a/src/new.rs::long 10|sleep crates/a/src/new.rs 1|" "$(tr '\n' '|' < "$X/t/base.txt")"
out=$(R); rc=$?; check "and the tree then passes" "0" "$rc"
out=$(R --moved crates/a/src/gone.rs crates/a/src/new.rs); rc=$?
check "a move of a path with no entry is refused" "1" "$rc"; has "saying so" "no entry" "$out"
fresh; { lines 30; echo 'fn s() { std::thread::sleep(D); }'; fn_of 10 long; } > "$X/t/crates/a/src/new.rs"; echo 'fn tiny() {}' > "$X/t/crates/a/src/old.rs"
printf "ffi crates/recto-ffi/src 0\nfile crates/a/src/old.rs 41\nfn crates/a/src/old.rs::long 10\nsleep crates/a/src/old.rs 1\n" > "$X/t/base.txt"
out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs); rc=$?
check "a move is allowed when a small new file took the old path" "0" "$rc"
check "and every entry is renamed" "ffi crates/recto-ffi/src 0|file crates/a/src/new.rs 41|fn crates/a/src/new.rs::long 10|sleep crates/a/src/new.rs 1|" "$(tr '\n' '|' < "$X/t/base.txt")"
fresh; { lines 30; fn_of 10 long; } > "$X/t/crates/a/src/new.rs"; lines 25 > "$X/t/crates/a/src/old.rs"
printf "ffi crates/recto-ffi/src 0\nfile crates/a/src/old.rs 40\nfn crates/a/src/old.rs::long 10\n" > "$X/t/base.txt"; before=$(cat "$X/t/base.txt")
out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs); rc=$?
check "a move is refused while the old path is still over budget" "1" "$rc"; has "saying what it still holds" "file crates/a/src/old.rs measures 25" "$out"
check "and leaves the baseline alone" "$before" "$(cat "$X/t/base.txt")"
fresh; { lines 30; echo 'fn s() { std::thread::sleep(D); }'; } > "$X/t/crates/a/src/new.rs"; echo 'fn t() { std::thread::sleep(D); }' > "$X/t/crates/a/src/old.rs"
printf "ffi crates/recto-ffi/src 0\nfile crates/a/src/old.rs 31\nsleep crates/a/src/old.rs 1\n" > "$X/t/base.txt"
out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs); rc=$?
check "and while the old path still has a fixed sleep" "1" "$rc"; has "naming it" "sleep crates/a/src/old.rs measures 1" "$out"
fresh; { lines 30; fn_of 10 long; } > "$X/t/crates/a/src/old.rs"; echo 'fn s() { std::thread::sleep(D); }' > "$X/t/crates/a/src/new.rs"
printf "ffi crates/recto-ffi/src 0\nfile crates/a/src/old.rs 41\nfn crates/a/src/old.rs::long 10\nsleep crates/a/src/old.rs 1\n" > "$X/t/base.txt"
out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs --kind sleep); rc=$?
check "--kind moves one kind's entries while the file keeps its others" "0" "$rc"
check "and only that kind's" "ffi crates/recto-ffi/src 0|file crates/a/src/old.rs 41|fn crates/a/src/old.rs::long 10|sleep crates/a/src/new.rs 1|" "$(tr '\n' '|' < "$X/t/base.txt")"
out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs --kind fn); rc=$?
check "--kind is refused when the old path still holds that kind's offender" "1" "$rc"
fresh; fn_of 10 long > "$X/t/crates/a/src/old.rs"; printf "ffi crates/recto-ffi/src 0\nfn crates/a/src/old.rs::long 10\n" > "$X/t/base.txt"
mv "$X/t/crates/a/src/old.rs" "$X/t/crates/a/src/new.rs"; fn_of 12 long > "$X/t/crates/a/src/new.rs"; before=$(cat "$X/t/base.txt")
out=$(R --moved crates/a/src/old.rs crates/a/src/new.rs); rc=$?
check "a move to something bigger is refused" "1" "$rc"; has "saying so" "measures 12, more than the entry 10" "$out"
check "and leaves the baseline alone" "$before" "$(cat "$X/t/base.txt")"
fresh; { fn_of 10 long; echo 'fn tiny() {}'; } > "$X/t/crates/a/src/old.rs"; fn_of 3 other > "$X/t/crates/a/src/new.rs"; printf "ffi crates/recto-ffi/src 0\nfn crates/a/src/old.rs::long 10\n" > "$X/t/base.txt"
out=$(R --moved crates/a/src/old.rs::long crates/a/src/new.rs::long); rc=$?
check "one item's move is refused while it's still in the old file" "1" "$rc"
echo 'fn tiny() {}' > "$X/t/crates/a/src/old.rs"; fn_of 10 long >> "$X/t/crates/a/src/new.rs"; out=$(R --moved crates/a/src/old.rs::long crates/a/src/new.rs::long); rc=$?
check "and renamed once it moved, the old file staying" "fn crates/a/src/new.rs::long 10" "$(grep '^fn' "$X/t/base.txt")"

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

# --resolve: two branches that each lowered a number conflict in the baseline; the tool writes the merge and leaves the check and the add to the user
# Failure (the coordinator resolved it by hand on nearly every merge): taking one side whole raises the other side's lowered numbers, and a key one side
# dropped came back from the other.
# merge3 BASE OURS THEIRS: a repo with the baseline in the three versions (printf formats), the merge of theirs into ours done and left in conflict.
merge3() {
    local g="$X/m"; rm -rf "$g"; mkdir -p "$g/tools/ratchet"
    git -C "$g" init -q -b main; git -C "$g" config user.email t@t; git -C "$g" config user.name t
    printf "$1" > "$g/tools/ratchet/baseline.txt"; git -C "$g" add . && git -C "$g" commit -qm base
    git -C "$g" checkout -qb other; printf "$3" > "$g/tools/ratchet/baseline.txt"; git -C "$g" commit -qam theirs
    git -C "$g" checkout -q main; printf "$2" > "$g/tools/ratchet/baseline.txt"; git -C "$g" commit -qam ours
    git -C "$g" merge other >/dev/null 2>&1
}
RES() { "$SOURCE/tools/ratchet.sh" --resolve --root "$X/m" 2>&1; }
MB() { cat "$X/m/tools/ratchet/baseline.txt"; }
merge3 'file a.rs 50\nfn a.rs::f 30\nsleep t.rs 2\n' 'file a.rs 40\nfn a.rs::f 30\nsleep t.rs 2\n' 'file a.rs 45\nfn a.rs::f 20\nsleep t.rs 2\n'
out=$(RES); rc=$?
check "resolve: both lowered the same key, the lower wins and a key only one lowered keeps that" "0" "$rc"
check "and the file is the merge" "file a.rs 40|fn a.rs::f 20|sleep t.rs 2|" "$(MB | tr '\n' '|')"
has "it says the key both lowered" "file a.rs: ours 40, theirs 45 -> 40" "$out"
has "and the one only one side lowered" "fn a.rs::f: ours 30, theirs 20 -> 20" "$out"
check "a key the sides agree on isn't listed" "0" "$(echo "$out" | grep -c 'sleep t.rs:')"
has "it tells to run the check" "tools/ratchet.sh on the merged tree" "$out"
check "and does not git add the file" "" "$(git -C "$X/m" diff --name-only --cached --diff-filter=M | grep baseline)"
check "the index still has the conflict" "3" "$(git -C "$X/m" ls-files -u | wc -l | tr -d ' ')"
merge3 'file a.rs 50\n' 'file a.rs 50\nstruct b.rs::S 20\n' 'file a.rs 45\nenum c.rs::E 41\nimpl d.rs::T 50\n'
out=$(RES); rc=$?
check "resolve: keys each side added are kept, with the other's lowered number" "0" "$rc"
check "all kinds" "enum c.rs::E 41|file a.rs 45|impl d.rs::T 50|struct b.rs::S 20|" "$(MB | tr '\n' '|')"
has "it says who added" "struct b.rs::S: added by ours at 20 -> kept" "$out"; has "both ways" "enum c.rs::E: added by theirs at 41 -> kept" "$out"
merge3 'file a.rs 50\nfn a.rs::f 30\nfile b.rs 60\nsleep t.rs 2\n' 'file a.rs 40\nfile b.rs 60\nsleep t.rs 2\n' 'file a.rs 50\nfn a.rs::f 20\nsleep t.rs 2\n'
out=$(RES); rc=$?
check "resolve: a key either side dropped is gone, even if the other lowered it" "0" "$rc"
check "the file" "file a.rs 40|sleep t.rs 2|" "$(MB | tr '\n' '|')"
has "said for the key the other side lowered" "fn a.rs::f: dropped by ours -> gone" "$out"; has "and the other side's drop" "file b.rs: dropped by theirs -> gone" "$out"
merge3 'ffi crates/x 5\n' 'ffi crates/x 5\napptest crates/t 9\n' 'ffi crates/x 5\napptest crates/t 7\n'
out=$(RES); rc=$?
check "resolve: a key both sides added with two numbers takes the lower" "0" "$rc"
check "the file" "apptest crates/t 7|ffi crates/x 5|" "$(MB | tr '\n' '|')"
has "and says it needs the check" "apptest crates/t: ours 9, theirs 7 -> 7  (both sides added it: needs the check)" "$out"
before=$(MB); git -C "$X/m" merge --abort 2>/dev/null; git -C "$X/m" reset -q --hard; before=$(MB)
out=$(RES); rc=$?
check "resolve: with no merge under way it refuses" "1" "$rc"; has "saying so" "not in conflict" "$out"; check "and changes nothing" "$before" "$(MB)"
merge3 'file a.rs 50\n' 'file a.rs 40\n' 'file a.rs not-a-number\n'
before=$(MB); out=$(RES); rc=$?
check "resolve: a line that doesn't parse is refused" "1" "$rc"; has "naming stage and line" "stage 3 (theirs) line 1 is not \"kind key number\": 'file a.rs not-a-number'" "$out"
check "and nothing is written" "$before" "$(MB)"

# the real tree is ok
out=$(env -u RATCHET_FILE_LINES -u RATCHET_FN_LINES -u RATCHET_STRUCT_FIELDS -u RATCHET_ENUM_VARIANTS -u RATCHET_IMPL_METHODS -u RATCHET_TYPE_METHODS "$SOURCE/tools/ratchet.sh"); rc=$?; check "the checked-in baseline holds for this tree" "0" "$rc"

[ "$fails" -eq 0 ] && echo "ratchet-test: all passed" || { echo "ratchet-test: $fails failed"; exit 1; }
