#!/usr/bin/env bash
# ci-gate-test: proves the release gate's decision (tools/release/ci-gate.sh) on made runs and a made repository, with no network, no tag and no
# waiting: the time waited is an argument, so ten minutes and an hour are given, not waited out. Seconds. Run by tools/release/release-test.sh,
# which tools/check.sh --full runs when tools/release/ changes. A mistake here is the costliest kind: v0.1.1's first tag spent its hour in the gate.
#
#   tools/release/ci-gate-test.sh
set -uo pipefail
SOURCE="$(cd "$(dirname "$0")/../.." && pwd)"
GATE="$SOURCE/tools/release/ci-gate.sh"
X=$(mktemp -d "${TMPDIR:-/tmp}/ci-gate-test.XXXXXX")
trap 'rm -rf "$X"' EXIT
fails=0
check() { if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: expected [$2], got [$3]"; fails=$((fails + 1)); fi; }
has() { case "$3" in *"$2"*) echo "ok    $1" ;; *) echo "FAIL  $1: no [$2] in [$3]"; fails=$((fails + 1)) ;; esac; }

A=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
B=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
# run <id> <sha> <branch> <status> <conclusion> <created>: one run as `gh run list --json` gives it.
run() { printf '{"databaseId":%s,"headSha":"%s","headBranch":"%s","event":"%s","status":"%s","conclusion":"%s","attempt":1,"createdAt":"%s","url":"https://example.invalid/runs/%s"}' \
  "$1" "$2" "$3" "${EVENT:-push}" "$4" "$5" "$6" "$1"; }
runs() { local f="$X/runs-$RANDOM.json"; printf '[%s]' "$(IFS=,; echo "$*")" > "$f"; echo "$f"; }
decide() { out=$("$GATE" --decide "$@" 2>&1); rc=$?; }

# Failure: the gate refuses or waits on a tag whose code CI passed on main, and the release is withdrawn for nothing.
decide "$(runs "$(run 1 $A main completed success 2026-11-16T10:00:00Z)")" 0 $A
check "the newest run on main green: passed" 0 "$rc"
has "naming the run" "CI passed on main on $A: https://example.invalid/runs/1" "$out"

# Failure: a run a newer push cancelled hides an older green run of the same commit (the review's first should-fix), and the gate waits and withdraws.
decide "$(runs "$(run 2 $A main completed cancelled 2026-11-16T11:00:00Z)" "$(run 1 $A main completed success 2026-11-16T10:00:00Z)")" 0 $A
check "the newest cancelled, an older green on the same commit: passed" 0 "$rc"
has "by the older run" "runs/1" "$out"

# Failure: a cancelled run is taken as a pass, or as a failure, or the gate gives up before ten minutes or never.
cancelled_only=$(runs "$(run 2 $A main completed cancelled 2026-11-16T11:00:00Z)")
decide "$cancelled_only" 599 $A
check "the newest cancelled and nothing else, before ten minutes: waits" 3 "$rc"
has "saying only a cancelled run is there" "only a cancelled run so far" "$out"
decide "$cancelled_only" 600 $A
check "the newest cancelled and nothing else, at ten minutes: refused" 1 "$rc"
has "saying the runs were cancelled and what to do" "were cancelled (https://example.invalid/runs/2), and no other started in ten minutes: re-run it" "$out"

# Failure: a branch's green run, or a run on main that no push made, passes a tag whose code main never had checked.
branch_green=$(runs "$(run 3 $A build/x completed success 2026-11-16T12:00:00Z)" "$(EVENT=workflow_dispatch run 4 $A main completed success 2026-11-16T12:00:00Z)")
decide "$branch_green" 0 $A
check "a green run on a branch, and one on main not from a push: not counted, waits" 3 "$rc"
decide "$branch_green" 600 $A
check "and refused at ten minutes" 1 "$rc"
has "saying there was no run on main" "no CI run on main on any of these after ten minutes" "$out"

# Failure: a red run passes, or an older green run on the same code outvotes the newest red one.
decide "$(runs "$(run 5 $A main completed failure 2026-11-16T12:00:00Z)" "$(run 1 $A main completed success 2026-11-16T10:00:00Z)")" 0 $A
check "a red run (the newest, over an older green one): refused" 1 "$rc"
has "naming how it ended" "CI on main on $A ended with failure: https://example.invalid/runs/5" "$out"
decide "$(runs "$(run 6 $A main completed timed_out 2026-11-16T12:00:00Z)")" 0 $A
check "a run that timed out: refused" 1 "$rc"

# Failure: a run still going is taken as decided, or waited for past the hour.
going=$(runs "$(run 7 $A main in_progress "" 2026-11-16T12:00:00Z)")
decide "$going" 3599 $A
check "a run still going: waits, up to the hour" 3 "$rc"
decide "$going" 3600 $A
check "a run still going at the hour: refused" 1 "$rc"
has "saying it's still going" "still in_progress after an hour" "$out"

# Failure: the gate waits out a default it could have said at once: no run at all.
decide "$(runs)" 599 $A
check "no run at all, before ten minutes: waits" 3 "$rc"
decide "$(runs)" 600 $A
check "no run at all, at ten minutes: gives up" 1 "$rc"
has "naming the commits asked about" "after ten minutes, so none is coming: $A" "$out"

# Failure: a run on a commit the gate wasn't asked about decides it.
decide "$(runs "$(run 8 $B main completed success 2026-11-16T12:00:00Z)")" 600 $A
check "a green run on another commit: not counted" 1 "$rc"

# ---- which commits: a made repository with main's history, as release.yml's checkout has it ----
R="$X/repo"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@example.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@example.invalid
git init -q -b main "$R"
g() { git -C "$R" "$@"; }
commit() { mkdir -p "$R/$(dirname "$1")"; echo "$2" >> "$R/$1"; g add -A; g commit -q -m "$2"; g rev-parse HEAD; }
commit src/a.rs one >/dev/null
code=$(commit src/a.rs two)
docs=$(commit docs/notes.md three)
readme=$(commit README.md four)
g switch -q -c build/side
side=$(commit src/b.rs five)
g switch -q main
g merge -q --no-ff -m "Merge build/side" build/side
merge=$(g rev-parse HEAD)
after=$(commit docs/plan.md six)
g switch -q -c build/unmerged
unmerged=$(commit src/c.rs seven)
g switch -q main
g update-ref refs/remotes/origin/main "$after"
commits() { out=$(cd "$R" && "$GATE" --commits "$1" 2>&1); rc=$?; }

# Failure: a documents-only tag asks only about itself, has no run, and waits its time out (as v0.1.1's first tag did, by another route).
commits "$readme"
check "a documents-only tag: asked back to the commit that changed code" "0 $(printf '%s\n' "$readme" "$docs" "$code")" "$rc $out"
# shellcheck disable=SC2086 # the commits, one word each
decide "$(runs "$(run 9 "$code" main completed success 2026-11-16T12:00:00Z)")" 0 $out
check "and a run on that earlier commit passes it" 0 "$rc"
# Failure: the code commit is taken from a branch's line under a merge (v0.1.1's first tag), which no push on main ended on.
commits "$after"
check "a tag after a merge: asked back to the merge, not the branch's commit" "0 $(printf '%s\n' "$after" "$merge")" "$rc $out"

# Failure: a tag on a commit main doesn't have is waited for, or passed by a run on its branch.
commits "$unmerged"
check "a tag's commit not on main: refused" 1 "$rc"
has "saying so" "the tag's commit $unmerged isn't on main" "$out"
commits "$side"
check "a tag on a branch's commit under a merge: refused" 1 "$rc"
has "saying CI on main never ran on it" "on main only as a branch's commit under a merge" "$out"
out=$(cd "$R" && g update-ref -d refs/remotes/origin/main && "$GATE" --commits "$after" 2>&1); rc=$?
check "main's history not here: refused" 1 "$rc"
has "saying it can't be said" "main's history isn't here" "$out"
g update-ref refs/remotes/origin/main "$after"

# ---- the gate itself, with gh replaced: it asks only main's pushes of ci.yml, for each commit, and acts on what decide says ----
mkdir -p "$X/bin"
cat > "$X/bin/gh" <<EOF
#!/usr/bin/env bash
echo "\$*" >> "$X/gh.args"
sha=""; while [ \$# -gt 0 ]; do [ "\$1" = --commit ] && sha="\$2"; shift; done
[ "\$sha" = "$code" ] || { echo '[]'; exit 0; }
printf '[{"databaseId":1,"headSha":"%s","headBranch":"main","event":"push","status":"completed","conclusion":"%s","attempt":2,"createdAt":"2026-11-16T12:00:00Z","url":"u"}]' "\$sha" "\$(cat "$X/conclusion")"
EOF
chmod +x "$X/bin/gh"
: > "$X/gh.args"
echo success > "$X/conclusion"
out=$(cd "$R" && PATH="$X/bin:$PATH" CI_GATE_POLL=0 "$GATE" "$readme" --repo o/r 2>&1); rc=$?
check "the gate on a documents-only tag, its code green on a re-run: passed" 0 "$rc"
has "by the code commit's run" "CI passed on main on $code" "$out"
args=$(cat "$X/gh.args")
has "asking ci.yml's pushes on main" "run list --repo o/r --workflow ci.yml --branch main --event push --commit $readme" "$args"
check "once for each commit" 3 "$(wc -l < "$X/gh.args" | tr -d ' ')"
echo failure > "$X/conclusion"
out=$(cd "$R" && PATH="$X/bin:$PATH" CI_GATE_POLL=0 "$GATE" "$readme" --repo o/r 2>&1); rc=$?
check "the gate on a red run: refused" 1 "$rc"
asked=$(wc -l < "$X/gh.args" | tr -d ' ')
out=$(cd "$R" && PATH="$X/bin:$PATH" CI_GATE_POLL=0 "$GATE" "$unmerged" --repo o/r 2>&1); rc=$?
check "the gate on a commit not on main: refused before asking CI" "1 $asked" "$rc $(wc -l < "$X/gh.args" | tr -d ' ')"
has "with the words" "isn't on main" "$out"

if [ "$fails" -gt 0 ]; then echo "$fails failed."; exit 1; fi
echo "All passed."
