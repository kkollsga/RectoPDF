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

## 2026-11-11, CI's whole history: trends and what changes

**Numbers** (`gh run list`, 53 runs on `main`, 2026-11-07 to 11-11): 27 green, 20 red, 5 cancelled. `main` was red for about 12.5 hours over five stretches: 4.5 h over 6 pushes (11-07), 1.5 h, 0.5 h, 3.7 h over 9 pushes (11-08) and about 2.3 h over 3 pushes (11-11). The red rate hasn't fallen: 37% on 11-07, 38% on 11-08, 3 of 7 on 11-11.

**What failed, by cause** (one red run can hold several):
- **The outside world (4 runs):** GitHub's release downloads answered 504 for over an hour. Fixed with retries, a cache and a source-build fallback; not seen since.
- **This machine isn't CI's (about 11):** macOS 15's launchd and sandbox behaviour (3 runs), macOS 26 killing a probe, a system scroller setting compared as app state (2), memory retirement never firing in macOS 15's sandbox (3), and CI's virtual machine waking about 80 times a second at rest (4 pushes in a row, still unexplained).
- **Tests assuming speed or order (about 10):** socket kill windows, a spin test, a print pixel tolerance, the quiet reopen count, the watchdog's 250 ms look, layout done only when asked, and a page-number fade.
- **Real product bugs, found by those failures (8):** a renderer starting up reaped as idle; a client catching up dropped for its own queued commands; an undo replay scanning the document twice per step; an agent's Find Next not waiting as the app's did; memory retirement off in the macOS 15 sandbox; a late state never applied at launch; the window rebuilt from it losing what was just done; and an update's launch showing a closed window.

**Trends.**
1. **No failure was in pure core logic.** Every red test touched time, processes, sockets, the window or the OS. The Rust core's deciding code, unit-tested without the app, never failed on CI. Risk sits at the boundaries, so the more that's decided in the core and tested on a fake clock, the less CI finds.
2. **The same failure came back push after push:** the wake-up test 4 times, launchd 3, the late restore 3. Each time a fix was checked only by pushing it, because CI's condition couldn't be made here. Every long red stretch is one of these loops.
3. **Red clusters on heavy merge days** (11-08: 10 of 26 runs), when merges went on over a red `main`.
4. **CI's difference is worth having:** its slower machine and its second macOS version found 8 real bugs and 5 OS assumptions no local run could. Cancelled runs (5) were all a newer push superseding an older one, as designed.

**Changing** (some already done today):
- CI runs on `build/*` branches, which merge only once green (owner, 2026-11-11). The late-state stage is in the full check, and a full slow mode is planned before release.
- **A CI-only failure's fix reproduces the failure locally first:** a slow mode, a seam, an OS condition or a loaded machine, shown failing before the fix and passing after. A fix that couldn't fail here doesn't go to `main` to be tested.
- **Tests that can fail only on another machine say what they saw:** timings, the OS version, what launched when. Several of today's CI logs couldn't tell late from on time.
- Process reviews count CI red hours and causes from `gh run list` each time, beside the queue and check numbers.

**Check next time.** Red rate and hours since today; whether every CI-only fix was shown failing locally first.

## 2026-11-12, after the release: why development felt slow, and the plan for phase R

Owner: "I have noticed extremely slow development speeds on this project". The whole review, with its sources, is `docs/reviews/2026-11-12-development-speed.md`.

**Numbers.** 729 commits: 225 feature, 177 product fix, 103 test fix, 67 tooling. A feature's code took 25 to 80 minutes and its branch then lived 1.5 to 5 hours. 209 commits came after the last feature merge (11-08 13:13), about 195 of them stabilising. A full check's pass went from 129 s to 514 s in five days; 29 of 52 failed runs failed after the app was built. CI: 32 of 80 runs red, and every failing test in the 14 red runs since 11-11 launched the app. Builds: 4 to 19 s after a hub edit, so not the cause.

**Structure since the last review.** `Session` 193 methods (unchanged), 36 fields; `Command` 68 variants (67); `reconcile` 500 lines (191 on 11-08); `recto-ffi` 14,468 lines (6,474) with 100 unit tests. Hub lines per merge stayed low, but no feature has landed since the quick fixes, and fixes kept growing the hubs.

**Why.** The app's logic sits in the binding crate, where only tests that launch the app reach it, so the fast check is blind to the code that changes most. Lifecycle code depends on speed. The window and the CLI run separate implementations of each action. Review finds the most bugs and comes after the code. The coordinator owns this: eight process reviews measured queues, runs and CI, and none measured a branch's tail or what the fast check couldn't see.

**One pattern in this file.** Fixes held by a tool stuck (the run budget, pass memory, rustfmt, the queue's refusal). Fixes that were only written rules recurred: tests assuming speed four times, merging over a red `main` four times, late process reviews twice. Phase R's process changes are all tools, tests or ratchets.

**Changing.** Phase R is replanned in `docs/PLAN.md`: tools first (R0), the app core out of the bridge (R1), one action with one implementation (R2), components (R3), then test homes, a feature kit and a measured pilot (R4). Guide §2.9, §2.10, §5.5 and §6.6 carry the general lessons.

**Check next time.** After R0: `--quick`'s time and whether a stale registration fails in under 30 s. After each track: the exit numbers in the review's §7. At every review from now: a branch's tail (minutes from last feature commit to merge, follow-up merges), beside the queue, runs and CI.

## 2026-11-12 evening, after seven merges (push-main's superseded runs, the headless kit, the CI guard, lifecycle slices 1 and 2, the test audit, the watchdog fix)

One evening after a restart of the Mac, with at most two workers at a time. The owner set three rules in it: tests are for real concerns, CI doesn't get slower, and the warning for that is stern (all in `AGENTS.md`).

**Numbers.**
- **CI's check job:** 33, 18, 28, 25 and 28 minutes on the day's first five single-job runs; 15 min 18 s on `420e2e3` (the first lifecycle slice, the audit and the watchdog fix); 23 min on `e656236` (the second slice, a run that also paid the tools' tests and every test, as `tools/check.sh` changed). App tests on CI: 461 to 754 s before, 272 s on `420e2e3`. One run each, on runners that vary, so the trend needs the next five.
- **Tests that launch the app:** 225 to 204 (21 gone: 5 for the kit, 12 and 8 for the lifecycle slices less one counted twice, 1 for the audit). The ratchet holds the number.
- **Local full check:** app-tests stage 174 to 195 s before, 141 to 150 s after, at differing loads. The slow stage is 107 to 109 s and has barely moved: it is now the largest local stage a branch can't avoid.
- **Workers:** five tasks, all Opus (design was the work, or judgement was). The kit: 43 min, 399k tokens. Lifecycle 1: 52 min, 502k. The audit: 61 min, 428k. Lifecycle 2: 49 min, 337k. Each stopped once to report back as briefed (a design, an audit table); none was sent back; each used one full run and, where lifecycle was touched, one CI-like run. The coordinator did three small tool branches itself.
- **Full runs:** 12 in the ledger for the evening: 4 by workers on their branches, 4 by the coordinator on branches (two tool branches, the watchdog fix, and a worker's branch after `main` was merged into it), 4 on `main`. None failed. Twice more `main` ran nothing, as the tool counted a branch's pass on the same code.
- **CI:** 6 pushes of code. One red: `the_watchdog_catches_a_stall_and_it_fails_the_script`, 1 of 5, fixed from its evidence in one round (below). One run where a sandbox job got no runner twice.
- **xprotectd:** 3.7 MB after the restart and after each of seven full checks. One evening of checks doesn't make it grow; the 3.9 GB of before took 19 days.

**What review and the audit found.**
- **A measurement changed the plan.** The kit's first five ports saved 13 test-seconds: a window-tier app test is under a second on a quiet Mac. The seconds were in lifecycle and on CI's slower machine, so R1.4 moved ahead of the remaining R1.3 ports. The handover had assumed the opposite, and so had the review's estimate of "about 90 ports".
- **Tests that couldn't fail.** The audit read about half the suite and found two tests that passed with their bug in place (an idle worker's wake removed; a kill that waited), one byte cap that was never reached, and a cap in a test's name that was never asserted. Each was found only by seeding the bug. A doc comment naming a failure isn't evidence the test catches it.
- **Two product bugs** came out of structure work: a quit and a hand-off each lifting the other's freeze (found by the machine's every-order test), and the hang watchdog blind to blocks the run loop hadn't bracketed (found by CI, 1 of 5). The second's cause in the run loop is still not known; a counter in `qa state` now says how often it happens.
- **One failure was enough.** The red `main` was diagnosed from the first run's evidence folder (`hangs.worst_ms` of 6 during a 2.3 s stall), without a rerun. R0.7 paid for itself. Its sample of the app didn't finish in its bound on a runner at load 52, which is when it's wanted most.

**What went wrong.**
1. **The coordinator repeated a stale figure to the owner** (CI "12 to 15 minutes", the undone three-part layout's) until the owner said CI seemed slow. A number in a handover is checked against its source before it's said again.
2. **A guard of the coordinator's own stopped it:** the watchdog fix first added two bridge methods, and the ratchet refused. The second form reused the existing call and is better. The guard was right; an hour's work should start with the ratchet in mind.
3. **Two branches edited the same six tests** (the audit rewrote one that a lifecycle slice then deleted), as the audit's out-of-bounds list named the first slice's files and not the second's. Bounds are set from every running brief, when each is written.
4. **The second slice's CI-like run was on the branch before `main` was merged into it.** The full check covered the merged result; the rule asks for the lifecycle part too. It should run after the merge of `main`, by the coordinator if the worker has handed back.

**Changing.**
- Reports to the owner give CI's check time from `tools/push-main.sh`'s kept times, not from memory or a note.
- A deletion or a rewrite of a test needs its seeded-bug proof (now in `AGENTS.md`'s rule and in the briefs); the audit's unread half is in the plan.
- The budget (25 min) is lowered when five green runs in a row are under 20.

**Check next time.** The median of CI's last five check times and whether the budget was lowered; the count of app-launching tests; whether the slow stage (107 s) has a plan; `hangs.unbracketed_blocks` in any new evidence; whether the watchdog test has failed again.

## 2026-11-14 afternoon, after 90 merges in 40 hours (phase R's main body: one executor, the parts of `Session`, `Reader` and `Inner`, typed payloads, the pilot, two security fixes)

From `be9a8cc0` (2026-11-13 00:34) to `89955067` (2026-11-14 16:28). The numbers were gathered read-only by a worker from git, `.git/recto-full-runs`, `heavy.sh --report`, `.git/recto-ci-times` and `gh run list`; the coordinator checked two of its findings against their source and corrected one (below). The queue's figures are all-time, as its report has no date filter.

**Numbers.**
- **Merges:** 90 (63 on the 13th, 27 on the 14th). A branch's first commit to its merge: median 36 min, 7 branches over 3 hours (settled-gaps 453 min, the pilot 309 min with 78 commits).
- **Full runs:** 201 in the ledger (43 on `main`, 158 on 74 branches); 22 failed, 12 of them in the app-tests stage. A passing run: median 329 s on `main` (n=40), 312 s on a branch (n=139). 22 of 74 branches had more than two runs, nearly all with a `--why` of a review round or a merged result; the most were one-executor-4 (7), record-lock-retry (6).
- **Queue (all-time):** wait median 0 s, p90 15 s, longest 754 s; 82 reruns of a failure on unchanged code refused.
- **CI:** 43 green kept runs in the period, the check job's median 1,157 s (19.3 min). The last five green: 837, 879, 958, 972 and 1,155 s, median 15 min 58 s against the budget of 21. At least 12 red runs in the period, each a test failing about 1 run in 5 on CI's runners.
- **Structure (the ratchet's baseline at each end):** tests that launch the app 204 to 200; the bridge 2,354 lines to 2,089; `Inner` whole type about 119 methods to 44, with 76 by the whole count; `Session` 36 fields to 12, 103 methods; `Reader` 138 methods to 82; `Engine` 81 to 46; `reconcile` 280 lines to 219; files over 1,500 lines 14 to 4.
- **Hubs:** 50 of 90 merges touched a code hub. `recto-agent/src/lib.rs` in 23 merges (+1,187, −1,266), `core/mod.rs` in 24 (+185, −566), the bridge in 13 (+135, −431).
- **Conflicts:** 30 of 90 merges had a file resolved by hand: `docs/PLAN.md` in 24 and the ratchet's baseline in 14.
- **Workers:** not kept per task in this period (minutes, tokens, times sent back), against the rule that the coordinator keeps them. Known without a table: the Sonnet worker on the contents fix was sent back for judgement more than twice and the task moved to Opus; four workers ran out of room and handed over; the weekly Opus limit stopped three workers at once on the 14th.

**What reviews found that checks and reports had not.**
- **Two security faults, both in 0.1.0 by its code:** a protected document's place and Find hits reaching agents through `recto watch` (the whole-phase review; fixed at one boundary), and its place through a side-by-side comparison (the review of that fix's sibling; the structural fix is in hand). And earlier, an agent's `palette --run` running as the person (fixed; `an_agents_palette_run_never_runs_as_the_person`).
- **A claim in the plan that was false:** "one export for a person's actions". The person's print was a second implementation; now a command, with every export in one table.
- **A decision of the coordinator's that broke an owner's rule** (undo dropping saved-highlight steps), caught by review and reversed; and a contents design the coordinator approved that a second review showed wrong.
- **Phase R's exit numbers are mostly missed** (a full check ≤ 240 s: 312 to 329 s; CI ≤ 10 min: 16 to 19; app-launching tests about 110: 200; `reconcile` under 80 lines: 219).

**What went wrong.**
1. **A red `main` went unrecorded and undiagnosed for 15 hours.** Run 43539671770 (5ae2fb1d, 2026-11-14 01:31 UTC): `a_hand_off_in_the_middle_of_things_loses_nothing`, `app.find.field_has_focus: true then false`. It is in neither `recto-ci-known` nor the plan; this review found it. (The gathering worker named the wrong test from the log: the line it read is a panic a passing test makes on purpose. The coordinator read the run's log.) A worker is on it now.
2. **A visible change merged without being seen in the real app,** and put a regression on `main` (the contents list drawn empty with nothing current). Found by the coordinator's sheet after the merge.
3. **A full check failed on a merge at load 77 and the cause took a branch of its own:** the system purging what the page view marks discardable. Real, and 0.1.0 has one fault from it (a sleep picture without its one-colour tiles).
4. **Leftover background commands:** 137 `tail -f` watchers and 9 wait loops left by workers, noticed by the owner as 41 and then 71 running tasks; the coordinator first answered with a guess.
5. **Numbers told to the owner and then corrected** (five in the period: a method count, a token's length, a size, a claim about what was re-applied, an exposure stated too widely).
6. **The same two files conflict in most merges** (`docs/PLAN.md`, the baseline). Recorded in the plan before, not fixed.
7. **Per-task worker costs weren't kept.**

**Changing.**
- **Every red run on `main` is read by its failing test's own lines, and recorded with its cause or "not known, in hand", before the next push.** `tools/push-main.sh` reports the run; the coordinator opens the log when it is red and doesn't go by a summary.
- **A visible change is seen in the real app before it merges** (in the guide's rewrite already).
- **Every brief says background commands exit by themselves and none is left at hand-back** (in force since the 14th).
- **Workers stop editing `docs/PLAN.md`'s long lines in place:** a branch adds its entry as new lines under its item and the coordinator folds them in at the merge; and the baseline is resolved with `tools/ratchet.sh --resolve` only. Whether that brings the conflicts down is checked next time.
- **The worker table is kept from now:** task, model, minutes, tokens, times sent back and why, full runs, what review found.
- **CI's budget stays at 21 min.** The rule lowers it when five green runs in a row are well under it, and the two latest aren't: 19 min 15 s (c8ddb0e5) and 19 min 33 s (89955067), after 13 min 57 s to 16 min 12 s on the four before. By stage against 0ee6e666's run (15 min 1 s of full check): changed-tests 101 s then 177 and 117, other-tests 229 s then 190 and 271, the app's build 112 s then 130 and 140, app-tests 334 s then 374 and 345, and a 59 s cache save on the last. No one stage explains it, and the stages that grew differ between the two runs, which reads as the runners; three runs don't settle it.

**Check next time.** Whether CI's check time comes back under 17 min or stays at 19 (then by stage, over five runs, before anything is changed); whether a red run went unrecorded; conflicts in `PLAN.md` and the baseline per merge; the worker table; the median of the last five CI check times against 20 min; failed full runs in the app-tests stage (12 of 22 this time: which tests, which the ledger doesn't say, so the ledger gets the first failing test's name).

## 2026-11-15, the close of phase R (16 merges since the afternoon's review; the phase's second independent read)

From `89955067` (2026-11-14 16:28) to `6aab5734` (2026-11-15 01:52). Numbers from git, `.git/recto-full-runs`, `.git/recto-ci-times` and the task notices of this session; estimates are called so.

**What closed the phase.** The owner ended its numeric targets on 2026-11-14 ("let's remove elements of diminishing returns from the plan and the backlog"): the plan went from 263 open items to 186, 46 taken out. The second independent read of everything since the first (given the code and the rules, nothing run) found the work sound to release from, nothing that widens what an agent can see or do, and three things to fix, now merged or settled. Input to frame of R3.7 and everything since, alone on the machine: no case moved more than 5.8 µs.

**Numbers.**
- **Merges:** 16 in the period (43 since the start of 14 November). The large ones: the withheld place (four rounds and a follow-up), the person's lane (two rounds and last fixes), the rules on `Actor`, the print for a password-opened document (a review, a fix round, a re-read), the snapshot tool through the executor (a review and its fixes).
- **Full runs:** 26 ledger lines in the period; none of main's failed. One failed earlier in the evening on main at load 32, on a test merged that afternoon whose time limit counted from its own start (fixed the same hour).
- **CI:** 12 green runs, the check job 13 min 53 s to 19 min 57 s, median 16 min 45 s against the budget of 21; one red (`e67fdd39`: Find's field taking the keyboard a turn after Find opened, there since 2026-11-08, fixed the same evening). Tests that launch the app: 200, unchanged; no branch added one. The bridge 2,089 lines to 2,044; whole-`Inner` 76 to 75.
- **Workers** (the table the last review asked for; tokens and minutes from the task notices, rounded): 24 worker tasks, 19 on Opus and 5 on Sonnet, about 7.9 million tokens; 6 reviewers on Opus over 12 reads, about 1.8 million. By task, tokens in thousands and minutes: weak tests (Sonnet) 177/16; process numbers (Sonnet) 122/4; print design 226/11; Find focus twice 327/64; first show 308/67; contents (Sonnet) 152/14; withheld rounds two to four and follow-ups 517/87, 419/79, 478/60, 573/99; lane 419/57 and 422/77; slow Find 287/43; Find announcing (Sonnet) 124/12; actor rules 334/37 and follow-ups (Sonnet) 162/17; print build 508/73 and its fix 379/60; snapshot 658/123 and its fix 340/47; the prune 287/13; closing fixes 422/50; the walk's diagnosis 264/57. Sent back for judgement: none of the Sonnet tasks. Out of room and handed over: the withheld worker three times, the lane's, the print's, the snapshot's (each past about 420 thousand tokens).

**What review found that checks and reports had not.**
- **Every security branch failed its first review, and two failed three.** The withheld place: four reads before "ready", each closing the last residue and exposing the next, until the rule was changed (nothing an agent sends lets a place go). The print: an agent could reopen the app's own descriptor on the plain copy by `open /dev/fd/N`, missed by the design and the build, found by the reviewer the coordinator had told to press on exactly that. The snapshot: a box parked behind a page delete would have copied another page.
- **Two of the coordinator's design decisions were wrong and reversed on review:** one global lane for the person's edits (a highlight's read held every tab's), and "no change" to menu enablement (the fault as filed survived on a fresh document). A third (one deadline shared by a quit's wait and its write) cut the write's time from 5 s to 2 s and was withdrawn.
- **The closing read found a fault none of the per-branch reviews could see,** as it sits where two of them meet: a print or snapshot parked in the lane and then dropped was never answered, and its feature stayed busy until a restart.

**What went wrong.**
1. **A worker was sent to fix a fault already fixed,** from a stale plan line (Find announcing when nothing changed): 124 thousand tokens for a table test.
2. **An unexplained failure was first handled by taking the test's segment out** (2 in 80). The closing read wouldn't let it rest; 200 rounds with a record and a read of the code gave a likely cause (a wait shorter than one job is allowed) and found the record itself mislabelling. The segment is back.
3. **Residue-by-residue fixing on the withheld place cost three rounds** before the rule was questioned. Two rounds in, the pattern (each fix exposing the next state derived from the same release) was visible.
4. **A finished worker kept sending its hand-back** until stopped.
5. **The plan's conflicts:** workers were told to add lines and not rewrite long ones; conflicts in `docs/PLAN.md` still met most merges of main into a branch, each small.

**Changing.**
- **A security change gets its attack question in the brief and in the review's brief:** what new thing does the app hold or do that it didn't before (a descriptor, a plain copy, a state derived from a secret), and by which commands could an agent reach it. The print's hole was found only because the review was asked it.
- **When a second review round finds a new residue of the same cause, the rule is reconsidered before a third round is built.**
- **A plan line is checked against the code before a worker is spent on it.**
- **An unexplained failure gets a record and a bounded run before any test is cut**, never the other way round.
- **Every brief ends "send your hand-back once and stop".**

**Check next time.** CI's check time now the day's tests are in (the median, and by stage if over 18 min); whether the comparison walk fails again and what its record says; a settle that runs out without naming the call (seen once); conflicts in `docs/PLAN.md` now it is a third shorter.


## 2026-11-15, afternoon: after the day's merges for 0.1.1

**Merged since the close of phase R** (each on its full check, `main` checked on the merged result, the fix diffs read): `build/on-screen-fixes` (one review round, 5 should-fix), `build/find-key` (a security review, 7), `build/ci-light-job`, `build/keychain-planted`, three probe-only merges, `build/no-page-pictures` (two rounds: 2 serious and 5, then 3), `build/index-faults`, `build/slow-open-starved`. Held: `build/find-text-encrypted` (two security rounds: 1 serious and 10, then 2 serious and 8), for the owner's choice of where the key lives. In review's second half: `build/shorter-file-pages` (2 serious, 4).

**CI,** from `tools/push-main.sh`'s kept times: the check job 963, 1,378 and 1,269 s before the split into two jobs, then 875, 955, 1,169 and 916 s for the longest job; the median of the last five 15 min 55 s against 21. One red run: `opening_a_slow_hostile_file_never_holds_the_window`, 1 in 5 at load 33 on 3 cores, a test holding the runner's speed (fixed the same hour, with the hang watch made able to tell a wait from a starved thread).

**What review and measurement found that reports had not.**
- **The Keychain design failed on measurement, not on reading.** Four probe runs on the team's signature each overturned something a worker or the coordinator had read: the per-call flag does nothing for the login keychain; an item open to any application opens silently whatever its partition; an item planted trusting the app alone can't be told from the app's own, and a partition entry is taken when forged at the add; an ad hoc program can overwrite the app's item after it exists. Two of those the coordinator had told the owner the other way round, as a worker's reading, hours before.
- **Removing a feature found faults the feature had covered:** a wake's record lost between its note and its showing, 0.1.0's pictures kept for a week in the copy an update makes, a contents list with no cap once pictures no longer bounded the file.
- **Faults of 0.1.0 found on the way, each fixed with a test that failed first:** the one backup pruned by the original's date; a crash-left history closed and pruned in one launch; an agent's palette words in the activity log; the update's copy in Time Machine; direction-changing characters in Find's rows; a clear and a resumed index build that hang on a pipe or follow a link out of the corpus.

**What went wrong.**
1. **The coordinator guessed the cause of slow CI runs and said it to the owner** ("compiling, from changes low in the crate graph"); the logs said otherwise (a check added the same morning, the tools' tests, the runner). It was called a guess, and was still wrong; the worker's read of three logs took 8 minutes.
2. **A worker's reading of the Keychain was relayed twice as the likely answer** before the probe that contradicted it had run. The probe existed for that question; the relay should have waited the 4 minutes.
3. **Two workers touched the owner's login keychain:** one announced test item during the design, and one by accident (an add to a keychain path that doesn't exist falls through to the default keychain). Both removed; the probe now refuses a path that isn't a file.
4. **A worker stopped a check with `pkill -f` on a name pattern.** Nothing else matched, by its account and the other worker's.
5. **A test left a shared folder open** (`/tmp/recto-501` at 0775 under a long TMPDIR), failing two log tests at random on every branch until found.
6. **The first privacy statement for the update was false for a week of everyone's use** until the second reviewer read 0.1.0's installer: the claim was checked against the new code only, not against what the old version leaves behind.

**Changing.**
- **A claim about the OS that a probe can settle is not told to the owner until the probe has run.** Until then it is "unmeasured", with no likely answer attached.
- **A cause is not named to the owner from stage totals.** It is named from the log's own lines, or it is called unknown.
- **A statement about what an update removes is checked against the released version's own code,** by tag, in the brief and in the review's brief.
- **No worker creates, reads or deletes anything in the person's Keychain, for any reason;** a probe that needs a keychain makes its own and refuses any path that isn't a file it made. In every brief that touches it.
- **Processes are stopped by pid.** In `worker-testing.md` next time it is edited.

**Check next time.** The longest CI job over the next five runs against the estimate (950 to 1,250 s); whether the starved-thread failure recurs and what the new record says; the print-to-PDF test, which holds the same one-second bar the same way (`crates/recto-cli/tests/app.rs:3644`) and has not been rewritten.

## 2026-11-15, evening: the Find text encrypted and merged (6 merges since the afternoon's review)

**Progress against the dates said.** The data-protection keychain probe: said "tonight, measured 16 November", measured the same evening (run 43754866746, then 43761944758 with the app's own key code). The app's keychain change: said midday 16 November, handed back the evening of the 15th. The security read's fixes: slipped to 18:00 on the 16th by the worker's estimate of 8 hours, said to the owner at once, and came back the same evening. Merged: `build/dp-keychain-probe`, `build/dp-probe-signs`, `build/find-text-encrypted` (f88d1386), `build/find-text-docs`, `build/find-text-numbers`, `build/find-text-help`. Not merged: `build/restore-read-late`, whose fix is in `place.rs`, one of three files a worker's edits were denied on; the owner has been asked five times today and hasn't answered, and nobody has worked around it.

**What review found.** Two independent security reads of the keychain commits. The first: one serious (the master key written to whatever `recto-indexer` is at the bundle path at each build, the bundle checked once), three should-fix (a check that couldn't be made read as "not the release" and replaced the lasting index; the entitlement check not holding "exactly three"; nothing running the app's own key code on the real signature). The second: one should-fix (the preflight compiled Rust with the signing keys loaded). The coordinator's own read added that the probes still ran with the keys loaded; the preflight now signs, removes the keychain, shows 0 identities, then runs. All fixed before the merge.

**Waste and mistakes.**
- The first probe run (43751288571) hung at its first team signing for the job's 20 minutes with no limit on the step: 20 minutes and a round trip. Cause unknown; the next run, with the old login-keychain probe's step gone and limits on every step, signed in 31 to 115 ms. **Change:** a CI step that signs or launches runs under a limit and says which step waited.
- The coordinator's brief for the probe allowed building while keys were loaded; it took the second security read to see it. **Change:** a brief for a workflow that holds a secret states the rule (nothing compiled or run of the product while a key is loaded) up front.
- The docs worker (Sonnet) spent two full runs finding a formatting gate and a golden file. **Change:** said in its next brief (fmt and the named golden test before the full run); it then used one. To go in `worker-testing.md` if it recurs.
- The coordinator's wording "or a `recto serve` that has the key" went into a help string unverified by the worker; checked after against `find_keys::plan` (true: a state folder of its own gets a key file) and left.
- A draft of `PRIVACY.md` said a program "signed by another developer" was refused; what the probe ran was another identifier of the same team. Caught in the coordinator's read of the diff. A claim about the OS is held to the probe's lines, again.

**Numbers.** Workers: the encryption worker (Opus) about 800,000 tokens over the day's resumptions, no hand-back sent back for judgement; the restore-test worker (Opus) 273,000 tokens, 20 minutes to a cause with a failing in-process test; the docs worker (Sonnet) 136,000 tokens over two tasks, three full runs then one; the measurement worker (Sonnet) 158,000 tokens, stopped correctly at a finding over the threshold. CI's longest job on the day's green runs: 875, 955, 1169, 916, 1193, 950, 770, 975, 1341, 1229 s; the two slow ones were slower in every stage by a similar factor (app-tests 279 s in a fast run, 405 and 343 in those), so the runner, not a stage. Median of the last five as `tools/push-main.sh` said it after f88d1386: 16 min 15 s against 21.

**Structure.** The key has one way in (`load_for_the_app`, held by a test that counts `load_or_create(` and `signed_as_a_release(` per file); the indexer holds only a key bound to its corpus; `recto` carries no Keychain code (a test on the built programs). Hubs: `corpus.rs` grew by a four-line guard; the sealing lives in `seal.rs` and `corpus/sealed.rs`.

## 2026-11-16, night: 0.1.1 published

- **Out:** 01:25 UTC, inside the owner's date ("Release should be 16gh morning the latest."). Checks and numbers are in PLAN's line for it.
- **What it cost that it needn't have: about 1 h 40 min.** The first tag's release run (43776362451) spent its hour in the gate and withdrew the tag. Cause, from its log: the gate asked for CI's run on the newest commit that changed code on any line of history, which under a `--no-ff` merge was the branch's commit, never the end of a push. 0.1.0's tag didn't show it because its newest code commit was the pushed head. The dress rehearsal and the two preflight workflows ran everything after the gate and not the gate.
- **What changes:** a release step that only a tag's push runs is run as written against `main` before the tag (the gate's step was, after the fix, in under a minute: that is the rehearsal it lacked). The gate gives up after ten minutes with no run started, not sixty. The review's two should-fixes are in PLAN before 0.1.2's tag.
- **Kept:** nothing was published on a failed check; the workflow withdrew its own tag; the fix had a full check, an independent review and a green CI run before the second tag.

## 2026-11-16, morning: after five merges (the owner's three requests of the morning, and two things the review started)

**Progress against the dates said.** Said at the start (the plan's commit of 07:19): the performance review, image selection in read mode and Capture each handed back on 16 November and merged by 17 November. Merged on 16 November, by the merge commits' times: the performance review 08:07, Capture 08:30, `qa budget`'s true times 08:40, the stall record 09:18, image selection 10:55, where the coordinator had told the owner mid-morning that it would be 17 November. The lines that missed, measured again with nothing else running: committed 10:11, the same morning. Not done: the owner's dev build, which waits on their yes to replacing the release in `/Applications`.

**What review found.** Three independent reads, the code and the rules given and not the workers' reports.
- Capture: nothing that blocked; six should-fixes, two a person would have met (Escape not reaching a waiting Capture with the sidebar holding the keyboard; a press in a comparison's other pane turning the tool off, there before the branch) and two tests that couldn't fail. The coordinator's read of the fixes then found the comparison fix wider than the rule (a third tab put in a pane kept the tool on); tightened with a test that failed first.
- Image selection, first read: three that blocked. The serious one was rule 1: nearly every still click on a page started a private render worker (351 MiB and 0.8 s on the heaviest map), which the worker's own hand-back had named only as "not measured". Second read: the same hole still open on pages whose pictures the tab's worker can't list, and six fixes.
- The stall record, read after its merge: sound; two things before its label is wired.
So a memory fault of the first order passed a full check, 17 new tests and the input-to-frame bound, and was caught by the read and by the coordinator's question in the reviewer's brief. **Change:** a brief for a feature that starts work on a click, a key or a pointer move asks for the count of processes and threads it starts and the tree's peak, on the heaviest file, as a number in the hand-back, as input to frame is asked for now. To go in `worker-testing.md`.

**Waste and mistakes.**
- Capture used six full runs (three failed) against the two a branch has: a golden its wording changed, a Swift compile error, an assertion on keyboard focus that a quiet window can't hold. Each was knowable before a full run: `--quick` runs the goldens, `app/build.sh` compiles the Swift, and whether `qa state` can see first responder is a question for one launch. **Change:** said in the next Sonnet brief that touches Swift: build the app and run the one launch test before the full run.
- Image selection's first worker ran out of room at about 700,000 tokens with the review's fixes unstarted; its notes for the next worker were good and the hand-over cost about ten minutes. It was the task the coordinator had started on Opus for its size; the feature and its first review round are one worker's room, the second round another's. **Change:** a review round on a branch of this size starts with a fresh worker and the first one's notes, not a resumption.
- The merge of `main` into the image branch left conflict markers in `SPEC.md` §6, committed, found by the next worker reading the row. Nothing checks for them. **Change:** a marker check in the tools' light job; in the plan.
- The coordinator's brief allowed image selection's worker to treat "boxes not known" as "look anyway"; the second review named it. The settled design should have said what a can't-say answer means.
- The permission check for shell commands was unavailable twice for a few minutes (one merge retried later; one worker's temp folder of two PDF copies left in `/private/tmp/claude-501/btt-vuZ0`, said to the owner, not removed by the coordinator on a worker's word).

**Numbers** (from the tools and the workers' own counts, read 2026-11-16 about 11:00).
- Full runs by branch (`tools/check.sh --runs`): `build/capture-once` 6 (3 failed, 1,456 s), `build/image-select` 4 (1 failed, 1,278 s), `build/budget-true-times` 1 (433 s), `build/stall-record` 1 (300 s); `main` ran one for real (the merge of the budget fix on Capture), the others counted as identical code.
- Workers, tokens and minutes to the last hand-back: the performance review (Opus) 308,000, 46 min, not sent back; Capture (Sonnet) 303,000 over three rounds, about 90 min of work, sent back once by the review and once by the coordinator's read; image selection's first worker (Opus) 696,000, about 85 min, handed back unfinished for room; its second (Opus) 747,000 over two rounds, about 110 min; `qa budget` (Sonnet) 141,000, 29 min, not sent back; the stall record (Sonnet) 147,000, 26 min, not sent back; the three reviews (Opus) 155,000, 231,000 and 237,000, 5 to 10 min each.
- CI's longest job on the day's four green runs with code (`tools/push-main.sh`): 16 min 56 s, 20 min 34 s, 15 min 42 s, and the image selection push's when it lands, against 21 minutes; the median of five 18 min 40 s. The next two were 19 min 43 s and 19 min 44 s, so it was looked into the same morning, from the runs' own logs: between run 43830180421 (15 min 42 s) and run 43850516861 (19 min 44 s) every stage is 30 to 60% longer, the ones that build and test nothing of the day's features among them (the ratchet 5 to 7 s, the headings' tests 20 to 31 s, the bundle 22 to 36 s, the app build 103 to 159 s), and the app tier 347 to 362 s. So it reads as a slower runner (two of the day's runs also never got one), not as seconds the merges added; the app tier's 15 s isn't explained by that alone and may be image selection's. The median of five is now 19 min 43 s against 21: no warning, and the next green runs say whether it comes back down.
- Tests added that launch the app: none. Seconds added to the check, by the workers' counts: about 2 s (Capture), 0.3 s (the stall record); image selection's 50 in-process tests not timed apart.

**Structure** (`tools/ratchet.sh --report 4b4e027c`, before image selection's merge was counted: 21 lines added to hubs, 11 in `recto-agent/src/table.rs`, 8 in `recto-ffi/src/lib.rs`). Each feature is in its own modules (`snapshot_menu.rs` and the tool's file for Capture; `image_pick.rs` in the reader, `image_select.rs` in the app; `watchdog/short.rs`; `qa_times.rs`). Left from the reviews, in the plan: the image's keep rule in `Selecting::press` and `ended` called from six places. No ratchet number was raised. The code-review graph's structure check wasn't run this time: owed at the next review.

**Sonnet and Opus.** Sonnet held the three small tasks (Capture, `qa budget`, the stall record) with no hand-back sent back for judgement; Capture's extra full runs were process, not judgement. Image selection was right to start on Opus and was two workers' work.
