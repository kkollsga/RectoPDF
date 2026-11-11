#!/usr/bin/env bash
# check-test: proves tools/check.sh's rules in a throwaway repository, with stubs for cargo and the app, so no real build runs. A few seconds.
# The real heavy.sh runs on a throwaway lock, so the queue's refusals are the real ones. Run it after any change to tools/check.sh.
#
#   tools/check-test.sh
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/.." && pwd)"
X=$(mktemp -d "${TMPDIR:-/tmp}/check-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
export HEAVY_LOCK="$X/lock" HEAVY_TESTING=1 MIN_FREE=0
export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t GIT_CONFIG_GLOBAL=/dev/null
unset CHECK_AGAIN HEAVY_RETRY HEAVY_SOLO HEAVY_PRIORITY HEAVY_CLASS CARGO_BUILD_JOBS NEXTEST_PROFILE
for name in $(env | sed -n 's/^\(RECTO_[A-Z0-9_]*\)=.*/\1/p'); do unset "$name"; done
fails=0
check() { # name, expected, actual
  if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi
}
has() { # name, text wanted, in what
  case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$3]"; fails=$((fails + 1)) ;; esac
}

# Stubs: every step writes what it was asked to a log. STUB_FAIL fails the step whose line contains it, STUB_EXIT with what code, STUB_DURING runs during it.
mkdir "$X/bin"
cat > "$X/bin/cargo" <<'STUB'
#!/usr/bin/env bash
case "$*" in
  "nextest --version") echo "nextest ${STUB_NEXTEST:-1}"; exit 0 ;;
  metadata*) echo '{"name":"alpha"},{"name":"beta"}'; exit 0 ;;
esac
echo "$*" >> "$CALLS"
[ -z "${RECTO_QA_SLOW:-}" ] || echo "$RECTO_QA_SLOW $*" >> "$CALLS.slow"
if [ -n "${STUB_DURING:-}" ] && [[ "$*" == *clippy* ]]; then bash -c "$STUB_DURING"; fi
if [ -n "${STUB_FAIL:-}" ] && [[ "$*" == *"$STUB_FAIL"* ]]; then exit "${STUB_EXIT:-100}"; fi
STUB
cat > "$X/bin/rustup" <<'STUB'
#!/usr/bin/env bash
echo x86_64-pc-windows-gnu
STUB
cat > "$X/bin/swift" <<'STUB'
#!/usr/bin/env bash
echo "swift ${STUB_SWIFT:-1}"
STUB
chmod +x "$X/bin/"*
export PATH="$X/bin:$PATH" CALLS="$X/calls"
: > "$CALLS"

git init -q --bare -b main "$X/origin.git"
git init -q -b main "$X/repo"
cd "$X/repo" || exit 1
mkdir -p tools/sysguard tools/hooks app crates/alpha/src crates/beta/src docs
cp "$SOURCE/tools/check.sh" tools/check.sh
cp "$SOURCE/tools/sysguard/heavy.sh" tools/sysguard/heavy.sh
cp "$SOURCE/tools/hooks/pre-push" tools/hooks/pre-push
printf '#!/usr/bin/env bash\n' > tools/pdfium.sh
printf '#!/usr/bin/env bash\necho app-build >> "$CALLS"\n[ "${STUB_FAIL:-}" != app-build ]\n' > app/build.sh
chmod +x tools/pdfium.sh app/build.sh
echo one > crates/alpha/src/lib.rs
echo one > crates/beta/src/lib.rs
echo notes > docs/notes.md
git add . && git commit -qm start
git remote add origin "$X/origin.git"
git push -q origin main
LEDGER=.git/recto-full-runs
PASSES=.git/recto-passes
CHECKED=.git/recto-checked
full() { tools/check.sh --full "$@" 2>&1; }
ran() { local n; n=$(wc -l < "$CALLS" | tr -d ' '); : > "$CALLS"; echo "$n"; }
last_ledger_field() { tail -1 "$LEDGER" | cut -f"$1"; }
runs_in_ledger() { if [ -f "$LEDGER" ]; then wc -l < "$LEDGER" | tr -d ' '; else echo 0; fi; }
commit_code() { echo "$1" >> crates/alpha/src/lib.rs; git add . && git commit -qm "$1"; }
# Whether the pre-push hook lets this commit go up as main.
hook_accepts() { printf 'refs/heads/main %s refs/heads/main 0\n' "$(git rev-parse HEAD)" | tools/hooks/pre-push >/dev/null 2>&1 && echo yes || echo no; }
fresh_ledger() { rm -f "$LEDGER" "$PASSES" "$CHECKED"; : > "$CALLS"; }

# Failure: a tool's stage removed or moved past a slower one by accident: every stage runs, compile errors surface before anything runs, the app comes last.
git checkout -qb work
commit_code alpha-change
out=$(full); rc=$?
check "a full run passes" "0" "$rc"
sed -E 's/^fmt --manifest-path crates\/recto-headings.*/headings-fmt/; s/^fmt .*/fmt/; s/^clippy --manifest-path crates\/recto-headings.*/headings-clippy/; s/^build --locked --manifest-path crates\/recto-headings.*/headings-build/; s/^test --locked --manifest-path crates\/recto-headings.*/headings-tests/; s/^(clippy) .*/\1/; s/^check --workspace.*windows.*/windows/; s/^build.*/workers/; s/^nextest run --locked (--no-fail-fast )?-p alpha.*/changed-tests/; s/^nextest run --locked (--no-fail-fast )?--no-tests.*/other-tests/; s/^nextest run --locked (--no-fail-fast )?-p recto-cli --test app --test restore -j 4 -E .*/slow/; s/^nextest run --locked (--no-fail-fast )?-p recto-cli --test app.*/app-tests/; s/^nextest run --locked (--no-fail-fast )?-p recto-cli --test registrations.*/registrations/' "$CALLS" > "$X/order"
check "every stage runs, cheapest and likeliest first" "fmt headings-fmt clippy headings-clippy windows workers headings-build headings-tests changed-tests other-tests app-build app-tests slow registrations" "$(tr '\n' ' ' < "$X/order" | sed 's/ $//')"
has "each stage says how long it took" "check: app-tests took" "$out"
: > "$CALLS"
# Failure: the slow stage runs fast, with the slow mode off, or another stage runs slow and every app test pays for it (2026-11-11).
check "only the slow stage runs with every waiting point late" "1" "$(wc -l < "$CALLS.slow" | tr -d ' ')"
has "with the ci profile, on its chosen tests" "ci nextest run --locked --no-fail-fast -p recto-cli --test app --test restore -j 4 -E test(=" "$(cat "$CALLS.slow")"
rm -f "$CALLS.slow"

# Failure: no record of a real run, so a process review can't count them.
check "a real run leaves one ledger line" "1" "$(runs_in_ledger)"
check "with its branch" "work" "$(last_ledger_field 2)"
check "with its commit" "$(git rev-parse HEAD)" "$(last_ledger_field 3)"
check "with its result" "pass" "$(last_ledger_field 4)"
check "the push guard's list has it" "$(git rev-parse HEAD)" "$(tail -1 "$CHECKED")"

# Failure: a whole suite run again on code that has already passed.
first_pass=$(git rev-parse HEAD)
out=$(full); rc=$?
check "the same code again exits 0" "0" "$rc"
check "and runs nothing" "0" "$(ran)"
check "and adds no ledger line" "1" "$(runs_in_ledger)"
has "and says which commit passed it" "$(printf '%s' "$first_pass" | cut -c1-12)" "$out"
has "and when" "$(date '+%Y-%m-%d')" "$out"

# Failure: a doc-only commit needs a whole run, or its pass isn't recognised by the push guard.
echo more >> docs/notes.md && echo rule >> SPEC.md && git add . && git commit -qm docs
out=$(full)
check "a doc-only commit runs nothing" "0" "$(ran)"
check "and the guard has that commit" "$(git rev-parse HEAD)" "$(tail -1 "$CHECKED")"
has "and it still names the commit that passed" "$(printf '%s' "$first_pass" | cut -c1-12)" "$out"

# Failure: a pass on one file's content used for another's. The real change is found whatever the commit message says.
commit_code second-change
full > /dev/null
check "changed code runs" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
check "and is a second run" "2" "$(runs_in_ledger)"

# Failure: going back to code that passed still runs everything.
git revert --no-edit HEAD > /dev/null
full > /dev/null
check "a revert to code that passed runs nothing" "0" "$(ran)"
check "and the revert commit counts as checked" "$(git rev-parse HEAD)" "$(tail -1 "$CHECKED")"

# Failure: uncommitted work reported as checked, by the memory or by the push guard.
checked_before=$(wc -l < "$CHECKED")
passes_before=$(wc -l < "$PASSES")
echo edit >> crates/alpha/src/lib.rs
full --why "dirty test" > /dev/null
check "a dirty tree runs" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
check "its result isn't remembered" "$passes_before" "$(wc -l < "$PASSES")"
check "and the guard doesn't get it" "$checked_before" "$(wc -l < "$CHECKED")"
check "the ledger says it was dirty" "$(git rev-parse HEAD)+dirty" "$(last_ledger_field 3)"
git checkout -q -- crates/alpha/src/lib.rs
echo new > crates/alpha/src/untracked.rs
full --why "untracked test" > /dev/null
check "an untracked file also makes it dirty" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
check "and it isn't remembered" "$passes_before" "$(wc -l < "$PASSES")"
rm crates/alpha/src/untracked.rs
# Failure (coordinator, 2026-11-08): the owner's notes, untracked at the top of the checkout, stopped every pass from being remembered, though nothing reads them.
commit_code notes-base
echo "a note" > owners-notes.txt
echo "a note" > docs/draft.md
out=$(full --why "untracked notes"); rc=$?
check "untracked notes don't stop a pass being remembered" "0 yes" "$rc $(grep -q "$(git rev-parse HEAD)" "$PASSES" && echo yes || echo no)"
: > "$CALLS"
out=$(full); rc=$?
check "so the same code again runs nothing" "0 0" "$rc $(ran)"
echo "[build]" > Cargo.config-untracked.toml
full --why "untracked build file" > /dev/null
check "an untracked build file at the top still makes it dirty" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
rm owners-notes.txt docs/draft.md Cargo.config-untracked.toml
# Same content staged with a mode change or an edit is another fingerprint.
chmod +x crates/alpha/src/lib.rs && git add . && git commit -qm mode
full --why "mode test" > /dev/null
check "a changed file mode is changed code" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"

# Failure: a commit made while a run is under way reported as what that run tested.
fresh_ledger
commit_code during-base
STUB_DURING='echo during >> crates/alpha/src/lib.rs; git add .; git commit -qm during' full > /dev/null
: > "$CALLS"
check "code that changed during the run isn't remembered" "no-passes" "$([ -s "$PASSES" ] && echo has-passes || echo no-passes)"
check "and the guard doesn't get it" "no-stamp" "$([ -s "$CHECKED" ] && echo has-stamp || echo no-stamp)"
case "$(last_ledger_field 3)" in *+dirty) r=dirty ;; *) r=clean ;; esac
check "and the ledger says so" "dirty" "$r"

# Failure: a commit of documents only, made while a run is under way, throws the pass away, so the push guard refuses code that passed (2026-11-08).
fresh_ledger
commit_code docs-during-base
tested=$(git rev-parse HEAD)
STUB_DURING='mkdir -p docs; echo note >> docs/notes.md; git add .; git commit -qm notes' full > /dev/null
: > "$CALLS"
check "the commit tested is remembered" "$tested" "$(tail -1 "$PASSES" 2>/dev/null | cut -f2)"
check "and stamped for the guard, not the documents' commit" "$tested" "$(tail -1 "$CHECKED" 2>/dev/null)"
check "and the guard takes the documents' commit on top" "yes" "$(hook_accepts)"
check "and the ledger names the tested commit" "$tested" "$(last_ledger_field 3)"

# Failure: CHECK_AGAIN ignored, so a suspect pass can't be run again.
fresh_ledger
commit_code again-base
full > /dev/null; : > "$CALLS"
CHECK_AGAIN=1 full > /dev/null
check "CHECK_AGAIN runs it for real" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"

# Failure: a pass taken for a pass after an upgrade: a new compiler for Swift or a new test runner can change a result on the same code.
fresh_ledger
commit_code tool-base
full > /dev/null; : > "$CALLS"
STUB_SWIFT=2 full > /dev/null
check "another Swift version runs it again" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
STUB_NEXTEST=2 full --why "new nextest" > /dev/null
check "another nextest version runs it again" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
# Failure: a run asked to read real files answered by a run that didn't.
RECTO_BENCH100=/somewhere full --why "corpus" > /dev/null
check "a setting that changes what the tests do runs it again" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"

# Failure: everything after the first failure still runs, minutes spent on an answer already known.
fresh_ledger
commit_code clippy-fails
out=$(STUB_FAIL=clippy STUB_EXIT=101 full); rc=$?
check "the failure's exit code comes through" "101" "$rc"
check "it stops at the first failing stage, after the layout's two" "3" "$(ran)"
check "the ledger names the stage" "fail:clippy" "$(last_ledger_field 4)"
check "a failure isn't remembered as a pass" "no-passes" "$([ -s "$PASSES" ] && echo has-passes || echo no-passes)"
check "nor given to the guard" "no-stamp" "$([ -s "$CHECKED" ] && echo has-stamp || echo no-stamp)"

# Failure: the queue's refusal of a repeat counted as a run nobody got.
before=$(runs_in_ledger)
out=$(STUB_FAIL=clippy STUB_EXIT=101 full); rc=$?
check "the queue refuses the same failure, with its exit" "101" "$rc"
has "and shows why" "heavy: refused" "$out"
check "and no step ran" "0" "$(ran)"
check "and it costs no budget" "$before" "$(runs_in_ledger)"

# Failure (review V1): code laid out by hand reaches clippy and the tests, minutes spent before the cheapest check says no.
fresh_ledger
commit_code fmt-fails
out=$(STUB_FAIL="fmt --all --check" STUB_EXIT=1 full); rc=$?
check "a layout off stops the run first" "1 1" "$rc $(ran)"
check "and the ledger says so" "fail:fmt" "$(last_ledger_field 4)"
STUB_FAIL="fmt --all --check" STUB_EXIT=1 tools/check.sh --quick > /dev/null 2>&1
check "the quick check has it too" "1" "$?"
: > "$CALLS"

# Failure: the check that nothing of this tree's is left registered with Launch Services (owner, 2026-11-11: "We should only ever have one") is skipped by
# a full run, runs where the app's tests can't have left anything, or fails without saying so.
fresh_ledger
commit_code registrations-fail
out=$(STUB_FAIL="--test registrations" STUB_EXIT=100 full); rc=$?
check "a stray registration fails the full run" "100" "$rc"
check "and the ledger names the stage" "fail:registrations" "$(last_ledger_field 4)"
: > "$CALLS"
commit_code registrations-pass
full > /dev/null
check "it runs, last, in a full run that passes" "registrations" "$(grep -c -- '--test registrations' "$CALLS" | sed 's/^1$/registrations/')"
check "and the run passes" "pass" "$(last_ledger_field 4)"
: > "$CALLS"
tools/check.sh --quick > /dev/null 2>&1
check "the quick check skips it, as it builds no app" "0" "$(grep -c -- '--test registrations' "$CALLS")"
: > "$CALLS"

# Failure: a job stopped from outside held against the branch.
fresh_ledger
commit_code stopped-base
STUB_FAIL=clippy STUB_EXIT=143 full > /dev/null
: > "$CALLS"
check "a run stopped from outside is in the ledger as stopped" "stopped" "$(last_ledger_field 4)"
counted=$(tools/check.sh --runs | sed -n 's/^work: \([0-9]*\) of.*/\1/p')
check "and doesn't count" "0" "$counted"

# Failure: runs mount up with nobody deciding. A third needs a reason, and the refusal shows everything needed.
fresh_ledger
commit_code budget-1
full > /dev/null
commit_code budget-2
full > /dev/null
commit_code budget-3
: > "$CALLS"
out=$(full); rc=$?
check "a third run is refused" "3" "$rc"
check "and nothing runs" "0" "$(ran)"
check "and it isn't in the ledger" "2" "$(runs_in_ledger)"
has "it shows the count" "has had 2 full runs" "$out"
has "the earlier runs" "$(git rev-parse HEAD~2 | cut -c1-12)" "$out"
has "the cheaper options" "tools/check.sh --failed" "$out"
has "and exactly what to do" 'tools/check.sh --full --why "what this run decides"' "$out"

# Failure: a speed bump that a reason can't get past, or a reason lost.
out=$(tools/check.sh --full --why $'  decides\tthe race\nis gone ' 2>&1); rc=$?
check "with a reason it runs" "0" "$rc"
check "and really does" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
check "the reason is in the ledger, on one line" "decides the race is gone" "$(last_ledger_field 6)"
check "and the ledger line has six fields" "6" "$(tail -1 "$LEDGER" | awk -F'\t' '{print NF}')"
out=$(tools/check.sh --full --why "   " 2>&1); rc=$?
check "an empty reason isn't a reason" "2" "$rc"
out=$(tools/check.sh --quick --why "no" 2>&1); rc=$?
check "and --why is only for --full" "2" "$rc"
: > "$CALLS"

# Failure: the budget blocks code that has already passed, which costs nothing.
out=$(full); rc=$?
check "a known pass is free however many runs came first" "0" "$rc"
check "and runs nothing" "0" "$(ran)"

# Failure: a branch's old runs held against a new one of the same name, or against one that has taken in main's new code.
git checkout -q main
echo main-moved >> crates/beta/src/lib.rs && git add . && git commit -qm main-moved
git branch -qD work
git checkout -qb work
: > "$CALLS"
out=$(tools/check.sh --runs)
has "a branch cut again under an old name starts its budget again" "work: 0 of 2" "$out"
has "--runs prints the runs per branch" "3 passed" "$out"
fresh_ledger
commit_code taken-1
full > /dev/null
commit_code taken-2
full > /dev/null
git checkout -q main
echo main-moved-2 >> crates/beta/src/lib.rs && git add . && git commit -qm main-moved-2
git checkout -q work
git merge -q --no-edit main
out=$(tools/check.sh --runs)
has "a branch that has taken in main's new code starts its budget again" "work: 0 of 2" "$out"

# How main works: a budget per range not yet pushed. Failure: every merge's needed check needing a reason, or a push blocked.
fresh_ledger
git checkout -q main
commit_code main-1
full > /dev/null
commit_code main-2
full > /dev/null
commit_code main-3
: > "$CALLS"
out=$(full); rc=$?
check "main's third run on an unpushed range is refused" "3" "$rc"
has "and says on what" "on commits not yet pushed" "$out"
has "and exactly what to do" "--why" "$out"
out=$(full --why "merge of the review's fixes"); rc=$?
check "a reason lets it through, so a needed push is never blocked" "0" "$rc"
check "and the push guard takes it" "yes" "$(hook_accepts)"
git push -q origin main
commit_code main-4
: > "$CALLS"
out=$(full); rc=$?
check "after a push, main's count starts again" "0" "$rc"
check "and it really ran" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
# Failure (coordinator, 2026-11-08): runs of merges taken back off main still counted, so the next merge was refused as main's third run.
commit_code main-5
full > /dev/null
git reset -q --hard HEAD~2
commit_code main-6
: > "$CALLS"
out=$(full); rc=$?
check "runs of commits taken back off main don't count" "0 true" "$rc $([ "$(ran)" -gt 0 ] && echo true || echo false)"
commit_code main-7
full > /dev/null
commit_code main-8
out=$(full); rc=$?
check "and runs of what's still there do" "3" "$rc"
git push -q origin main

# Failure: a merge identical to its branch checked again, or not accepted for pushing.
fresh_ledger
git checkout -qb merged
commit_code merged-change
full > /dev/null
git checkout -q main
git merge -q --no-ff --no-edit merged
: > "$CALLS"
out=$(full); rc=$?
check "a merge identical to its branch runs nothing" "0" "$rc"
check "really nothing" "0" "$(ran)"
check "and the push guard takes the merge commit" "yes" "$(hook_accepts)"
# And a merge where main moved in between is new code, which does get a run.
git checkout -qb merged2
commit_code merged2-change
full > /dev/null
git checkout -q main
echo other > crates/beta/src/lib.rs && git add . && git commit -qm other
git merge -q --no-ff --no-edit merged2
: > "$CALLS"
full > /dev/null
check "a merge that makes new code gets a real run" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"

# Failure: the guard's idea of a document and the memory's drifting apart, so a code change is called a document and a stale pass accepted.
check "this script and the push guard agree on what a document is" "1" "$(grep -c "grep -vE '^docs/|\\\\.md\$'" "$SOURCE/tools/hooks/pre-push")"
check "the memory uses that same rule" "1" "$(grep -c '\$2 !~ /^docs\\/|\\.md\$/' "$SOURCE/tools/check.sh")"

# Failure: code starts reading a document, so a doc-only change can change a result while the memory says nothing changed.
readers=$(git -C "$SOURCE" grep -lE '"(\.\./)*docs/|"(\.\./)*(SPEC|AGENTS|CLAUDE)\.md"|include_(str|bytes)!\("[^"]*\.md"' -- crates app fuzz tools ':!tools/check-test.sh' 2>/dev/null | wc -l | tr -d ' ')
check "no code here reads a document" "0" "$readers"

# Failure: a pass recorded in one worktree isn't seen in another, so each runs its own.
fresh_ledger
git checkout -q main
commit_code worktree-base
full > /dev/null
git worktree add -q -b other-tree "$X/tree" main
: > "$CALLS"
out=$(cd "$X/tree" && tools/check.sh --full 2>&1); rc=$?
check "a pass counts in another worktree" "0" "$rc"
check "with nothing run" "0" "$(ran)"

# Failure (the rustfmt pass, 2026-11-08): a change to every crate left no crate for the rest, and `--workspace` with every member left out stopped the run.
fresh_ledger
git checkout -q main
git checkout -qb every-crate
echo every >> crates/alpha/src/lib.rs && echo every >> crates/beta/src/lib.rs && git add . && git commit -qm every-crate
out=$(full); rc=$?
cp "$CALLS" "$X/every-calls"
check "a change to every crate runs them all as changed, and passes" "0 yes no" "$rc $(grep -q 'nextest run --locked --no-fail-fast -p alpha -p beta' "$X/every-calls" && echo yes || echo no) $(grep -q 'no-tests=pass' "$X/every-calls" && echo yes || echo no)"
[ "$rc" = 0 ] || printf '%s\n' "$out" | tail -5
: > "$CALLS"
git checkout -q main

# The real output of an empty ledger.
fresh_ledger
check "--runs with nothing recorded says so" "No full runs recorded yet." "$(tools/check.sh --runs)"

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
