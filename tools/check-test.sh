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
# Everything check.sh and the queue read from the environment, so a run on CI (CI, CHECK_PART, NEXTEST_PROFILE, GITHUB_*) or in a shell of someone's own makes no
# difference: the first full run of the three-part CI layout failed 119 cases because CHECK_PART=core leaked in. A case below runs a slice with them all set.
unset CHECK_ALL CHECK_BASE CHECK_AGAIN CHECK_PART HEAVY_RETRY HEAVY_SOLO HEAVY_PRIORITY HEAVY_CLASS CARGO_BUILD_JOBS CARGO_TARGET_DIR NEXTEST_PROFILE CI TMPDIR_CHECK
# ERE, as macOS's sed has no `\|` in a basic expression: written that way this loop unset nothing, and a run of this inside `check.sh --full` took its
# CHECK_RERUN=0 (2026-11-12), as one on CI would take its 4. What check.sh exports to its children is unset by name too.
unset CHECK_RERUN CHECK_RERUN_TESTS CHECK_RERUN_SECONDS CHECK_SLOW_SEED CHECK_EVIDENCE_ROOT CHECK_LIKE_CI CHECK_LIKE_CI_WRAP CHECK_STAGE CHECK_STAGE_JOBS LIKE_CI_LIMITS
for name in $(env | sed -nE 's/^((GITHUB|NEXTEST|CHECK|PUSH_MAIN|LIKE)_[A-Z0-9_]*)=.*/\1/p'); do [ "$name" = CHECK_TEST_SLICE ] || unset "$name"; done
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
# With STUB_LIST, `nextest list` lists those tests, `binary name` a line, as the real one does with -T oneline.
if [ -n "${STUB_LIST:-}" ] && [[ "$*" == "nextest list "* ]]; then printf '%s\n' "$STUB_LIST"; exit 0; fi
# With STUB_JUNIT, a nextest run writes the JUnit the real one does: a core test, a process test (alpha::proc) and, in the slow stage, the same process test.
if [ -n "${STUB_JUNIT:-}" ] && [[ "$*" == "nextest run "* ]]; then
  mkdir -p target/nextest/default
  printf '<testsuites>\n<testsuite name="alpha">\n<testcase name="t1" classname="alpha" timestamp="2026-11-12T10:00:00.000+01:00" time="0.500"/>\n<testcase name="p1" classname="alpha::proc" timestamp="2026-11-12T10:00:01.000+01:00" time="1.500"/>\n</testsuite>\n</testsuites>\n' > target/nextest/default/junit.xml
fi
# With STUB_JUNIT_FAIL, the nextest run whose line has it fails one test, `flaky_one`, in its JUnit, and leaves its evidence as the harness would. A rerun of
# it (its line names the test by binary_id) fails while fewer than STUB_RERUN_FAILS reruns have, and passes after.
if [ -n "${STUB_JUNIT_FAIL:-}" ] && [[ "$*" == "nextest run "* ]] && [[ "$*" == *"$STUB_JUNIT_FAIL"* ]]; then
  junit="target/nextest/${NEXTEST_PROFILE:-default}/junit.xml"
  mkdir -p "$(dirname "$junit")"
  verdict=fail
  if [[ "$*" == *"binary_id(="* ]]; then
    n=$(( $(cat "$CALLS.reruns" 2>/dev/null || echo 0) + 1 )); echo "$n" > "$CALLS.reruns"
    [ "$n" -le "${STUB_RERUN_FAILS:-0}" ] || verdict=pass
  fi
  if [ "$verdict" = fail ]; then
    printf '<testsuites>\n<testsuite name="recto-cli::app">\n<testcase name="steady_one" classname="recto-cli::app" timestamp="2026-11-12T10:00:00.000+01:00" time="0.5"/>\n<testcase name="flaky_one" classname="recto-cli::app" timestamp="2026-11-12T10:00:00.000+01:00" time="1.0">\n<failure type="test failure">boom</failure>\n</testcase>\n</testsuite>\n</testsuites>\n' > "$junit"
    mkdir -p "$RECTO_EVIDENCE/recto-cli--app--flaky_one" && echo "tools/check.sh --like-ci 'binary_id(=recto-cli::app) & test(=flaky_one)' --jobs ${CHECK_STAGE_JOBS:-}" > "$RECTO_EVIDENCE/recto-cli--app--flaky_one/rerun.txt"
    exit 100
  fi
  printf '<testsuites>\n<testsuite name="recto-cli::app">\n<testcase name="flaky_one" classname="recto-cli::app" timestamp="2026-11-12T10:00:00.000+01:00" time="1.0"/>\n</testsuite>\n</testsuites>\n' > "$junit"
  exit 0
fi
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
# taskpolicy, for --like-ci: says what it was asked, then runs the command after its options.
cat > "$X/bin/taskpolicy" <<'STUB'
#!/usr/bin/env bash
echo "taskpolicy $* profile=${NEXTEST_PROFILE:-}" >> "$CALLS.policy"
while [ $# -gt 0 ] && [ "$1" != cargo ]; do shift; done
exec "$@"
STUB
# gh, for tools/push-main.sh, which check.sh asks about main's CI: runs from STUB_GH_RUNS (none by default), a failed run's jobs, log and artifacts;
# STUB_GH_HANG never answers, STUB_GH_FAIL fails. Every case here uses it, so no case reaches GitHub.
cat > "$X/bin/gh" <<'STUB'
#!/usr/bin/env bash
echo "gh $*" >> "$CALLS.gh"
[ -z "${STUB_GH_HANG:-}" ] || exec sleep 30
[ -z "${STUB_GH_FAIL:-}" ] || exit 1
case "$*" in
  # A commit's own runs when the case set them (STUB_GH_COMMITS/<sha>), for a watch that moves from one commit's run to another's.
  "run list"*"--commit"*) c=$(printf '%s' "$*" | sed -E 's/.*--commit ([0-9a-f]+).*/\1/'); if [ -n "${STUB_GH_COMMITS:-}" ] && [ -f "$STUB_GH_COMMITS/$c" ]; then cat "$STUB_GH_COMMITS/$c"; elif [ -f "$CALLS.gh-rerun" ] && [ -n "${STUB_GH_RUNS_AFTER:-}" ]; then printf '%s\n' "$STUB_GH_RUNS_AFTER"; else printf '%s\n' "${STUB_GH_RUNS:-[]}"; fi ;;
  "run list"*) if [ -f "$CALLS.gh-rerun" ] && [ -n "${STUB_GH_RUNS_AFTER:-}" ]; then printf '%s\n' "$STUB_GH_RUNS_AFTER"; else printf '%s\n' "${STUB_GH_RUNS:-[]}"; fi ;;
  # A run's jobs: name, conclusion, steps, steps failed or cut off, id. By default a part that ran and failed; STUB_GH_JOBS for others.
  "run view"*"--json jobs"*"steps"*) printf "${STUB_GH_JOBS:-check (core)\tsuccess\t15\t0\t1\ncheck (lifecycle)\tfailure\t15\t1\t2\n}" ;;
  "run view"*"--json jobs"*) echo "check (lifecycle)" ;;
  "run rerun"*) touch "$CALLS.gh-rerun"; exit "${STUB_GH_RERUN_EXIT:-0}" ;;
  "api"*"annotations"*) echo "The job was not acquired by Runner of type hosted even after multiple attempts" ;;
  "run view"*"--log-failed"*) printf 'check (lifecycle)\tFull check, this part\t2026-11-12T11:50:03Z \033[31;1m        FAIL\033[0m [   8.3s] (2/2) recto-cli::registrations no_copy\ncheck (lifecycle)\tFull check, this part\t2026-11-12T11:50:04Z check:   recto-cli::registrations no_copy      failed 3 of 5\n' ;;
  "api"*) echo "check-logs-lifecycle-1 4096" ;;
esac
STUB
chmod +x "$X/bin/"*
export PATH="$X/bin:$PATH" CALLS="$X/calls" PUSH_MAIN_GH="$X/bin/gh" PUSH_MAIN_GH_TIMEOUT=2
: > "$CALLS"

git init -q --bare -b main "$X/origin.git"
git init -q -b main "$X/repo"
cd "$X/repo" || exit 1
mkdir -p tools/sysguard tools/hooks app crates/alpha/tests crates/alpha/src crates/beta/src crates/gamma/src docs
cp "$SOURCE/tools/check.sh" tools/check.sh
# The real rules, with this repository's own guards and tiers: alpha has a guard, gamma is what the lifecycle tier is about, beta is a window crate.
cp "$SOURCE/tools/check-scope.sh" tools/check-scope.sh
cp "$SOURCE/tools/check-times.sh" tools/check-times.sh
cp "$SOURCE/tools/check-rerun.sh" tools/check-rerun.sh
cp "$SOURCE/tools/push-main.sh" tools/push-main.sh
cp "$SOURCE/tools/check-like-ci.sh" tools/check-like-ci.sh
echo "fn p1() {}" > crates/alpha/tests/proc.rs
cat >> tools/check-scope.sh <<'SCOPE'
GUARD_TESTS='test(~guard_one) | test(~stale::)'
APP_BINARIES=''
PROCESS_BINARIES='alpha::proc'
LIFECYCLE_TESTS='test(~life_test)'
WINDOW_TESTS="test(~window_test) & !($LIFECYCLE_TESTS)"
WINDOW_CRATES='beta'
LIFECYCLE_PATHS='^app/|^crates/gamma/|^crates/beta/src/record\.rs$'
SCOPE
cp "$SOURCE/tools/sysguard/heavy.sh" tools/sysguard/heavy.sh
cp "$SOURCE/tools/hooks/pre-push" tools/hooks/pre-push
printf '#!/usr/bin/env bash\n' > tools/pdfium.sh
# The ratchet has its own test (tools/ratchet-test.sh); here it, and the tools' tests, only log that they ran, and STUB_FAIL can fail them.
for tool in ratchet ratchet-test check-test; do
  printf '#!/usr/bin/env bash\necho %s >> "$CALLS"\n[ "${STUB_FAIL:-}" != %s ]\n' "$tool" "$tool" > "tools/$tool.sh"
  chmod +x "tools/$tool.sh"
done
printf '#!/usr/bin/env bash\necho app-build >> "$CALLS"\n[ "${STUB_FAIL:-}" != app-build ]\n' > app/build.sh
chmod +x tools/pdfium.sh tools/ratchet.sh app/build.sh
printf 'mod stale;\nfn guard_one() {}\n' > crates/alpha/src/lib.rs
echo one > crates/beta/src/lib.rs
echo "fn life_test() {}" > crates/gamma/src/lib.rs
echo "fn window_test() {}" >> crates/beta/src/lib.rs
echo notes > docs/notes.md
# As in the real repository: what the check and its tests write under target/ (JUnit, the evidence of failed tests) is never part of the tree.
echo "target/" > .gitignore
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
check "every stage runs, cheapest and likeliest first" "ratchet fmt headings-fmt clippy headings-clippy windows workers headings-build headings-tests changed-tests other-tests app-build app-bundle app-tests slow registrations" "$(tr '\n' ' ' < "$X/order" | sed 's/ $//')"
has "each stage says how long it took" "check: app-tests took" "$out"
# Failure: the tests depend on the shell they run in. The same script's first cases, with the variables CI sets (and a part, and a slow switch), must pass.
if [ -z "${CHECK_TEST_SLICE:-}" ]; then
  slice=$(CI=true CHECK_PART=core CHECK_ALL=1 CHECK_BASE=HEAD NEXTEST_PROFILE=ci GITHUB_ACTIONS=true RECTO_QA_SLOW=ci CHECK_TEST_SLICE=1 bash "$SOURCE/tools/check-test.sh" 2>&1)
  check "the cases pass with CI's variables set (CI, CHECK_PART, NEXTEST_PROFILE, GITHUB_*)" "0" "$(printf '%s\n' "$slice" | grep -c '^FAIL')"
  has "and that slice did run" "ok    every stage runs" "$slice"
else
  echo "slice done"; exit 0
fi
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
check "it stops at the first failing stage, after the ratchet and the layout's two" "4" "$(ran)"
check "a clippy error is the compile gate's: no run is in the ledger" "0" "$(runs_in_ledger)"
has "and it says no test was queued" "no test was queued" "$out"
check "a failure isn't remembered as a pass" "no-passes" "$([ -s "$PASSES" ] && echo has-passes || echo no-passes)"
check "nor given to the guard" "no-stamp" "$([ -s "$CHECKED" ] && echo has-stamp || echo no-stamp)"

# Failure: the queue's refusal of a repeat counted as a run nobody got.
before=$(runs_in_ledger)
out=$(STUB_FAIL=clippy STUB_EXIT=101 full); rc=$?
check "the queue refuses the same failure, with its exit" "101" "$rc"
has "and shows why" "heavy: refused" "$out"
check "and nothing ran but the ratchet and the layout's two checks, which build nothing" "3" "$(ran)"
check "and it costs no budget" "$before" "$(runs_in_ledger)"

# Failure (review V1): code laid out by hand reaches clippy and the tests, minutes spent before the cheapest check says no.
fresh_ledger
commit_code fmt-fails
out=$(STUB_FAIL="fmt --all --check" STUB_EXIT=1 full); rc=$?
check "a layout off stops the run first, after the ratchet" "1 2" "$rc $(ran)"
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
# Failure: structure that grew found only after the queue, or a tool's own test run by hand and forgotten. The ratchet runs first in every mode; the tools'
# tests run in a full run when something under tools/ changed, before the gate, and a failure there queues nothing.
git checkout -q main
git checkout -qb tools-1
fresh_ledger
echo g >> crates/gamma/src/lib.rs && git add . && git commit -qm gamma-for-ratchet
out=$(STUB_FAIL=ratchet STUB_EXIT=1 tools/check.sh --quick 2>&1); rc=$?
check "a grown structure stops the quick check before anything" "1 1" "$rc $(ran)"
out=$(STUB_FAIL=ratchet full); rc=$?
check "and the full one, with nothing queued" "1 1" "$rc $(ran)"
sel > /dev/null
check "a change outside tools/ runs neither of the tools' tests" "0" "$(sel_has 'ratchet-test\|check-test')"
echo t > tools/note.txt && git add . && git commit -qm tools-change-2
fresh_ledger
sel > /dev/null
check "a change under tools/ runs the ratchet's test and this one" "1 1" "$(sel_has '^ratchet-test') $(sel_has '^check-test')"
has "and says how long" "the tools' tests took" "$(cat "$X/sel.out")"
fresh_ledger
: > "$CALLS"
out=$(STUB_FAIL=check-test STUB_EXIT=1 sel_full); rc=$?
check "a failing tool test stops the full run before the gate" "1 true" "$rc $([ "$(grep -c 'clippy' "$CALLS")" = 0 ] && echo true || echo false)"
has "naming it" "tools/check-test.sh failed" "$out"
: > "$CALLS"
git checkout -q main

branch_with sel-window
echo w >> crates/beta/src/lib.rs && git add . && git commit -qm beta-only
check "a change to a window crate passes selectively" "0" "$(sel)"
has "the start says what was selected" "selective" "$(cat "$X/sel.out")"
has "and what was skipped" "skipped: lifecycle tier" "$(cat "$X/sel.out")"
check "the window tier runs" "1" "$(sel_has 'app --test restore --test compare --test evidence -j 4 -E test(~window_test)')"
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
check "a change to app/ runs both tiers" "1" "$(sel_has 'app --test restore --test compare --test long_session --test evidence -j 4$')"
check "but no crate's tests, the guards only" "0" "$(sel_has 'package(=')"
branch_with sel-tools
echo t > tools/note.txt && git add . && git commit -qm tools
sel > /dev/null
has "a change to the tools says why it runs everything" "everything runs" "$(cat "$X/sel.out")"
check "it runs every crate's tests" "1" "$(sel_has 'nextest run --locked --no-fail-fast --no-tests=pass --workspace')"
check "and both app tiers" "1" "$(sel_has 'app --test restore --test compare --test long_session --test evidence -j 4$')"
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
check "and both tiers" "1" "$(sel_has 'app --test restore --test compare --test long_session --test evidence -j 4$')"
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

# Failure: a selective run on a dirty tree escaping the budget, so a branch could be run again and again unseen. It is in the ledger as dirty, is not
# remembered, and counts like any real run (decided: a run is a run, dirty or not).
branch_with sel-dirty
echo d1 >> crates/gamma/src/lib.rs && git add . && git commit -qm d1
echo d2 >> crates/gamma/src/lib.rs
sel > /dev/null
sel > /dev/null
check "dirty selective runs are in the ledger as dirty" "2 +dirty" "$(runs_in_ledger) $(last_ledger_field 3 | grep -o '+dirty')"
check "they count against the budget of two" "2" "$(tools/check.sh --runs | sed -n 's/^sel-dirty: \([0-9]*\) of.*/\1/p')"
check "so a third needs a reason" "3" "$(sel)"
git checkout -q -- crates/gamma/src/lib.rs

# --- Test durations (tools/check-times.sh) and named parts of a full run ---------------------------------------------------------------------------------
TIMES=.git/recto-test-times
branch_with times
echo t1 >> crates/alpha/src/lib.rs && git add . && git commit -qm times-1
# Failure: durations lost between stages (nextest overwrites its JUnit), or not kept at all, so a process review has nothing to read.
check "no durations before any run" "0" "$(ls $TIMES 2>/dev/null | wc -l | tr -d ' ')"
STUB_JUNIT=1 full > /dev/null
check "a full run keeps durations in one file" "1" "$(ls $TIMES | grep -c 'tsv$')"
f=$(ls $TIMES/*.tsv | head -1)
check "the file says commit, scope, part and result" "run	$(git rev-parse HEAD)	all	all	pass" "$(awk -F'\t' '$1 == "run" { print $1 "\t" $2 "\t" $4 "\t" $5 "\t" $6 }' "$f")"
check "it has the wall time of the stages" "1" "$(grep -c '^stage	other-tests	' "$f")"
check "a core test is in tier 1" "tier	1" "$(grep '^tier	1	' "$f" | cut -f1,2)"
# Each stage that ran nextest wrote the same two tests: they count once each, however many stages' JUnit holds them, and the slow stage's copies apart.
check "a test in several stages' JUnit counts once" "1 1" "$(grep '^tier	1	' "$f" | cut -f3) $(grep '^tier	2	' "$f" | cut -f3)"
check "the slow stage's tests are kept apart from the tiers" "slow	2" "$(grep '^slow	' "$f" | cut -f1,2)"
check "the slowest tests are listed with their binary and seconds" "true" "$([ "$(grep -c '^test	2	[a-z-]*	alpha::proc	1.50	p1$' "$f")" -ge 1 ] && echo true || echo false)"
check "and the full run is on the ledger as before" "pass" "$(last_ledger_field 4)"
check "no scratch files are left in the repository's git dir" "0" "$(ls .git | grep -c 'part$')"
# Failure: the folder grows for ever (a file a run, each with a few dozen lines), or the newest are the ones deleted.
for n in $(seq 10 33); do printf 'run\told%s\t2020-01-01 00:00\tall\tall\tpass\n' "$n" > "$TIMES/202001010000$n-old$n.tsv"; done
commit_code times-2
STUB_JUNIT=1 full > /dev/null
check "only the newest 20 runs are kept" "20" "$(ls $TIMES | grep -c 'tsv$')"
check "the run just made is among them" "1" "$(grep -l "$(git rev-parse HEAD)" $TIMES/*.tsv | wc -l | tr -d ' ')"
check "and the oldest are the ones gone, the newer old ones kept" "0 1" "$(ls $TIMES | grep -c 'old10\.\|old15\.') $(ls $TIMES | grep -c 'old33\.')"
# Failure: a run stopped from outside, or one with no JUnit (a stub, a refusal), leaves a file that says nothing.
rm -rf $TIMES
commit_code times-3
full > /dev/null
check "a run that wrote no JUnit keeps nothing" "0" "$(ls $TIMES 2>/dev/null | wc -l | tr -d ' ')"

# Failure: --slow cannot say which tier is over its budget, or hides it.
rm -rf $TIMES; mkdir -p $TIMES
printf 'run\taaaaaaaaaaaaaaaa\t2026-11-12 10:00\tall\tall\tpass\nstage\tother-tests\t100\nstage\tapp-tests\t300\nstage\tslow\t120\ntier\t1\t50\t30.0\t20.0\ntier\t2\t10\t90.0\t50.0\ntier\t3\t40\t500.0\t200.0\ntier\t4\t8\t300.0\t200.0\nslow\t25\t400.0\ntest\t3\tapp-tests\trecto-cli::app\t42.10\tdrawing_is_slow\n' > $TIMES/20261112T100000-aaaaaaaaaaaa.tsv
printf 'run\tbbbbbbbbbbbbbbbb\t2026-11-12 11:00\tall\tall\tpass\nstage\tother-tests\t120\nstage\tapp-tests\t290\nstage\tslow\t120\ntier\t1\t50\t30.0\t20.0\ntier\t2\t10\t90.0\t50.0\ntier\t3\t40\t500.0\t130.0\ntier\t4\t0\t0.0\t0.0\nslow\t25\t400.0\ntest\t3\tapp-tests\trecto-cli::app\t42.10\tdrawing_is_slow\n' > $TIMES/20261112T110000-bbbbbbbbbbbb.tsv
out=$(tools/check.sh --slow 2>&1); rc=$?
check "--slow exits 0 even over budget" "0" "$rc"
has "it lists the slowest tests with seconds and binary" "42.1 s  3     recto-cli::app  drawing_is_slow" "$out"
has "a tier over its budget is named, with by how much" "wall 130 s: OVER by 40 s" "$out"
check "a tier within it is not" "0" "$(printf '%s\n' "$out" | grep -E '^  (1|2) ' | grep -c OVER)"
has "the tier that wasn't in the run says so" "(no tests in this run)" "$out"
has "the slow stage's total is there" "25 tests, 400 test-seconds, 120 s wall" "$out"
# Failure: two tiers that share one nextest run each get a wall that overlaps the other's, so neither number is true. The pair gets the run's wall, once, against both budgets.
printf 'run\tdddddddddddddddd\t2026-11-12 13:00\tall\tall\tpass\nstage\tapp-tests\t400\ntier\t3\t40\t500.0\t380.0\ntier\t4\t8\t300.0\t390.0\n' > $TIMES/20261112T130000-dddddddddddd.tsv
out=$(tools/check.sh --slow 2>&1)
has "tiers sharing a run are given the run's wall against both budgets added" "tiers 3+4 (window + lifecycle): 400 s wall against 330 s (90 + 240), OVER by 70 s" "$out"
has "each with its own test count and test-seconds, and a note on the overlap" "shares its run with tier 3" "$out"
has "and says CI gives the per-tier wall" "CI runs them as separate jobs" "$out"
rm -f $TIMES/20261112T130000-dddddddddddd.tsv
# The comparison needs the run of everything before the latest; it shows the change per stage.
printf 'run\tbbbbbbbbbbbbbbbb\t2026-11-12 11:00\tall\tall\tpass\nstage\tother-tests\t120\nstage\tapp-tests\t290\ntier\t1\t1\t1.0\t1.0\n' > $TIMES/20261112T110000-bbbbbbbbbbbb.tsv
out=$(tools/check.sh --slow 2>&1)
has "the latest run of everything is compared with the one before: slower" "other-tests 120 100 +20" "$(printf '%s' "$out" | tr -s ' ')"
has "and faster" "app-tests 290 300 -10" "$(printf '%s' "$out" | tr -s ' ')"
# A selective run or a part is newer but isn't a run of everything, so it isn't what the comparison goes by.
printf 'run\tcccccccccccccccc\t2026-11-12 12:00\tselective\tall\tpass\nstage\tother-tests\t5\n' > $TIMES/20261112T120000-cccccccccccc.tsv
has "a selective run is shown first but isn't compared" "Latest kept run: commit cccccccccccc" "$(tools/check.sh --slow 2>&1)"
has "and the comparison skips it" "(bbbbbbbbbbbb against aaaaaaaaaaaa)" "$(tools/check.sh --slow 2>&1)"
rm -rf $TIMES
has "with nothing kept it says where they will be" "No test durations kept yet" "$(tools/check.sh --slow 2>&1)"

# Failure: the tiers' lists drift from the tests, or a binary renamed drops its tests into tier 1 unseen.
mv crates/alpha/tests/proc.rs crates/alpha/tests/proc2.rs
: > "$CALLS"
out=$(full); rc=$?
check "a process binary that isn't there stops the check before anything runs" "2 0" "$rc $(ran)"
has "and names it" "process binary alpha::proc" "$out"
mv crates/alpha/tests/proc2.rs crates/alpha/tests/proc.rs
: > "$CALLS"
# The tier of an app test comes from nextest's own filterset: the lifecycle ones are named by it, the rest of the app binaries are the window's.
. tools/check-scope.sh; . tools/check-times.sh
printf 'recto-cli::restore update::an_update\n' > "$X/life"
got=$(printf 'app-tests\t2.000\t100.000\t102.000\trecto-cli::restore\tupdate::an_update\napp-tests\t3.000\t100.000\t103.000\trecto-cli::app\ta_window_test\nother-tests\t0.100\t50.000\t50.100\trecto-cli::socket\ts\nother-tests\t0.100\t50.000\t50.100\trecto-core\tc\nslow\t9.000\t200.000\t209.000\trecto-cli::app\ta_window_test\n' |
  APP_BINARIES='app restore' PROCESS_BINARIES='recto-cli::socket' times_summarise "$X/life" | grep -E '^(tier|slow)' | awk -F'\t' '{ printf "%s%s=%s ", $1, $2, $3 }')
check "tests are put in their tier, the slow stage's apart" "tier1=1 tier2=1 tier3=1 tier4=1 slow9.0=9.0 " "$(printf '%s' "$got" | sed 's/slow1=/slow9.0=/')"

# Failure: a test that is in two stages' JUnit counted twice (the app tests are in the workspace stage too, where they skip in no time), which doubled the
# window and lifecycle test counts and put their time in the wrong stage; or the slow stage's late copies added into a tier.
printf 'recto-cli::restore update::an_update\n' > "$X/life2"
got=$(printf 'other-tests\t0.010\t10.000\t10.010\trecto-cli::app\ta_window_test\nother-tests\t0.010\t10.000\t10.010\trecto-cli::restore\tupdate::an_update\nother-tests\t0.100\t10.000\t10.100\trecto-core\tc\napp-tests\t3.000\t100.000\t103.000\trecto-cli::app\ta_window_test\napp-tests\t2.000\t100.000\t102.000\trecto-cli::restore\tupdate::an_update\nslow\t9.000\t200.000\t209.000\trecto-cli::app\ta_window_test\nslow\t8.000\t200.000\t208.000\trecto-cli::restore\tupdate::an_update\n' |
  APP_BINARIES='app restore' PROCESS_BINARIES='' times_summarise "$X/life2")
tiers_of() { printf '%s\n' "$got" | awk -F'\t' '$1 == "tier" && $3 > 0 { printf "%s:%s:%s ", $2, $3, $4 }'; }
check "a test in two stages counts once, with the time of the stage where it ran" "1:1:0.1 3:1:3.0 4:1:2.0 " "$(tiers_of)"
check "the slow stage is kept apart and never added into a tier" "slow 2 17.0" "$(printf '%s\n' "$got" | awk -F'\t' '$1 == "slow" { print "slow " $2, $3 }')"
check "and the slowest list names each test once" "1" "$(printf '%s\n' "$got" | grep -c '^test	3	.*	a_window_test$')"

# Failure: a part of a full run does the other part's work, or is remembered as a pass of everything, or CI's jobs overlap in what they run.
branch_with parts
echo p >> crates/alpha/src/lib.rs && git add . && git commit -qm parts-1
check "a named part that doesn't exist is refused" "2" "$(CHECK_PART=bogus CHECK_ALL=1 tools/check.sh --full > /dev/null 2>&1; echo $?)"
check "and a part with --quick" "2" "$(CHECK_PART=core tools/check.sh --quick > /dev/null 2>&1; echo $?)"
: > "$CALLS"
CHECK_PART=core full > "$X/part.out"
check "core has no tools' tests when nothing under tools/ changed and it isn't CI" "0" "$(grep -c '^check-test$' "$CALLS")"
: > "$CALLS"
CI=true CHECK_PART=core CHECK_ALL=1 GITHUB_REF_NAME=main GITHUB_EVENT_NAME=workflow_dispatch tools/check.sh --full > /dev/null 2>&1
check "on CI with no base to compare with, the tools' own tests run, in core" "1 1" "$(grep -c '^check-test$' "$CALLS") $(grep -c '^ratchet-test$' "$CALLS")"
check "the gate runs once in core" "1" "$(grep -c '^clippy --workspace --all-targets' "$CALLS")"
: > "$CALLS"
CI=true CHECK_PART=window CHECK_ALL=1 GITHUB_REF_NAME=main GITHUB_EVENT_NAME=workflow_dispatch tools/check.sh --full > /dev/null 2>&1
check "and not in an app part" "0" "$(grep -c '^check-test$' "$CALLS")"
: > "$CALLS"
CHECK_PART=core full > /dev/null
check "core runs the gate and the workspace" "1 1" "$(grep -c '^clippy --workspace --all-targets' "$CALLS") $(grep -c -- '--no-tests=pass --workspace\|--no-tests=pass -p\|--no-tests=pass' "$CALLS")"
check "but no app" "0" "$(grep -c 'app-build\|--test restore' "$CALLS")"
: > "$CALLS"
CHECK_PART=window full > /dev/null
check "window builds the app and runs the window tier only" "1 1 0" "$(grep -c '^app-build$' "$CALLS") $(grep -c -- '--test compare --test evidence -j 4 -E test(~window_test)' "$CALLS") $(grep -c 'long_session' "$CALLS")"
check "with the notices, and no gate or workspace tests" "1 0 0" "$(grep -c -- '--test notices' "$CALLS") $(grep -c '^clippy' "$CALLS") $(grep -c -- '--workspace' "$CALLS")"
check "and no tools' tests" "0" "$(grep -c '^check-test$' "$CALLS")"
check "and no registrations" "0" "$(grep -c -- '--test registrations' "$CALLS")"
: > "$CALLS"
CHECK_PART=lifecycle full > /dev/null
check "lifecycle runs the lifecycle tier and the registrations" "1 1 0" "$(grep -c 'long_session -j 4 -E test(~life_test)' "$CALLS") $(grep -c -- '--test registrations' "$CALLS") $(grep -c 'window_test' "$CALLS")"
check "and not the notices" "0" "$(grep -c -- '--test notices' "$CALLS")"
check "no part is a ledger line, a pass, or a stamp for the push guard" "0 0 0" "$(runs_in_ledger) $([ -f "$PASSES" ] && wc -l < "$PASSES" | tr -d ' ' || echo 0) $([ -f "$CHECKED" ] && wc -l < "$CHECKED" | tr -d ' ' || echo 0)"
: > "$CALLS"
CHECK_PART=window full > /dev/null
check "a part runs again on the same code: it is never remembered" "true" "$([ "$(wc -l < "$CALLS" | tr -d ' ')" -gt 0 ] && echo true || echo false)"
: > "$CALLS"
# The three parts are what a run of everything is: the same commands, except that the app tests are split by tier (the window part's filter and the
# lifecycle part's, which together are every test in the binaries) and the slow stage is split the same way.
full > /dev/null
all_calls=$(grep -vE -- '--test app --test restore' "$CALLS" | sort -u)
: > "$CALLS"
for p in core window lifecycle; do CHECK_PART=$p CHECK_ALL=1 tools/check.sh --full > /dev/null 2>&1; done
parts_calls=$(grep -vE -- '--test app --test restore' "$CALLS" | sort -u)
check "the parts together run what a run of everything does" "$all_calls" "$parts_calls"
check "and the app tests are in the parts as two filters, not the unfiltered whole" "1 1 0" "$(grep -c -- '--test compare --test evidence -j 4 -E test(~window_test)' "$CALLS") $(grep -c -- 'long_session -j 4 -E test(~life_test)' "$CALLS") $(grep -c -- 'long_session --test evidence -j 4$' "$CALLS")"

# Failure: CI ran the tools' own tests on every run, 173 s of core whatever changed (run 43195226598), or skips them when what they test changed since CI's
# base, or when that base can't be worked out. The base: on main the commit the push came after, on a branch where it left origin/main. What they test is
# tools/ (the hooks too), .config/ and .github/, here and on CI alike.
git checkout -q main
git push -q origin main
ci_core() { : > "$CALLS"; env CI=true CHECK_PART=core "$@" tools/check.sh --full > "$X/ci.out" 2>&1; echo "$(grep -c '^check-test$' "$CALLS") $(grep -c '^ratchet-test$' "$CALLS")"; }
branch_ci() { ci_core GITHUB_REF_NAME="$(git rev-parse --abbrev-ref HEAD)" GITHUB_EVENT_NAME=push; }
git checkout -qb ci-crate
echo cc >> crates/alpha/src/lib.rs && git add . && git commit -qm ci-crate
check "on CI, a branch that changed no tool since origin/main skips the tools' tests" "0 0" "$(branch_ci)"
has "and says why" "the tools' tests are skipped: nothing under tools/, .config/ or .github/ changed since $(git merge-base origin/main HEAD | cut -c1-12)" "$(cat "$X/ci.out")"
check "but still runs the rest of core" "1 1" "$(grep -c '^clippy --workspace --all-targets' "$CALLS") $(grep -c -- '--no-tests=pass --workspace' "$CALLS")"
for changed in tools/hooks/pre-push .config/nextest.toml .github/workflows/ci.yml; do
  git checkout -q main; git checkout -qb "ci-$(basename "$changed")"
  mkdir -p "$(dirname "$changed")" && echo "# $changed" >> "$changed" && git add . && git commit -qm "ci $changed"
  echo cc >> crates/alpha/src/lib.rs && git add . && git commit -qm "ci crate after $changed"
  check "on CI, a branch that changed $changed runs them, whatever came after" "1 1" "$(branch_ci)"
  has "naming it" "the tools' tests run: $changed changed since" "$(cat "$X/ci.out")"
done
check "and here, off CI, a branch that changed .github/ runs them too, by the same rule" "1 1" "$(: > "$CALLS"; sel_full > /dev/null; echo "$(grep -c '^check-test$' "$CALLS") $(grep -c '^ratchet-test$' "$CALLS")")"
git checkout -q ci-crate
git update-ref -d refs/remotes/origin/main
check "a branch with no origin/main to compare with runs them" "1 1" "$(branch_ci)"
has "saying why" "CI's base couldn't be worked out" "$(cat "$X/ci.out")"
git fetch -q origin
# main: what came since the push before, which the event names.
git checkout -q main
before=$(git rev-parse HEAD)
echo cm >> crates/alpha/src/lib.rs && git add . && git commit -qm ci-main-crate
event() { printf '{"before":"%s"}\n' "$1" > "$X/event.json"; echo "$X/event.json"; }
on_main() { ci_core GITHUB_REF_NAME=main GITHUB_EVENT_NAME=push GITHUB_EVENT_PATH="$(event "$1")"; }
check "on main, a push with no tool changed since the one before skips them" "0 0" "$(on_main "$before")"
echo "# t" >> tools/note.txt && git add . && git commit -qm ci-main-tools
echo cm >> crates/alpha/src/lib.rs && git add . && git commit -qm ci-main-crate-2
check "a push with a tool changed in any of its commits runs them" "1 1" "$(on_main "$before")"
check "a push with only the last commit after the tools' change skips them" "0 0" "$(on_main "$(git rev-parse HEAD~1)")"
check "a first push (before is all zeros) runs them" "1 1" "$(on_main 0000000000000000000000000000000000000000)"
check "a before that isn't a commit here runs them" "1 1" "$(on_main 1234567890123456789012345678901234567890)"
check "a before that isn't in HEAD's history runs them" "1 1" "$(on_main "$(git rev-parse ci-crate)")"
check "an event that isn't a push runs them" "1 1" "$(ci_core GITHUB_EVENT_NAME=workflow_dispatch GITHUB_REF_NAME=main GITHUB_EVENT_PATH="$(event "$before")")"
check "an event file that isn't JSON runs them" "1 1" "$(echo 'not json' > "$X/bad.json"; ci_core GITHUB_REF_NAME=main GITHUB_EVENT_NAME=push GITHUB_EVENT_PATH="$X/bad.json")"
check "and CHECK_BASE still decides when it's given" "0 0" "$(ci_core CHECK_BASE=HEAD GITHUB_REF_NAME=main GITHUB_EVENT_NAME=workflow_dispatch)"

# Failure (PLAN R0.7b): the slow stage's seed can't be given, so an order of arrivals it found can't be replayed; or a seed is drawn when none was asked.
commit_code slow-seed
rm -f "$CALLS.slow"
out=$(CHECK_SLOW_SEED=4242 full --why "seed"); rc=$?
has "a seed asked for goes to the slow stage as the profile's jitter" "ci,jitter=4242 nextest run" "$(cat "$CALLS.slow")"
has "and is said" "check: slow: RECTO_QA_SLOW=ci,jitter=4242" "$out"
check "a seed that isn't a number is refused" "2" "$(CHECK_SLOW_SEED=x full --why "bad seed" > /dev/null; echo $?)"
rm -f "$CALLS.slow"
# Failure (PLAN R0.7c): a failed test on CI says only that it failed, and its rate is learned by rerunning the job by hand; or a failed app tier hides the
# registrations stage after it, whose result is evidence about leaks; or the reruns turn a failure into a pass, or run here when nobody asked.
git checkout -q main
git checkout -qb rerun-cases
commit_code rerun-base
: > "$CALLS"; rm -f "$CALLS.reruns"
out=$(STUB_JUNIT_FAIL="--test app" full --why "rerun: local"); rc=$?
check "a failed app tier still fails the run" "1" "$([ "$rc" -ne 0 ] && echo 1 || echo 0)"
check "and the ledger names the stage that failed, not the one after" "fail:app-tests" "$(last_ledger_field 4)"
check "the registrations stage runs after a failed app tier" "1" "$(grep -c -- '--test registrations' "$CALLS")"
check "the slow stage doesn't, as it's the same tests late" "0" "$(grep -c -- '--test app --test restore -j 4 -E' "$CALLS")"
check "here, nothing is run again unless asked" "0" "$(grep -c 'binary_id(=' "$CALLS")"
has "the failure says where its evidence is" "evidence of 1 failed test run(s), one folder each, in $PWD/target/recto-evidence" "$out"
has "with the command that runs it again" "run it again here as it ran: tools/check.sh --like-ci 'binary_id(=recto-cli::app) & test(=flaky_one)' --jobs 4" "$out"
: > "$CALLS"; rm -f "$CALLS.reruns"
commit_code rerun-asked
out=$(CHECK_RERUN=4 STUB_RERUN_FAILS=1 STUB_JUNIT_FAIL="--test app" full --why "rerun: asked"); rc=$?
check "asked, the failed test alone runs again that many times" "4" "$(grep -c 'binary_id(=recto-cli::app) & test(=flaky_one)' "$CALLS")"
check "and only it: the test that passed isn't named" "0" "$(grep 'binary_id(=' "$CALLS" | grep -c steady_one)"
check "with the stage's arguments but its own filter" "4" "$(grep 'binary_id(=' "$CALLS" | grep -c -- '-p recto-cli --test app --test restore --test compare --test long_session --test evidence -j 4 -E')"
has "the table gives the rate, counting the first run" "recto-cli::app flaky_one" "$out"
has "as N of M" "failed 2 of 5" "$(printf '%s' "$out" | tr -s ' ')"
check "and the run still fails" "1" "$([ "$rc" -ne 0 ] && echo 1 || echo 0)"
check "each round keeps its own evidence, and the first run's is there too" "yes yes yes" "$([ -f target/recto-evidence/app-tests/recto-cli--app--flaky_one/rerun.txt ] && echo yes) $([ -f target/recto-evidence/app-tests-rerun-1/recto-cli--app--flaky_one/rerun.txt ] && echo yes) $([ -f target/recto-evidence/app-tests-reruns.txt ] && echo yes)"
: > "$CALLS"; rm -f "$CALLS.reruns"
commit_code rerun-budget
out=$(CHECK_RERUN_SECONDS=0 CHECK_RERUN=4 STUB_JUNIT_FAIL="--test app" full --why "rerun: budget"); rc=$?
check "the reruns stop at their time bound" "0" "$(grep -c 'binary_id(=' "$CALLS")"
has "and say so" "reruns stopped after 0 of 4 rounds" "$out"
has "the table then counts the first run only" "failed 1 of 1" "$(printf '%s' "$out" | tr -s ' ')"
check "and what the last run left is gone: no round's evidence from the run before" "no" "$([ -d target/recto-evidence/app-tests-rerun-1 ] && echo yes || echo no)"
: > "$CALLS"; rm -f "$CALLS.reruns"
commit_code rerun-ci
STUB_RERUN_FAILS=9 STUB_JUNIT_FAIL="--test app" CI=true CHECK_PART=lifecycle tools/check.sh --full > "$X/ci.out" 2>&1; rc=$?
check "on CI a failed test runs again 4 times without being asked" "4" "$(grep -c 'binary_id(=' "$CALLS")"
has "failing every time, it says so" "failed 5 of 5" "$(tr -s ' ' < "$X/ci.out")"
check "and in the lifecycle part the registrations stage still runs" "1" "$(grep -c -- '--test registrations' "$CALLS")"
check "and the part fails" "1" "$([ "$rc" -ne 0 ] && echo 1 || echo 0)"
mkdir -p "$X/given"; echo keep > "$X/given/mine"
: > "$CALLS"
commit_code rerun-given
RECTO_EVIDENCE="$X/given" STUB_JUNIT_FAIL="--test app" CI=true CHECK_RERUN=0 CHECK_PART=window tools/check.sh --full > "$X/ci.out" 2>&1
check "a root given in RECTO_EVIDENCE (CI's) is never emptied, and gets the evidence" "keep yes" "$(cat "$X/given/mine") $([ -f "$X/given/app-tests/recto-cli--app--flaky_one/rerun.txt" ] && echo yes)"
check "the reruns' junit helpers find the failed test" "recto-cli::app	flaky_one" "$(printf '<testcase name="a" classname="x"/>\n<testcase name="flaky_one" classname="recto-cli::app" time="1">\n<failure type="t">b</failure>\n</testcase>\n<testcase name="c" classname="x">\n</testcase>\n' > "$X/j.xml"; . tools/check-rerun.sh; junit_failed "$X/j.xml")"
check "and take a stage's filter off its arguments, keeping the rest" "--locked -p recto-cli -j 4" "$(. tools/check-rerun.sh; args_without_filter --locked -p recto-cli -j 4 -E 'test(x)' --no-tests=pass | tr '\0' ' ' | sed 's/ $//')"

# Failure (PLAN R0.7e): main's CI is red and nothing says so, so the next merge is checked, stamped and pushed over it (four times by 2026-11-12); or a
# red marked known never ends; or gh missing, signed out or offline hangs the check or stops the stamp; or a branch's check asks GitHub at all.
git checkout -q main
git push -q origin main
pushed=$(git rev-parse origin/main)
run_of() { printf '[{"conclusion":"%s","databaseId":%s,"headSha":"%s","status":"%s","url":"https://ci/%s"}]' "$1" "$2" "$3" "${4:-completed}" "$2"; }
red=$(run_of failure 7 "$pushed")
commit_code after-red
out=$(STUB_GH_RUNS=$red full --why "red main"); rc=$?
check "on main, a pass while the last pushed commit's CI is red isn't stamped" "4 no" "$rc $(grep -qx "$(git rev-parse HEAD)" "$CHECKED" && echo yes || echo no)"
has "and says why, with the run" "not stamped for the push guard: main's CI is red on $(printf '%s' "$pushed" | cut -c1-12) (https://ci/7)" "$out"
check "the pass is still remembered" "pass" "$(last_ledger_field 4)"
check "so the push guard refuses it" "no" "$(hook_accepts)"
: > "$CALLS"
out=$(STUB_GH_RUNS=$red full); rc=$?
check "the same code again runs nothing and still doesn't stamp" "4 0 no" "$rc $(ran) $(grep -qx "$(git rev-parse HEAD)" "$CHECKED" && echo yes || echo no)"
check "a known red can't be marked past a week" "2" "$(tools/push-main.sh --known "$pushed" --until "$(date -j -v+8d '+%Y-%m-%d')" --why "x" > /dev/null 2>&1; echo $?)"
check "nor for a day gone" "2" "$(tools/push-main.sh --known "$pushed" --until "$(date -j -v-1d '+%Y-%m-%d')" --why "x" > /dev/null 2>&1; echo $?)"
check "nor without saying what is known" "2" "$(tools/push-main.sh --known "$pushed" --until "$(date -j -v+2d '+%Y-%m-%d')" > /dev/null 2>&1; echo $?)"
tools/push-main.sh --known "$(printf '%s' "$pushed" | cut -c1-12)" --until "$(date -j -v+2d '+%Y-%m-%d')" --why "the contents pane, tracked in PLAN" > /dev/null
out=$(STUB_GH_RUNS=$red full); rc=$?
check "marked known with a date, it stamps" "0 yes" "$rc $(grep -qx "$(git rev-parse HEAD)" "$CHECKED" && echo yes || echo no)"
has "and says it's red and known" "marked known until $(date -j -v+2d '+%Y-%m-%d'): the contents pane, tracked in PLAN" "$out"
printf '%s\t%s\t%s\t%s\n' "$pushed" "$(date -j -v-1d '+%Y-%m-%d')" "old" "x" > .git/recto-ci-known
commit_code after-known
out=$(STUB_GH_RUNS=$red full --why "due mark"); rc=$?
check "a mark past its date doesn't count" "4" "$rc"
rm -f .git/recto-ci-known
commit_code after-green
out=$(STUB_GH_RUNS="$(run_of success 8 "$pushed")" full --why "green main"); rc=$?
check "green, it stamps" "0 yes" "$rc $(grep -qx "$(git rev-parse HEAD)" "$CHECKED" && echo yes || echo no)"
has "and says so" "main's CI is green on" "$out"
commit_code after-running
out=$(STUB_GH_RUNS="$(run_of "" 9 "$pushed" in_progress)" full --why "running main"); rc=$?
check "still running, it stamps, and says so" "0 yes" "$rc $(printf '%s' "$out" | grep -q "still running on" && echo yes)"
commit_code after-other
out=$(STUB_GH_RUNS="$(run_of failure 10 1234567890123456789012345678901234567890)" full --why "unrelated red"); rc=$?
check "a red run of a commit that isn't in main's history doesn't count" "0" "$rc"
commit_code after-cancelled
out=$(STUB_GH_RUNS="[{\"conclusion\":\"cancelled\",\"databaseId\":11,\"headSha\":\"$pushed\",\"status\":\"completed\",\"url\":\"u\"},$(run_of failure 12 "$pushed" | tr -d '[]')]" full --why "cancelled then red"); rc=$?
check "a cancelled run is passed over for the one before it" "4" "$rc"
commit_code no-gh
out=$(PUSH_MAIN_GH=/nonexistent/gh full --why "no gh"); rc=$?
check "with no gh it stamps" "0 yes" "$rc $(grep -qx "$(git rev-parse HEAD)" "$CHECKED" && echo yes || echo no)"
has "with a plain warning" "check: warning: gh isn't installed, so main's CI can't be read." "$out"
commit_code gh-hangs
began=$(date +%s)
out=$(STUB_GH_HANG=1 PUSH_MAIN_GH_TIMEOUT=1 full --why "gh hangs"); rc=$?
check "a gh that never answers costs its bound, not a hang" "0 yes" "$rc $([ $(($(date +%s) - began)) -lt 20 ] && echo yes)"
has "and says so" "gh didn't answer within 1 s" "$out"
commit_code gh-fails
out=$(STUB_GH_FAIL=1 full --why "gh fails"); rc=$?
check "a gh that fails (signed out) warns and stamps" "0" "$rc"
has "saying so" "signed out, offline, or no access" "$out"
git checkout -qb not-main
commit_code branch-change
rm -f "$CALLS.gh"
STUB_GH_RUNS=$red sel_full > /dev/null
check "a branch's check never asks GitHub" "no" "$([ -s "$CALLS.gh" ] && echo yes || echo no)"
# The watcher: the pushed commit's run to its end, the result, and on a failure the jobs, the tests, their rates and the artifacts.
out=$(STUB_GH_RUNS="$(run_of failure 13 "$pushed")" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
check "a red watch ends with 1" "1" "$rc"
has "naming the run" "CI is RED on main $(printf '%s' "$pushed" | cut -c1-12) (failure): https://ci/13" "$out"
has "the failed job" "  check (lifecycle)" "$out"
has "the failed test, without the log's colours" "check (lifecycle): FAIL recto-cli::registrations no_copy" "$out"
has "its rate" "failed 3 of 5" "$out"
has "and where its evidence is" "check-logs-lifecycle-1 (4 KiB): gh run download 13 -n check-logs-lifecycle-1" "$out"
check "and keeps the result" "failure" "$(tail -1 .git/recto-ci-main | cut -f4)"
out=$(STUB_GH_RUNS="$(run_of success 14 "$pushed")" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
check "a green watch ends with 0 and says so" "0 yes" "$rc $(printf '%s' "$out" | grep -q "CI is green on main" && echo yes)"
out=$(STUB_GH_RUNS="$(run_of "" 15 "$pushed" in_progress)" PUSH_MAIN_POLL=0 PUSH_MAIN_LIMIT=0 tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
check "a watch that runs out of time ends with 3 and says how to look again" "3 yes" "$rc $(printf '%s' "$out" | grep -q "tools/push-main.sh --watch" && echo yes)"
out=$(PUSH_MAIN_GH=/nonexistent/gh tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
check "a watch with no gh says so at once" "2 yes" "$rc $(printf '%s' "$out" | grep -q "gh isn't installed" && echo yes)"
# Failure (coordinator, 2026-11-12, run 43248252599): a run whose macOS jobs never got a runner, cancelled after 15 minutes, ends as `failure` with no test
# run, and was read as red; or as green. It never ran: neither, said plainly, run again once, and the push guard isn't held by it.
never_jobs='check (window)\tcancelled\t0\t0\t3\ncheck (core)\tcancelled\t0\t0\t4\ncheck (lifecycle)\tcancelled\t0\t0\t5\nsandbox (macos-15)\tsuccess\t17\t0\t6\n'
rm -f "$CALLS.gh-rerun"
out=$(STUB_GH_JOBS=$never_jobs STUB_GH_RUNS="$(run_of failure 16 "$pushed")" STUB_GH_RUNS_AFTER="$(run_of success 16 "$pushed")" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
has "a run that never ran is said to have never run" "CI NEVER RAN on main $(printf '%s' "$pushed" | cut -c1-12) (failure, but no test ran): https://ci/16" "$out"
has "with each job and GitHub's annotation" "check (window) (cancelled, 0 steps: The job was not acquired by Runner of type hosted even after multiple attempts)" "$out"
check "it's run again once, and the watch follows it to its end" "0 yes yes" "$rc $([ -f "$CALLS.gh-rerun" ] && echo yes) $(printf '%s' "$out" | grep -q "CI is green on main" && echo yes)"
check "and it's never kept as a failure" "no" "$(grep -q $'\t16\tfailure\t' .git/recto-ci-main && echo yes || echo no)"
rm -f "$CALLS.gh-rerun"
out=$(STUB_GH_JOBS=$never_jobs STUB_GH_RUNS="$(run_of failure 17 "$pushed")" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
check "never running twice ends the watch with 2 and says how to go on" "2 yes" "$rc $(printf '%s' "$out" | grep -q "gh run rerun 17 --failed" && echo yes)"
rm -f "$CALLS.gh-rerun"
out=$(PUSH_MAIN_RERUN=0 STUB_GH_JOBS=$never_jobs STUB_GH_RUNS="$(run_of failure 18 "$pushed")" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
check "with PUSH_MAIN_RERUN=0 it only offers the rerun" "2 no" "$rc $([ -f "$CALLS.gh-rerun" ] && echo yes || echo no)"
out=$(STUB_GH_JOBS='check (lifecycle)\tfailure\t15\t1\t2\n' STUB_GH_RUNS="$(run_of failure 19 "$pushed")" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
check "a run where a part ran and failed is red, not rerun" "1 no" "$rc $([ -f "$CALLS.gh-rerun" ] && echo yes || echo no)"
out=$(STUB_GH_JOBS='check (lifecycle)\tfailure\t9\t1\t2\n' STUB_GH_RUNS="$(run_of timed_out 20 "$pushed")" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$pushed" 2>&1); rc=$?
check "and so is one whose part was cut off mid-step (a hang to the job's limit)" "1" "$rc"
# Failure (coordinator, 2026-11-12): a run cancelled by a newer push was reported as "CI is RED (cancelled)" and kept as a result, though nothing
# failed; the verdict was the newer run's, which nobody was then watching. It's superseded, and the watch goes on to the newer run and ends as it ends.
git checkout -q main
older=$(git rev-parse HEAD)
commit_code superseding
newer=$(git rev-parse HEAD)
mkdir -p "$X/gh-commits"
run_of cancelled 21 "$older" > "$X/gh-commits/$older"
run_of success 22 "$newer" > "$X/gh-commits/$newer"
both="[$(run_of success 22 "$newer" | sed -E 's/^\[//; s/\]$//'),$(run_of cancelled 21 "$older" | sed -E 's/^\[//; s/\]$//')]"
out=$(STUB_GH_COMMITS="$X/gh-commits" STUB_GH_RUNS=$both PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$older" 2>&1); rc=$?
has "a run cancelled by a newer push is superseded, with both commits" "CI on main $(printf '%s' "$older" | cut -c1-12) was superseded: a newer push ($(printf '%s' "$newer" | cut -c1-12)) cancelled its run" "$out"
check "it's never called red, and the watch ends as the newer run does" "0 no yes" "$rc $(printf '%s' "$out" | grep -q "RED" && echo yes || echo no) $(printf '%s' "$out" | grep -q "CI is green on main $(printf '%s' "$newer" | cut -c1-12)" && echo yes)"
check "and is kept as superseded, not as a failure" "superseded" "$(grep "$older" .git/recto-ci-main | tail -1 | cut -f4)"
run_of failure 22 "$newer" > "$X/gh-commits/$newer"
out=$(STUB_GH_COMMITS="$X/gh-commits" STUB_GH_RUNS="[$(run_of failure 22 "$newer" | sed -E 's/^\[//; s/\]$//'),$(run_of cancelled 21 "$older" | sed -E 's/^\[//; s/\]$//')]" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$older" 2>&1); rc=$?
check "a newer run that fails is the red one, by its own commit" "1 yes" "$rc $(printf '%s' "$out" | grep -q "CI is RED on main $(printf '%s' "$newer" | cut -c1-12)" && echo yes)"
out=$(STUB_GH_COMMITS="$X/gh-commits" STUB_GH_RUNS="$(run_of cancelled 21 "$older")" PUSH_MAIN_POLL=0 tools/push-main.sh --watch "$older" 2>&1); rc=$?
check "a cancelled run with no newer one is still reported as it ended, never as superseded" "no" "$(printf '%s' "$out" | grep -q superseded && echo yes || echo no)"
commit_code after-never
out=$(STUB_GH_JOBS=$never_jobs STUB_GH_RUNS="$(run_of failure 21 "$pushed")" full --why "never ran"); rc=$?
check "on main, a run that never ran doesn't hold the stamp" "0 yes" "$rc $(grep -qx "$(git rev-parse HEAD)" "$CHECKED" && echo yes || echo no)"
has "and the check says it's neither red nor green" "main's CI never ran on $(printf '%s' "$pushed" | cut -c1-12) (https://ci/21): no part got a runner or ran a step, so it's neither red nor green" "$out"
commit_code after-real-red
out=$(STUB_GH_RUNS="$(run_of failure 22 "$pushed")" full --why "real red"); rc=$?
check "while a run whose part failed still does" "4" "$rc"

# Failure (PLAN R0.7d): a CI-like run isn't CI's profile, concurrency or limits, holds the build to the limits too, runs the slow stage CI doesn't, counts
# as a pass for the push guard, or doesn't say what it can't give; or a test filter's rerun can't be repeated to give its rate here.
git checkout -q main
git checkout -qb like-ci-cases
commit_code like-ci-part
rm -f "$CALLS.policy" "$CALLS.slow"; : > "$CALLS"
runs_before=$(runs_in_ledger); checked_before=$(wc -l < "$CHECKED" | tr -d ' ')
out=$(tools/check.sh --like-ci lifecycle 2>&1); rc=$?
check "a part like CI passes as the part does" "0" "$rc"
has "it says what it can't give" "a clean user" "$out"
has "and a clean Launch Services database" "clean Launch Services" "$out"
check "it runs the part's tests, held by the limits, with CI's profile" "1" "$(grep -c -- '^taskpolicy -c background cargo nextest run .*long_session -j 4 -E test(~life_test) profile=ci$' "$CALLS.policy")"
check "and builds them first without the limits (the app tests', the registrations')" "2 0" "$(grep -c -- '^nextest run --no-run' "$CALLS") $(grep -c -- '--no-run' "$CALLS.policy")"
check "with no slow stage, as on CI" "no" "$([ -s "$CALLS.slow" ] && echo yes || echo no)"
check "and it's never a run, a pass or a stamp" "$runs_before $checked_before" "$(runs_in_ledger) $(wc -l < "$CHECKED" | tr -d ' ')"
check "an unknown option is refused" "2" "$(tools/check.sh --like-ci lifecycle --fast > /dev/null 2>&1; echo $?)"
commit_code like-ci-filter
rm -f "$CALLS.policy" "$CALLS.reruns"; : > "$CALLS"
# This repository's app binaries are none; for this case, the real one's first.
echo "APP_BINARIES='app'" >> tools/check-scope.sh
out=$(STUB_LIST='recto-cli::app flaky_one' STUB_JUNIT_FAIL='binary_id(=recto-cli::app)' STUB_RERUN_FAILS=1 tools/check.sh --like-ci 'binary_id(=recto-cli::app) & test(=flaky_one)' --times 3 2>&1); rc=$?
check "a filter like CI, N times: the failure fails it" "1" "$([ "$rc" -ne 0 ] && echo 1 || echo 0)"
check "it builds the app for an app binary's test" "1" "$(grep -c '^app-build$' "$CALLS")"
check "and runs it that many times under the limits, 4 at once for the app's tests" "3" "$(grep -c -- '-j 4 -E binary_id(=recto-cli::app) & test(=flaky_one) profile=ci$' "$CALLS.policy")"
has "with its rate" "failed 1 of 3" "$(printf '%s' "$out" | tr -s ' ')"
has "and its evidence, with the line that runs it again" "evidence of 1 failed test run(s)" "$out"
git checkout -q -- tools/check-scope.sh
commit_code like-ci-none
out=$(STUB_LIST='' tools/check.sh --like-ci 'test(=nothing_here)' 2>&1); rc=$?
check "a filter that matches nothing says so" "2 yes" "$rc $(printf '%s' "$out" | grep -q 'no test matches' && echo yes)"
rm -f "$CALLS.policy"; : > "$CALLS"
LIKE_CI_LIMITS='' STUB_LIST='alpha t1' tools/check.sh --like-ci 'test(=t1)' > /dev/null 2>&1
check "with the limits off (LIKE_CI_LIMITS=''), no taskpolicy, and 3 at once for a test that isn't the app's" "no 1" "$([ -s "$CALLS.policy" ] && echo yes || echo no) $(grep -c -- '--workspace -j 3 -E test(=t1)' "$CALLS")"

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
