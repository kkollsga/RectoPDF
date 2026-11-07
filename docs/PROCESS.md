# Process reviews

The coordinator is responsible for progress and quality. Owner, 2026-11-07: "Remember you are responsible for the progress and quality. So add a few process reviews every once in a while to catch inefficiencies".

**When:** at each phase's end, and at least after every fourth merge. **Looks at:** what landed against what's open, the queue (`tools/sysguard/heavy.sh --report`), CI, review rounds and their findings, and what the owner found that we didn't. **Ends with:** what changes, lessons added to `docs/agent-instructions/rust_app_development.md`, then checks at the next review whether it helped. An independent reviewer reads the numbers at each phase's end.

## 2026-11-07, about 17 hours in

**Numbers.** Phases 0 and 1 of 8 finished; 27 plan items done, 59 open. About 34,500 lines on `main`, about 25,000 more unmerged. GitHub 50 commits behind local `main`, with its last CI run failed. Queue: 8.3 h waiting against 6.8 h running; 36 of 177 jobs failed, using 0.5 h. Review findings: search 15 then 14, SVG 13 then 13, writer 11 then 6, app about 26 then 15.

**Worked.** Independent reviews found what tests didn't, such as documents leaking into each other and a save overwriting a newer file. The owner found more in minutes in a real window than screenshots had in hours.

**Waste, mostly the coordinator's.** SVG and fuzzing, from Phases 3 and 7, started before the first window merged. Seven workers spent more time waiting than building. New requests joined a branch under review, so its review never finished. Fix rounds added features, so review findings didn't fall. A third build slot opened before reading the guide's warning about reboots, and a merge script didn't stop on a conflict.

**Changing.** Merge first; freeze branches under review; two review rounds at most; about three workers; short dev-build cycles.

**Check next time.** Has the app merged? Is waiting less than running? Are review findings falling? Has anything sat unmerged over a day?

## 2026-11-07, the queue stopped

**What happened.** A solo job deadlocked the build queue for over 80 minutes, stopping RectoPDF and SeedWell. The bug was in the queue the coordinator wrote, set off by the coordinator's own instruction to use solo mode. A worker noticed; the coordinator had read the notices as normal. The coordinator then reported a blocked kill as done, and a worker killed its own job on that basis.

**Changing.** The queue can't hold slots while waiting, and all jobs share one order. A watchdog wakes the coordinator when the queue stops, and `--status` says if it isn't running. The memory guard runs. Waiting jobs report their wait every 15 minutes. Nothing is reported done until seen. Workers don't use solo.

**Check next time.** Did the watchdog fire, and was it right? Did the queue stop for over 15 minutes unnoticed?

## 2026-11-07, after four merges (app window, the queue fixes, fuzzing, SVG export)

**Numbers.** Queue over its whole log: 331 jobs, 81 failed, using 2,683 s of 36,062 s of run time (7%), so failures now cost little. Median wait 29 s, p90 367 s; the longest, 4,925 s, is the deadlock. Most running at once 3, lowest free memory 61%, highest swap 3,035 MiB. The watchdog hasn't fired since it started, and the queue hasn't stopped. CI: 5 red runs in a row before `0816cd4` went green.

**Waste.**
- One test kept CI red across 5 runs, and 4 of them were spent working it out. The coordinator pushed an explanation before it had evidence (`cd427f4`), then a sample taken after the moment that mattered (`8e58336`). Capturing the evidence during the failing window from the first push would have saved about two rounds of full check and CI.
- Nearly all of that test's noise had one answer in the end: the test was stricter than what it's for. But an intermittent failure in the same stretch was a real product bug (`recto stop` falsely saying the session ended), found only because "flaky" isn't accepted here.
- A wrapper script ended in `; echo`, so a failed check read as exit 0, and only reading the log showed it.
- The owner's design talk on restore changed the running worker's brief six times in about an hour. Sending notes early was right, since the format was still open, but the spec should be written first every time, so the worker reads one current text rather than a chain of corrections.
- The distillPDF worker ran about 6.8 hours, and asked to finish at the last minute it still found the lopdf regression. The regression test should have been run on the new pin as soon as the fork moved, not at release.

**Quality.** Reviews and workers kept finding real bugs late: the lopdf text regressions came from tests, with no change on 141 real documents, so corpus gates alone don't cover decoding edge cases. The worker correctly refused to change Claude's configuration on a relayed approval.

**Changing.**
- A failure seen only on another machine gets evidence captured in the test before any explanation is pushed.
- Scripts that gate a push end with the real exit status.
- Owner decisions go in the spec first, and the worker gets a pointer to it.
- Anything that moves a shared dependency runs its dependents' tests straight away.
- Steps that change an agent's own configuration are done by the coordinator on the owner's direct word, not delegated.

**Check next time.** Did CI stay green on `main`? Did a merge go through red? Did any test fail on CI without carrying its own evidence?

## 2026-11-07, how many suite runs we need

Owner: "13 tests at 10mins is more than two hours. There are surely ways of reducing how many tests we need, while achieving the same results and quality".

**Numbers.** distillPDF's full gate: 536 to 820 s, of which the 141-document sweep is 220 to 265 s, the wheel build about 100 s and the suites the rest. Its fast mode: about 200 s, sweeping 13 documents. Owner: "Not the 10 minutes, check why 13 tests where needed. That's the problem".

**Why 13.** Traced run by run. Seven were needed: the baseline freeze, the one proof the gate agrees with itself, the first look at lopdf 0.47, the final one on the pinned commit, and on the per-page branch its first pass, the pass after a performance fix, and the pass after the review's fixes. Six weren't:
- Four used the whole gate as a diagnostic while working out two documents' changes under lopdf 0.47: a repeat alongside memory measurements, one with a proposed patch, and final runs, with and without that patch, on a pin about to be replaced because the fork's fix was still waiting to be pushed. The with-and-without pair is read from the worker's note, not from two run folders. `--docs` for those two documents answers that in a minute or so.
- One moved lopdf and ran the whole gate, at the coordinator's instruction, when its three failing unit tests would have said so in about a minute.
- One freeze was stopped and restarted because of how it was queued.

The gate's length wasn't the cause. Nothing told anyone to ask a smaller question first.

A failed run also went on to 453 s, because the gate keeps going after a failure. The 141-document sweep caught nothing on that branch that a cheaper stage didn't: the document differences were the same two throughout.

**Changing.**
- Ask the smallest question that answers it: named tests, then `--docs` for the documents in doubt, then fast mode. The whole suite is for a decision to hand back, never for finding out why.
- Settle what's being tested first. No final gate on a dependency that isn't the final one.
- The coordinator's instructions say which check, not "run the gate".
- Fast mode while working. The full gate runs once on the commit being handed back, and once more only if code changed after it.
- After distillPDF 0.1.7: the gate stops at the first failure, with `--keep-going` for when the rest is wanted, and unit tests run before the wheel is built.
- After distillPDF 0.1.7: any document the full gate has ever flagged joins the fast set, so fast mode gets stronger as it catches things.
- A fork's CI set: the affected tests while working, the whole set once before pushing.
- RectoPDF's own tests are not the cost, 1,046 of them in 30 s, so none are being cut.

**Check next time.** Full gates per release. Target: at most 2, about 20 minutes in place of 2 hours. Did the full gate catch anything fast mode missed? If it does, grow the fast set.

## 2026-11-07, the trace of full checks, and what enforces the rules

**Trace.** `build/app`, `build/index`, `build/svg`, `build/select` and `build/render`: 41 real runs. 14 needed, 18 a failing check repeated after a fix, 9 a repeat a smaller test would answer. `main`: about 24 real runs: 9 on identical code (all skipped by the new memory), 6 on new code, 3 first-day setup, 3 chasing a CI-only failure, 1 a failure and its fix, 2 not classified. Classifying a run is a judgement; the detail is in the worker's report.

**Now in the tools.** `tools/check.sh` skips code that has already passed, counts full runs per branch and asks for a reason on the third, keeps a ledger (`--runs`), and shows all of a test stage's failures in one run. `heavy.sh` records repo and branch. `tools/check-test.sh` proves `check.sh`'s rules with no real build.

**The cost, accepted.** One of the 9 identical reruns found a real load-sensitive failure. CI still runs everything, and `CHECK_AGAIN=1` forces a run.

**Check next time.** `tools/check.sh --runs`: real full runs per branch, how many asked for a reason, and whether those reasons were real. Did the budget move agents to `--failed`? That's unknown until it's been used.

