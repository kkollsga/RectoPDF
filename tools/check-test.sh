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
unset CHECK_AGAIN HEAVY_RETRY HEAVY_SOLO HEAVY_PRIORITY HEAVY_CLASS CARGO_BUILD_JOBS NEXTEST_PROFILE CI
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
  # beta depends on alpha, gamma on nothing: a change to alpha affects beta too.
  metadata*) echo '{"packages":[{"name":"alpha","dependencies":[]},{"name":"beta","dependencies":[{"name":"alpha","path":"/x"},{"name":"serde","path":null}]},{"name":"gamma","dependencies":[]}]}'; exit 0 ;;
esac
echo "$*" >> "$CALLS"
[ -z "${RECTO_QA_SLOW:-}" ] || echo "$RECTO_QA_SLOW $*" >> "$CALLS.slow"
[[ "$*" != *"--test notices"* ]] || echo "skip=${RECTO_SKIP_APP:-} require=${RECTO_REQUIRE_APP:-} $*" >> "$CALLS.bundle"
if [ -n "${STUB_DURING:-}" ] && [[ "$*" == "build --locked -p recto-render-service"* ]]; then bash -c "$STUB_DURING"; fi
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
mkdir -p tools/sysguard tools/hooks app crates/alpha/src crates/beta/src crates/gamma/src docs
cp "$SOURCE/tools/check.sh" tools/check.sh
# The real rules, with this repository's own guards and tiers: alpha has a guard, gamma is what the lifecycle tier is about, beta is a window crate.
cp "$SOURCE/tools/check-scope.sh" tools/check-scope.sh
cat >> tools/check-scope.sh <<'SCOPE'
GUARD_TESTS='test(~guard_one) | test(~stale::)'
APP_BINARIES=''
LIFECYCLE_TESTS='test(~life_test)'
WINDOW_TESTS="test(~window_test) & !($LIFECYCLE_TESTS)"
WINDOW_CRATES='beta'
LIFECYCLE_PATHS='^app/|^crates/gamma/|^crates/beta/src/record\.rs$'
SCOPE
cp "$SOURCE/tools/sysguard/heavy.sh" tools/sysguard/heavy.sh
cp "$SOURCE/tools/hooks/pre-push" tools/hooks/pre-push
printf '#!/usr/bin/env bash\n' > tools/pdfium.sh
printf '#!/usr/bin/env bash\necho app-build >> "$CALLS"\n[ "${STUB_FAIL:-}" != app-build ]\n' > app/build.sh
chmod +x tools/pdfium.sh app/build.sh
printf 'mod stale;\nfn guard_one() {}\n' > crates/alpha/src/lib.rs
echo one > crates/beta/src/lib.rs
echo "fn life_test() {}" > crates/gamma/src/lib.rs
echo "fn window_test() {}" >> crates/beta/src/lib.rs
echo notes > docs/notes.md
git add . && git commit -qm start
git remote add origin "$X/origin.git"
git push -q origin main
LEDGER=.git/recto-full-runs
PASSES=.git/recto-passes
CHECKED=.git/recto-checked
# A full run of everything, as CI and main have it, which is what most cases here are about. Selection has its own cases further down: sel_full.
full() { CHECK_ALL=1 tools/check.sh --full "$@" 2>&1; }
sel_full() { tools/check.sh --full "$@" 2>&1; }
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
sed -E 's/^fmt --manifest-path crates\/recto-headings.*/headings-fmt/; s/^fmt .*/fmt/; s/^clippy --manifest-path crates\/recto-headings.*/headings-clippy/; s/^build --locked --manifest-path crates\/recto-headings.*/headings-build/; s/^test --locked --manifest-path crates\/recto-headings.*/headings-tests/; s/^clippy --workspace --locked --target x86_64-pc-windows-gnu.*/windows/; s/^(clippy) .*/\1/; s/^build.*/workers/; s/^nextest run --locked (--no-fail-fast )?-p alpha.*/changed-tests/; s/^nextest run --locked (--no-fail-fast )?--no-tests.*/other-tests/; s/^nextest run --locked (--no-fail-fast )?-p recto-cli --test app --test restore -j 4 -E .*/slow/; s/^nextest run --locked (--no-fail-fast )?-p recto-cli --test app.*/app-tests/; s/^nextest run --locked (--no-fail-fast )?-p recto-cli --test registrations.*/registrations/; s/^nextest run --locked (--no-fail-fast )?-p recto-cli --test notices.*/app-bundle/' "$CALLS" > "$X/order"
check "every stage runs, cheapest and likeliest first" "fmt headings-fmt clippy headings-clippy windows workers headings-build headings-tests changed-tests other-tests app-build app-bundle app-tests slow registrations" "$(tr '\n' ' ' < "$X/order" | sed 's/ $//')"
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

# Failure (process review, 2026-11-11): a tracked file edited during a run and put back before it ends, as a test tried mid-run is, leaves the tree as it
# started, so the run's pass was stamped on a commit whose code wasn't all of what ran.
fresh_ledger
commit_code put-back-base
out=$(STUB_DURING='cp crates/alpha/src/lib.rs "$CALLS.keep"; echo during >> crates/alpha/src/lib.rs; cp "$CALLS.keep" crates/alpha/src/lib.rs' full); rc=$?
: > "$CALLS"
check "a file changed and put back during the run: the run still passes" "0" "$rc"
check "but isn't remembered" "no-passes" "$([ -s "$PASSES" ] && echo has-passes || echo no-passes)"
check "nor given to the guard" "no-stamp" "$([ -s "$CHECKED" ] && echo has-stamp || echo no-stamp)"
case "$(last_ledger_field 3)" in *+dirty) r=dirty ;; *) r=clean ;; esac
check "and the ledger says it was dirty" "dirty" "$r"
has "and it says why" "changed while it ran" "$out"
has "naming the file" "crates/alpha/src/lib.rs" "$out"
# And a run nothing touched is still stamped, so the check never refuses everything.
commit_code untouched-base
full > /dev/null
: > "$CALLS"
check "a run nothing touched is stamped" "$(git rev-parse HEAD)" "$(tail -1 "$CHECKED" 2>/dev/null)"

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
check "a clippy error is the compile gate's: no run is in the ledger" "0" "$(runs_in_ledger)"
has "and it says no test was queued" "no test was queued" "$out"
check "a failure isn't remembered as a pass" "no-passes" "$([ -s "$PASSES" ] && echo has-passes || echo no-passes)"
check "nor given to the guard" "no-stamp" "$([ -s "$CHECKED" ] && echo has-stamp || echo no-stamp)"

# Failure: the queue's refusal of a repeat counted as a run nobody got.
before=$(runs_in_ledger)
out=$(STUB_FAIL=clippy STUB_EXIT=101 full); rc=$?
check "the queue refuses the same failure, with its exit" "101" "$rc"
has "and shows why" "heavy: refused" "$out"
check "and nothing ran but the layout's two checks, which build nothing" "2" "$(ran)"
check "and it costs no budget" "$before" "$(runs_in_ledger)"

# Failure (review V1): code laid out by hand reaches clippy and the tests, minutes spent before the cheapest check says no.
fresh_ledger
commit_code fmt-fails
out=$(STUB_FAIL="fmt --all --check" STUB_EXIT=1 full); rc=$?
check "a layout off stops the run first" "1 1" "$rc $(ran)"
check "and costs no run: it is found before the queue" "0" "$(runs_in_ledger)"
has "and says nothing was queued" "nothing was queued" "$out"
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

# Failure (icon worker, 2026-11-11): the bundle's own checks (its notices, no extended attributes) ran only with RECTO_SKIP_APP, so a full run never held
# the app it had just built to them. They run after the build, needing that app, and a failure there fails the run.
fresh_ledger
rm -f "$CALLS.bundle"
commit_code bundle-pass
full > /dev/null
: > "$CALLS"
bundle=$(cat "$CALLS.bundle" 2>/dev/null)
check "the bundle's checks run once in a full run" "1" "$(printf '%s' "$bundle" | grep -c . | tr -d ' ')"
has "needing the app just built, not skipping it" "skip= require=1 " "$bundle"
has "with the notices' copy" "test(=the_built_app_carries_the_notices)" "$bundle"
has "and extended attributes" "test(=nothing_in_the_built_app_has_extended_attributes)" "$bundle"
commit_code bundle-fail
out=$(STUB_FAIL="--test notices" STUB_EXIT=100 full); rc=$?
check "a bundle without its notices fails the full run" "100" "$rc"
check "and the ledger names the stage" "fail:app-bundle" "$(last_ledger_field 4)"
rm -f "$CALLS.bundle"
tools/check.sh --quick > /dev/null 2>&1
check "the quick check skips them, as it builds no app" "0" "$( [ -f "$CALLS.bundle" ] && wc -l < "$CALLS.bundle" | tr -d ' ' || echo 0)"
: > "$CALLS"

# Failure (2026-11-11): the Windows compile check passed on a warning, an unused variable in code only Windows builds see (916efaf), which no other
# stage compiles. It fails on warnings as clippy does, in the quick check and the full one.
: > "$CALLS"
tools/check.sh --quick > /dev/null 2>&1
check "the quick check's Windows compile check fails on warnings" "1" "$(grep -c -- '--target x86_64-pc-windows-gnu.* -- -D warnings$' "$CALLS")"
: > "$CALLS"
fresh_ledger
commit_code windows-warns
out=$(STUB_FAIL="x86_64-pc-windows-gnu" STUB_EXIT=101 full); rc=$?
check "and a warning there fails the full run" "101" "$rc"
check "at the compile gate, with no run counted" "0" "$(runs_in_ledger)"
: > "$CALLS"

# Failure: a job stopped from outside held against the branch.
fresh_ledger
commit_code stopped-base
STUB_FAIL="-p recto-render-service" STUB_EXIT=143 full > /dev/null
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
out=$(CHECK_ALL=1 tools/check.sh --full --why $'  decides\tthe race\nis gone ' 2>&1); rc=$?
check "with a reason it runs" "0" "$rc"
check "and really does" "true" "$([ "$(ran)" -gt 0 ] && echo true || echo false)"
check "the reason is in the ledger, on one line" "decides the race is gone" "$(last_ledger_field 6)"
check "and the ledger line has seven fields, the last saying everything ran" "7 all" "$(tail -1 "$LEDGER" | awk -F'\t' '{print NF}') $(last_ledger_field 7)"
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
echo every >> crates/alpha/src/lib.rs && echo every >> crates/beta/src/lib.rs && echo every >> crates/gamma/src/lib.rs && git add . && git commit -qm every-crate
out=$(full); rc=$?
cp "$CALLS" "$X/every-calls"
check "a change to every crate runs them all as changed, and passes" "0 yes no" "$rc $(grep -q 'nextest run --locked --no-fail-fast -p alpha -p beta -p gamma' "$X/every-calls" && echo yes || echo no) $(grep -q 'nextest run --locked --no-fail-fast --no-tests=pass' "$X/every-calls" && echo yes || echo no)"
[ "$rc" = 0 ] || printf '%s\n' "$out" | tail -5
: > "$CALLS"
git checkout -q main

# The real output of an empty ledger.
fresh_ledger
check "--runs with nothing recorded says so" "No full runs recorded yet." "$(tools/check.sh --runs)"

# Failure (CI run 43080381613): the slow stage ran on CI, the slow machine already, and its profile on top of CI's own slowness took two tests past their
# bounds. With CI set, every other stage runs and the slow one doesn't.
git checkout -qb on-ci
commit_code ci-run
: > "$CALLS"
rm -f "$CALLS.slow"
CI=true full >/dev/null; rc=$?
check "on CI a full run passes" "0" "$rc"
check "with nothing run slow" "0" "$( [ -f "$CALLS.slow" ] && wc -l < "$CALLS.slow" | tr -d ' ' || echo 0)"
has "and the app tests still run" "-p recto-cli --test app --test restore --test compare --test long_session" "$(cat "$CALLS")"
: > "$CALLS"
rm -f "$CALLS.slow"


# ---- R0.1: the quick check ------------------------------------------------------------------------------------------------------------------------------
# Failure: `--quick` looked only at uncommitted files, so after a commit it fell back to the whole workspace (or, with nothing changed, tested nothing).
# It takes what changed since the branch left main, committed or not, and untracked files too.
quick() { : > "$CALLS"; tools/check.sh --quick > "$X/quick.out" 2>&1; echo $?; cp "$CALLS" "$X/quick.calls"; : > "$CALLS"; }
quick_tests() { grep '^nextest run' "$X/quick.calls" || true; }
git checkout -q main
git checkout -qb quick-1
echo q >> crates/gamma/src/lib.rs && git add . && git commit -qm gamma-committed
check "a committed change is enough for the quick check" "0" "$(quick)"
has "it tests the crate changed in the commit" "package(=gamma)" "$(quick_tests)"
check "and not the whole workspace" "0" "$(quick_tests | grep -c -- '^nextest run --locked --no-fail-fast --no-tests=pass --workspace$')"
echo more >> crates/beta/src/lib.rs
quick > /dev/null
has "and what is uncommitted on top" "package(=beta)" "$(quick_tests)"
echo new > crates/alpha/src/new.rs
quick > /dev/null
has "and an untracked file" "package(=alpha)" "$(quick_tests)"
rm crates/alpha/src/new.rs; git checkout -q crates/beta/src/lib.rs
# Failure: a registration list broken by a change in another crate fails only minutes later, in a full run: the guards run whichever crate changed.
quick > /dev/null
has "the registration guards run whichever crate changed" "test(~guard_one)" "$(quick_tests)"
has "all of them" "test(~stale::)" "$(quick_tests)"
check "and not the other crates' tests" "0" "$(quick_tests | grep -c 'package(=alpha)')"
git checkout -q main
git checkout -qb quick-2
quick > /dev/null
has "with nothing changed it runs the guards alone" "-E test(~guard_one)" "$(quick_tests)"
# A change outside crates/ and app/ can affect anything, as before.
echo t > tools/note.txt && git add . && git commit -qm tools-change
quick > /dev/null
check "a change to the tools runs every test" "1" "$(quick_tests | grep -c -- '--workspace$')"
git checkout -q main

# Failure: a guard renamed or moved, so the filter names nothing and the guard quietly stops running. It stops the check before anything is queued, and says which.
git checkout -qb quick-3
sed -i '' 's/fn guard_one/fn guard_renamed/' crates/alpha/src/lib.rs
check "a guard that no longer exists stops the quick check" "2" "$(quick)"
has "naming it" "test guard_one" "$(cat "$X/quick.out")"
check "before any step ran" "0" "$(wc -l < "$X/quick.calls" | tr -d ' ')"
git checkout -q crates/alpha/src/lib.rs
sed -i '' 's/fn life_test/fn life_renamed/' crates/gamma/src/lib.rs
out=$(full); rc=$?
check "a test a tier names that no longer exists stops a full run" "2" "$rc"
has "naming it" "test life_test" "$out"
git checkout -q crates/gamma/src/lib.rs
sed -i '' 's/^mod stale;//' crates/alpha/src/lib.rs
quick > /dev/null
has "a guard module that no longer exists is found too" "module stale::" "$(cat "$X/quick.out")"
git checkout -q crates/alpha/src/lib.rs
# A test file that drives the app and is named in no tier would run in none.
mkdir -p crates/recto-cli/tests
echo 'fn x() { app_path(); }' > crates/recto-cli/tests/newbie.rs
has "a test file that starts the app and is in no tier is found" "tests/newbie.rs" "$(bash -c 'set -o pipefail; . tools/check-scope.sh; scope_names_missing .')"
rm -rf crates/recto-cli
git checkout -q main
# And this repository's real names exist, now, so the rule above isn't only proved on a stub.
check "every name in the real tools/check-scope.sh exists" "" "$(cd "$SOURCE" && bash -c 'set -o pipefail; . tools/check-scope.sh; scope_names_missing .')"

# Failure: a compile error found by a test job, minutes into a queue wait (46 of 156 failed jobs in 30 hours). Clippy and the Windows check are a job of their
# own, in front; when they fail no test is queued.
git checkout -qb gate-1
echo g >> crates/gamma/src/lib.rs && git add . && git commit -qm gate-change
out=$(STUB_FAIL=clippy STUB_EXIT=101 tools/check.sh --quick 2>&1); rc=$?
check "a clippy error fails the quick check" "101" "$rc"
check "and no test was started" "0" "$(grep -c '^nextest' "$CALLS")"
check "nor the workers built" "0" "$(grep -c '^build' "$CALLS")"
: > "$CALLS"
git checkout -q main

# ---- R0.2: --full by what changed -----------------------------------------------------------------------------------------------------------------------
# Failure: a change to one crate ran every test and every app test, 514 s on average. The selection:
branch_with() { git checkout -q main; git checkout -qb "$1"; fresh_ledger; }
sel() { : > "$CALLS"; rm -f "$CALLS.slow"; sel_full "$@" > "$X/sel.out"; echo $?; cp "$CALLS" "$X/sel.calls"; : > "$CALLS"; }
sel_has() { grep -c -- "$1" "$X/sel.calls" | tr -d ' '; }
branch_with sel-window
echo w >> crates/beta/src/lib.rs && git add . && git commit -qm beta-only
check "a change to a window crate passes selectively" "0" "$(sel)"
has "the start says what was selected" "selective" "$(cat "$X/sel.out")"
has "and what was skipped" "skipped: lifecycle tier" "$(cat "$X/sel.out")"
check "the window tier runs" "1" "$(sel_has 'app --test restore --test compare -j 4 -E test(~window_test)')"
check "the lifecycle tier doesn't" "0" "$(sel_has 'long_session')"
check "nor the stage that checks what is left registered" "0" "$(sel_has '--test registrations')"
check "the slow stage runs only what is in the window tier" "1" "$(grep -c 'window_test' "$CALLS.slow" | tr -d ' ')"
check "its crate's tests run, and the guards" "1" "$(sel_has 'nextest run --locked --no-fail-fast -p beta')"
check "the other crates' tests don't (alpha, gamma)" "0" "$(sel_has 'package(=gamma)')"
check "the app is built, for the window tier" "1" "$(sel_has '^app-build')"
check "the run's ledger line says it was selective" "selective" "$(last_ledger_field 7)"
check "the push guard is not given a selective pass" "no-stamp" "$([ -s "$CHECKED" ] && echo has-stamp || echo no-stamp)"
check "the memory has it, for this selection" "1" "$(grep -c 'sel:' "$PASSES")"

# Failure: a selective pass taken for a pass of everything, so main is pushed on tests that were never run.
: > "$CALLS"
out=$(sel_full); rc=$?
check "the same code and selection again runs nothing" "0 0" "$rc $(ran)"
has "and says it was not a pass of everything" "doesn't let \`main\` be pushed" "$out"
check "and still gives the guard nothing" "no-stamp" "$([ -s "$CHECKED" ] && echo has-stamp || echo no-stamp)"
out=$(CHECK_ALL=1 tools/check.sh --full --why "everything, after a selective pass" 2>&1); rc=$?
check "CHECK_ALL=1 runs everything on the same code" "0 true" "$rc $([ "$(ran)" -gt 0 ] && echo true || echo false)"
check "and that pass is stamped for the guard" "$(git rev-parse HEAD)" "$(tail -1 "$CHECKED")"
check "with the ledger saying everything ran" "all" "$(last_ledger_field 7)"
check "and everything's pass answers a selective run, free" "0 0" "$(sel >/dev/null; echo 0) $(wc -l < "$X/sel.calls" | tr -d ' ')"
# A pass of a smaller selection isn't a pass of a larger one: the same code with more changed.
branch_with sel-memory
echo m1 >> crates/gamma/src/lib.rs && git add . && git commit -qm m1
sel > /dev/null
echo m2 >> crates/beta/src/lib.rs && git add . && git commit -qm m2
check "another selection on the same branch runs for real" "true" "$(sel --why selection > /dev/null; [ "$(wc -l < "$X/sel.calls" | tr -d ' ')" -gt 0 ] && echo true || echo false)"
# Failure: main moved, so the same branch code is selected differently (its base changed), and the earlier pass was for another selection.
check "a different base selects differently, so the old pass doesn't answer" "true" "$(CHECK_BASE=HEAD~1 sel --why base > /dev/null; [ "$(wc -l < "$X/sel.calls" | tr -d ' ')" -gt 0 ] && echo true || echo false)"

branch_with sel-lifecycle
echo l >> crates/gamma/src/lib.rs && git add . && git commit -qm gamma-only
check "a change to the lifecycle's crates passes" "0" "$(sel)"
check "it runs the lifecycle tier" "1" "$(sel_has 'long_session -j 4 -E test(~life_test)')"
check "and the stage that checks what is left registered" "1" "$(sel_has '--test registrations')"
check "and not the window tier" "0" "$(sel_has 'window_test')"
branch_with sel-dependent
echo d >> crates/alpha/src/lib.rs && git add . && git commit -qm alpha-only
sel > /dev/null
check "a crate's dependents are tested with it" "1" "$(sel_has 'package(=beta)')"
check "and a change two crates down reaches the window tier" "2" "$(sel_has 'window_test')"
check "without the lifecycle tier" "0" "$(sel_has 'long_session')"
branch_with sel-app
echo a > app/x.swift && git add . && git commit -qm app-only
sel > /dev/null
check "a change to app/ runs both tiers" "1" "$(sel_has 'app --test restore --test compare --test long_session -j 4$')"
check "but no crate's tests, the guards only" "0" "$(sel_has 'package(=')"
branch_with sel-tools
echo t > tools/note.txt && git add . && git commit -qm tools
sel > /dev/null
has "a change to the tools says why it runs everything" "everything runs" "$(cat "$X/sel.out")"
check "it runs every crate's tests" "1" "$(sel_has 'nextest run --locked --no-fail-fast --no-tests=pass --workspace')"
check "and both app tiers" "1" "$(sel_has 'app --test restore --test compare --test long_session -j 4$')"
check "that pass is of everything, so the guard has it" "$(git rev-parse HEAD)" "$(tail -1 "$CHECKED")"
branch_with sel-docs
echo d >> docs/notes.md && git add . && git commit -qm docs-only
check "a change to documents only builds no app and starts none" "0 0 0" "$(sel) $(sel_has '^app-build') $(sel_has '-p recto-cli --test')"
check "it runs only the cheap guards" "1" "$(sel_has 'nextest run.*--workspace -E test(~guard_one)')"
branch_with sel-ci
echo w >> crates/beta/src/lib.rs && git add . && git commit -qm beta-ci
: > "$CALLS"; rm -f "$CALLS.slow"
CI=true sel_full > "$X/sel.out"; cp "$CALLS" "$X/sel.calls"
has "on CI the same change runs everything" "CI runs everything" "$(cat "$X/sel.out")"
check "every crate" "1" "$(sel_has 'nextest run --locked --no-fail-fast --no-tests=pass --workspace')"
check "and both tiers" "1" "$(sel_has 'app --test restore --test compare --test long_session -j 4$')"
# main runs everything, whatever changed since it: it is what gets pushed.
git checkout -q main
echo mm >> crates/beta/src/lib.rs && git add . && git commit -qm beta-on-main
sel > /dev/null
has "main runs everything too" "main runs everything" "$(cat "$X/sel.out")"
check "and its pass is stamped" "$(git rev-parse HEAD)" "$(tail -1 "$CHECKED")"

# The push guard through a merge: a branch's selective pass doesn't let the same code go up as main.
branch_with sel-merge
echo sm >> crates/beta/src/lib.rs && git add . && git commit -qm sel-merge
sel > /dev/null
git checkout -q main
git merge -q --no-ff --no-edit sel-merge
check "a merge identical to a branch that passed selectively is not accepted by the push guard" "no" "$(hook_accepts)"
out=$(sel_full); rc=$?
check "main then runs everything, for real" "0 true" "$rc $([ "$(wc -l < "$CALLS" | tr -d ' ')" -gt 0 ] && echo true || echo false)"
: > "$CALLS"
check "and then the push guard takes it" "yes" "$(hook_accepts)"

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
