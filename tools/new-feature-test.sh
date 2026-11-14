#!/usr/bin/env bash
# new-feature-test: the feature kit's own test. Proves tools/new-feature.sh on a copy of the tree, tools/doc-paths.sh on a page made to fail, and
# app/qa/contact-sheet.sh against a stand-in recto. check.sh runs it in a full run when any of them changes (tool_tests_for, tools/check-scope.sh).
#
#   tools/new-feature-test.sh                              a few seconds: no build, since recto-agent's and recto-app's own tests compile the
#                                                          templates (`scaffold_template`), and the scaffold only puts a name in them
#   NEW_FEATURE_TEST_BUILD=1 tools/new-feature-test.sh     also clippy on the two crates of a scaffolded copy, through the queue, into the checkout's
#                                                          target (it leaves that copy's artifacts there: about a minute warm)
#
# The copy is the committed tree with the working copies of the scripts and templates under test, in a temp folder of its own, gone when the test ends.
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
target_dir=${CARGO_TARGET_DIR:-target}
case "$target_dir" in /*) ;; *) target_dir="$SOURCE/$target_dir" ;; esac
tree=$(mktemp -d "${TMPDIR:-/tmp}/new-feature-test.XXXXXX")
trap 'rm -rf "$tree"' EXIT
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t GIT_CONFIG_GLOBAL=/dev/null
fails=0
check() { # name, expected, actual
  if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi
}

git -C "$SOURCE" archive HEAD | tar -x -C "$tree"
for script in new-feature.sh doc-paths.sh doc-paths.py new-feature/command.rs new-feature/headless.rs; do
  mkdir -p "$tree/tools/$(dirname "$script")" && cp -p "$SOURCE/tools/$script" "$tree/tools/$script"
done
cd "$tree" || exit 1
git init -q && git add -A && git commit -qm base

# Failure: the scaffold leaves a tree that doesn't build, isn't in rustfmt's layout, or touches more than its two files and two marked lines.
out=$(tools/new-feature.sh demo_feature 2>&1); rc=$?
check "it makes a feature" "0" "$rc"
check "its two files" "yes yes" "$([ -f crates/recto-agent/src/commands/demo_feature.rs ] && echo yes) $([ -f crates/recto-app/src/core/headless/demo_feature.rs ] && echo yes)"
check "and one marked line in each of two shared files, nothing else" \
  "crates/recto-agent/src/commands/mod.rs crates/recto-app/src/core/headless/mod.rs" "$(git diff --name-only | tr '\n' ' ' | sed 's/ $//')"
check "each a mod line" "1 1" "$(git diff -U0 crates/recto-agent/src/commands/mod.rs | grep -c '^+pub mod demo_feature; // new-feature: demo_feature') $(git diff -U0 crates/recto-app/src/core/headless/mod.rs | grep -c '^+mod demo_feature; // new-feature: demo_feature')"
check "the tree is in rustfmt's layout" "0" "$(cargo fmt --all --check >/dev/null 2>&1; echo $?)"
check "a test at each tier, and one for its decisions, says what to fill in" "5" "$(cat crates/recto-agent/src/commands/demo_feature.rs crates/recto-app/src/core/headless/demo_feature.rs | grep -c 'fn fill_me_in_demo_feature_')"
case "$out" in *"FEATURES.md"*"tools/new-feature.sh --remove demo_feature"*) check "it prints the checklist" ok ok ;; *) check "it prints the checklist" ok "$out" ;; esac
check "a second time is refused, changing nothing" "1 4" "$(tools/new-feature.sh demo_feature >/dev/null 2>&1; echo $?) $(git status --porcelain | grep -c .)"
check "a name that isn't a tag is refused" "2" "$(tools/new-feature.sh Demo-Feature >/dev/null 2>&1; echo $?)"
check "nothing of the template's own is in it" "0" "$(cat crates/recto-agent/src/commands/demo_feature.rs crates/recto-app/src/core/headless/demo_feature.rs | grep -c 'template-only\|scaffold_template\|new_feature\|NewFeature')"
if [ "${NEW_FEATURE_TEST_BUILD:-0}" = 1 ]; then
  # The two crates it touches, their tests included, with the gate's lints. Through the queue, into the checkout's target so its dependencies are reused.
  built=$(CARGO_TARGET_DIR="$target_dir" HEAVY_CLASS=quick "$SOURCE/tools/sysguard/heavy.sh" \
    cargo clippy --locked -p recto-agent -p recto-app --all-targets -- -D warnings 2>&1); rc=$?
  check "it builds, its tests too, with no lint" "0" "$rc"
  [ "$rc" = 0 ] || printf '%s\n' "$built" | tail -30
fi
tools/new-feature.sh --remove demo_feature >/dev/null
check "--remove leaves the tree as it was" "" "$(git status --porcelain)"

# Failure (review): --remove takes a file it didn't make, or another feature's line whose name starts with this one's; a name that is a keyword, a
# module already there, or a folder already there makes a tree that doesn't build or overwrites what was there.
tools/new-feature.sh --remove find >/dev/null 2>&1
check "--remove of a name it didn't make takes nothing" "" "$(git status --porcelain)"
tools/new-feature.sh page_labels >/dev/null
tools/new-feature.sh --remove page >/dev/null 2>&1
check "--remove of a name that starts another's leaves the other's lines" "1 1" "$(grep -c 'mod page_labels; // new-feature: page_labels' crates/recto-agent/src/commands/mod.rs) $(grep -c 'mod page_labels; // new-feature: page_labels' crates/recto-app/src/core/headless/mod.rs)"
tools/new-feature.sh --remove page_labels >/dev/null
check "a keyword is refused" "2" "$(tools/new-feature.sh type >/dev/null 2>&1; echo $?)"
check "a module that's there is refused" "1" "$(tools/new-feature.sh find >/dev/null 2>&1; echo $?)"
mkdir -p crates/recto-agent/src/commands/thing && touch crates/recto-agent/src/commands/thing/mod.rs
check "a folder that's there is refused" "1" "$(tools/new-feature.sh thing >/dev/null 2>&1; echo $?)"
rm -rf crates/recto-agent/src/commands/thing
check "and none of them changed anything" "" "$(git status --porcelain)"

# Failure: a how-to page names a file or a test that a rename took away, and the check says nothing.
printf 'See `crates/recto-agent/src/lib.rs`, `execute` and `tools/check.sh --quick`.\n' > good.md
# A name made here, so no source (this one included) has it.
gone="a_test_renamed_$RANDOM$RANDOM"
printf 'See `crates/recto-agent/src/gone.rs` and `%s`.\n' "$gone" > bad.md
check "doc-paths passes a page that names what's there" "0" "$(tools/doc-paths.sh good.md >/dev/null 2>&1; echo $?)"
said=$(tools/doc-paths.sh bad.md 2>&1); rc=$?
check "and fails one that doesn't" "1" "$rc"
case "$said" in *"bad.md:1: no such file: crates/recto-agent/src/gone.rs"*"no such name in the sources: $gone"*) check "naming each, with its line" ok ok ;; *) check "naming each, with its line" ok "$said" ;; esac
check "the pages it holds pass" "0" "$(tools/doc-paths.sh >/dev/null 2>&1; echo $?)"

# The contact sheet (app/qa/contact-sheet.sh), against a stand-in `recto` that writes down each call and the state folder it was given, so nothing is
# launched. Failure (review): a `recto` call, the trap's `stop` included, goes to the user's own state, and so to their own app or session; or a step
# that fails is passed over, and a shot after it stands for a state that never was.
sheet="$tree/sheet" && mkdir -p "$sheet/bin" "$sheet/stub"
cat > "$sheet/bin/recto" <<'STUB'
#!/usr/bin/env bash
printf '%s\t%s\n' "${RECTO_STATE_DIR:-<none>}" "$*" >> "$SHEET_LOG"
if [ -n "${STUB_FAIL_ON:-}" ]; then case "$*" in *"$STUB_FAIL_ON"*) exit 1 ;; esac; fi
st=$SHEET_STUB
case "$*" in
  "qa state --json") printf '{"data":{"app":{"document_edited":%s,"sheet":%s,"copy_notice":%s}}}\n' "$(cat "$st/edited" 2>/dev/null || echo false)" "$(cat "$st/sheet" 2>/dev/null || echo null)" "$(cat "$st/copy" 2>/dev/null || echo null)" ;;
  "qa shot "*) echo '{"settled":true}' ;;
  "select "*) echo '{"data":{"pages":[{"rects":[[0,0,9,9],[0,0,9,9],[0,0,9,9],[0,0,9,9],[0,0,9,9],[0,0,9,9],[0,0,9,9]]}]}}' ;;
  "contents --generated --json") echo '{"data":{"entries":[{"entry":1,"depth":0},{"entry":2,"depth":1}]}}' ;;
  "qa as-person page rotate"*) echo true > "$st/edited" ;;
  "qa menu file.close") echo '{"kind":"close_prompt"}' > "$st/sheet" ;;
  "qa as-person open"*) echo '{"kind":"password"}' > "$st/sheet" ;;
  "qa sheet-answer"* | "qa password") echo null > "$st/sheet" ;;
  "qa menu edit.copy") echo '"notice"' > "$st/copy" ;;
esac
exit 0
STUB
printf '#!/usr/bin/env bash\neval "out=\\${$#}"; : > "$out"\n' > "$sheet/bin/qpdf"
printf '#!/usr/bin/env bash\nexit 0\n' > "$sheet/bin/swift"
chmod +x "$sheet/bin/"*
contact_sheet() { # OUT [STUB_FAIL_ON]
  : > "$sheet/log"; rm -f "$sheet/stub/"*
  PATH="$sheet/bin:$PATH" RECTO_BIN="$sheet/bin/recto" RECTO_APP="$sheet/no-such-app/RectoPDF" SHEET_LOG="$sheet/log" \
    SHEET_STUB="$sheet/stub" STUB_FAIL_ON="${2:-}" RECTO_STATE_DIR= bash "$SOURCE/app/qa/contact-sheet.sh" "$1" > "$sheet/run.out" 2>&1
}
contact_sheet "$sheet/out"; rc=$?
check "the contact sheet runs through every step against the stand-in" "0" "$rc"
check "and calls recto many times" "true" "$([ "$(wc -l < "$sheet/log")" -gt 50 ] && echo true || echo false)"
# One temp root for every call, the script's own: a state folder in it for each look and one before the first, and gone when the script ends.
roots=$(cut -f1 "$sheet/log" | sed -E 's#/state-(none|light|dark)$##' | sort -u)
check "every call, the first stop and the trap's included, goes to a state folder of the script's own temp root" "1 yes" \
  "$(printf '%s\n' "$roots" | grep -c .) $(case "$roots" in (/*) [ ! -e "$roots" ] && echo yes ;; esac)"
check "and the calls before the first run and in the trap are among them" "yes" "$(head -1 "$sheet/log" | grep -q '/state-none'$'\t''stop' && tail -1 "$sheet/log" | grep -q $'\t''stop$' && echo yes)"
contact_sheet "$sheet/out2" "palette-type"; rc=$?
check "a step that fails stops the run" "1" "$rc"
check "naming the step" "1" "$(grep -c 'FAILED (light, after shot 10): recto qa palette-type zoom' "$sheet/out2/notes.txt")"
check "and nothing after it ran" "0" "$(grep -c 'edit mode on' "$sheet/log")"

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
