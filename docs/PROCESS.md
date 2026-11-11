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

## 2026-11-08, after eight merges (restore, saved point, deep sleep and its follow-ups, updater, reader layout, drawing fixes, contents)

**Numbers.** Queue since the deadlock fix: median wait 0 s, p90 4 s; average 1.09 jobs running. Full runs by branch (`tools/check.sh --runs`): contents 6 (3 failed), updater 6 (2), reader layout 5 (2), drawing fixes 4 (1), deep sleep 4 (1), main 3 (0). The budget asked for a reason on the third: every branch above two ran with `--why`, mostly a merge of main or a review fix. Every review found real blockers, and every branch had two or three rounds.

**Waste and misses.**
- **The coordinator didn't watch CI after pushes.** CI failed on `e3e14d3` and `8125488` (two tests that failed once each, passing locally), and it showed only at this review. Every push now gets a watcher that reports CI's result.
- **Workers kept using `git stash`** until the rule was explicit, once touching the owner's own WIP in the lopdf fork. Now in AGENTS.md and the testing guide.
- **A worker reported tests that didn't exist**, as an edit script missed its anchor. Reviews now check that tests exist and fail without their fix.
- **Branches that ran long conflicted with each other**: the updater, contents and render branches each needed a merge of main with conflicts in session restore and the app bridge. Smaller branches merged sooner would have conflicted less.
- **Heuristics took three rounds:** header detection for copies swung between dropping content and keeping headers before settling. A corpus comparison against the previous survey was what made each round measurable.

**What worked.** Reviews ran the reviewer's own repros again in the last round instead of trusting the worker. The coordinator rechecked claims itself before merging (the bold-text cases and the header comparison), which caught a stale build being used for evidence.

**Changing.**
- Watch CI after every push to main, and fix a red main before merging more.
- Keep branches to one feature; split stage 2 into its own branch when stage 1 is reviewable.
- Corpus surveys for any heuristic, compared with the previous survey before a merge.

**Check next time.** Was main red on CI at any point, and for how long? Conflicts per merge of main.


## 2026-11-08, after four more merges (CI fixes, comparison view, dark paper after a replace, Save stage 1)

**Numbers.** Full runs since the last review (`tools/check.sh --runs`): comparison 4 (2 failed), Save 4 (1 failed, and a third and fourth with `--why`), dark paper 2 (1 failed), main 2 for its unpushed range (1 failed). Queue: median wait 0 s, p90 37 s (4 s at the last review), average 1.02 jobs running. CI on main: green on every push since the watcher started (`a640e59`, `c52ad25`), so main wasn't red at any point in this stretch. Reviews: Save round 1 found 3 blockers and 5 serious items, round 2 found 2 blockers; comparison round 1 found 3 medium. The merge of each long branch had conflicts in the same hub files: `recto-session/src/lib.rs`, `recto-ffi/src/lib.rs`, `qa.rs`.

**What the failed runs were.** Four of the five failures were tests making timing assumptions, not product bugs:
- a thread count read 100 ms after a command, twice, which catches a command's thread on its way out;
- a test that asked for two background jobs at once and assumed the newer would start first;
- a 100 ms render deadline, only 4× a page read, which a loaded Mac passed.
The fifth was a whole-state restore test that hadn't been told about new fields, which is its job. The deadline one also hid a real bug: a page read the Mac was too busy for was remembered as unreadable for the session.

**Waste and misses.**
- **649 MiB of build output was committed** on the Save branch: a helper built in its own folder, and the root `.gitignore` covered only `/target`. Review round 2 caught it before any push. The history was rewritten, and `target/` is now ignored anywhere. The coordinator now checks a branch's added objects for build folders and their total size before merging.
- **A worker's visual claims were wrong twice.** It reported a blurry page (the shot was taken mid-draw) and then a white page on dark paper (a zoom change made the pixel sample brighter). The coordinator caught both by looking at the shots. `qa shot` now waits for drawing to settle, and the real bug behind the second, a save changing the zoom, is now on a branch.
- **One worker ran out of working room** after building Save stage 1 and taking it through two review rounds and two merges of main. Its last report had to hand unfinished items to whoever came next. A stage now gets a fresh worker, with the previous report as its brief.
- **Security came from review, not the build:** Save As would write a hostile PDF's bytes to any file name, including a shell's startup file. The worker had copied `render`'s checks elsewhere but not to Save As.

**What worked.** Reviews ran their own repros: the symlink Save As bypass was reproduced, not argued. A "flaky" test traced to its cause found a real reader bug. Workers fixed every finding in one round before one full run, so no branch went past four full runs.

**Changing.**
- Tests wait for a condition with a bound, never a fixed sleep before counting threads, and never assume which of two queued jobs starts first. Added to the worker testing brief.
- Before a merge, the coordinator checks the branch's size and that no build folder is in it.
- Every path an agent can name for writing gets the same checks as the existing ones (extension, hidden parts after following links, `~/Library`). Reviewers check every new write path against them.
- A fresh worker for each stage of a large feature.

**Check next time.** Failed full runs that were tests' timing assumptions: still the main cause? Conflicts per merge of main in the hub files: worth splitting `recto-ffi/src/lib.rs`?

## 2026-11-08, why the code needs restructuring, and what changes

**Numbers** (architecture review through the code-review graph, `main` at `74cf8ec`): 19 crates, cleanly layered, no cycles. 11 hub files took every feature: each of the last 8 features touched 7 to 11 of them; links put half its 3,100 lines into about 20 shared files. `Session` has 34 fields and 180 methods; `Command` 59 variants (7 to 9 edits per new command); `execute_held` 262 lines; the bridge holds policy with 10 unit tests for 2,100 lines. Find, links and Save conflicted in the same files on one afternoon, and Find had to come back off main once.

**Why.** Speed with parallel workers, briefs that never said where a feature lives, reviews that checked correctness and never structure, process reviews that measured tests and queues but not code shape, and an early design that suited 5 features left alone at 30. No `rustfmt` and a hand-kept whole-state field list turned every pair of branches into a conflict. The coordinator owns this.

**Changing.** Guide §2.8 and AGENTS.md rule 7: a feature in its own module with registration-only edits to shared files, named in the brief; reviews check structure; budgets for file, function, struct and enum size; a structure check through the code graph at every process review. The refactor plan itself is in `docs/reviews/2026-11-08-architecture.md`, waiting on the owner's timing.

**Check next time.** Files the last features touched, the hub sizes against the numbers above, and merge conflicts per merge.

## 2026-11-08 afternoon, after twelve merges (UI fixes, save view, sandbox, links, Save stage 2, Find, CI timing, opening, ⌘K, sandbox follow-ups, Find Next fix, image copy and Print)

**Numbers.** 12 merges since the last review, against the rule of a review at least every fourth: this one is late. Full runs: main 13 (3 failed), and the feature branches 2 to 5 each. Every branch went through at least one review round; Save stage 2, the sandbox, links and opening, and image copy and Print each had a blocker found by review, not by tests (a signature brought in with added pages, an agent reading a protected PDF's outline, unbounded snapshot workers, Save As writing anywhere earlier). Queue: median wait 0 s, p90 6 s. **CI on main: red on every push from `7dab7af` to `10f7c0c`, about four hours.**

**Why CI stayed red, in order:** GitHub's release downloads failed for over an hour (fixed with a source-build fallback, retries and a shared PDFium cache); the new macOS 15 job found a test that assumed macOS 27's port descriptions; two tests assumed CI's timing (one hid a real product bug: an undo replay that scanned the whole document twice per step); a restore test compared a system setting; a palette test raced Find's query (a real gap: an agent's Find Next didn't wait as the app's did); and on macOS 15 the renderer's memory retirement never fires in the sandbox (being fixed). Each one was real and each was fixed, but they arrived one behind another.

**Waste and misses.**
- **Merging on a red main hid the next failure.** The rule says fix a red main before merging more. I kept merging because each red item was understood and unrelated, and each push then showed one more failure that a green main would have shown alone. The scroller and palette failures were only seen because every log was read.
- **Hub conflicts on every merge,** as the architecture review measured: Find had to come back off main once, and links, Save, Find, opening, ⌘K and Print each conflicted in the same five files. The structure quick fixes are under way.
- **A check-tool bug blocked a push:** a documents commit made while a full check ran threw the pass away. Fixed in `check.sh`, with a test that fails on the old code.
- **Workers ran out of room** three more times, handing over mid-task; fresh workers per stage, now the rule, kept that cheap.
- **The new CI job was worth it:** the macOS 15 and 26 sandbox job found two real problems (a test's assumption and the retirement) that no local run could.

**Changing.**
- A red main gets fixed before anything else merges, even when the cause is understood. If a failure needs time, the one test is marked known-failing in its own commit with a PLAN line and a deadline, so CI is green again and the next failure shows on its own.
- Process reviews on schedule, every fourth merge.
- A worker's new tests are run by name on CI-like timing before the hand-back where possible (a busy queue is a fair stand-in).

**Check next time.** Hours main was red, and whether each red period had a single cause.

## 2026-11-11, after five merges (reader fixes, Save safety, build speed, soak growth, update gaps)

**Numbers.** Five merges since the last review, one past the every-fourth rule. CI on `main` green on all three pushes today (`11276a3`, `41f5ec3`, `b76531b`); the last red was the macOS 15 retirement test before `41f5ec3`, fixed in the test (`82678b3`) as a crossing after the last page has nothing to answer it. Full runs: `main` 3, no failure; branches 1 or 2 each, none needing `--why`. Save safety used 5 queue suites with 2 failed, the most of the five. Queue: 204 jobs, median wait 0 s, p90 51 s, longest 444 s; peak 3 jobs, lowest free memory 51%. A full check now takes 270 to 370 s under load.

**Structure (code-review graph and git).** Hub files touched per merge (the FFI lib, the session, command tables, QA tables, menus, MainWindow, bridge): 0, 2, 0, 1, 2, against 7 to 11 per feature before the quick fixes. The quick fixes worked for where features live. But the hubs themselves still grow: `Session` 180 to 193 methods, `Command` 59 to 67 variants. The largest files are unchanged: `recto-session/src/lib.rs` 7,012 lines, `recto-reader/src/tiles.rs` 5,489, `recto-ffi/src/lib.rs` 5,410, `recto-agent/src/table.rs` 5,298. Phase R is still needed and stays first after release. Merge conflicts today: only update gaps against dev updates, expected and handed to the worker.

**Waste and misses.**
- **I edited a test in the main checkout while a full check ran there.** The run passed on a dirty tree and stamped a commit without that edit. Harmless this time: product code was the same, and CI ran the committed code. But a pass should only ever describe what it ran. Fixes go in a worktree, or wait for the run to end.
- **Waiting builds.** The icon worker waited for the owner's choice, at no cost. The dev-updates and update-gaps branches touched the same updater files in parallel; one merge cleans up after both. Two branches on one module at once is the cost of finishing started work before release, and it was named in both briefs.

**What worked.** Merging two reviewed branches under one full check saved a run with nothing lost. Every worker hand-back used one full run, last, as the brief said. The worktree seed made first builds cheap enough that nobody skipped a worktree.

**Changing.**
- While a full check runs in a checkout, nothing in that checkout is edited. `check.sh` could refuse to stamp when the tree changed during the run; added to PLAN as a follow-up, with a `check-test.sh` case.
- Process reviews keep the hub-touches-per-merge count, and add hub growth (methods and variants) as a second number.

**Check next time.** Hub growth since today's numbers; whether phase R's first steps start right after the release tag; CI red hours.
