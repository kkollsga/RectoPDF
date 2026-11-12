# Handover, 2026-11-12 evening

For the coordinator who picks this up in a fresh session. Read `AGENTS.md` first, as always; this note says where the work stands, what to do next and what went wrong today, so you don't repeat it. Delete this file when phase R's next process review is written into `docs/PROCESS.md`.

## The state, in five lines

- `main` is `edde0c1` plus this note, pushed. Its local full check passed. GitHub's CI was green on `edde0c1` (read 2026-11-12, after the restart).
- The Mac was restarted to clear macOS's `xprotectd` (3.9 GB, see "The machine"). Nothing of ours was running.
- No worktree is left.
- The owner's RectoPDF is on development build 0.1.0.4, agent access Full, development builds allowed.
- Phase R (the restructuring) is the only project. Its plan, with every step's status, is in `docs/PLAN.md` under "Phase R". The review behind it is `docs/reviews/2026-11-12-development-speed.md`.

## What the owner wants

In their words, 2026-11-12: "after this project is done the codebase is easily maintainable and coding projects are efficient and fast." And late in the day: "Are we progressing at all?" The structure work has gone well. **The speed they were promised is not visible yet:** the full check is about 400 s (514 s that morning), CI is 12 to 15 minutes, and 229 tests still launch the real app. The next step is the first one that changes that. Report progress to them in those terms.

## Phase R: done, and next

| Step | Status |
|---|---|
| R0.1–R0.5, R0.7 | Done: the quick check with registration guards, the compile gate, a branch's full check by what changed, the ratchet, per-test times and `check.sh --slow`, the docs split, and "one failure is enough" (evidence folders, rerun lines, CI's N-of-M table, `tools/push-main.sh`, `check.sh --like-ci`) |
| R0.6 | Tried and undone: three CI parts needed five macOS jobs a run and GitHub couldn't supply them. CI's check is one job again |
| R1.1 | Done: the view model is a pure function |
| R1.2, R1.3 | Largely done: the app's whole core lives in `recto_app::core`; the bridge is 2,652 production lines (11,970 that morning). The headless kit exists and five app tests are ported; the rest of the ports wait on CI's per-test seconds |
| R2.1 | Done: typed events |
| R2.2 | The `recto-agent` side is done: a `Cmd` trait, eight families, 45 commands migrated. 23 commands and the routing lists are left (the list is in the merge of `build/commands`) |
| R3.1 | Done: `Session`'s `impl` and tests split by concern |
| R1.4, R2.3, R2.4, R3.2–R3.4, R4 | Not started |

**R1.3's kit merged the same evening** (the constructor, `HeadlessApp`, five ports; see the plan's R1.3 entry for the numbers and what's left). It showed that window-tier app tests cost under a second each on a quiet Mac, so the seconds are in R1.4.

**The next task** is R1.4 (launch, restore, quit, update and sleep as state machines on the clock; its inputs are listed in the merges of `build/app-core-3` and `build/core-move`), then R2.3 (the window sends the same commands as the CLI: the owner's rule, "all features should be implemented once").

## Rules the owner set today (all in `AGENTS.md`, in their words)

- A feature is implemented once; the CLI and the window call the same code (rule 4).
- Branches merge on local evidence: their full check, plus a `--like-ci` run of the lifecycle part when they touch launch, quit, update or sleep. GitHub's CI is watched after the push with `tools/push-main.sh` and no longer gates a merge.
- Workers start on Sonnet (`sonnet55-worker`) and call back at edge cases; a task where the design is the work, or that is too big for one worker's room, starts on Opus. Track each task's minutes, tokens, send-backs and full runs.
- One failure must be enough to debug: evidence, a rerun line, a rate.
- CI's machine is approximated here by process limits, not a virtual machine.
- Agent access on the owner's copy stays at Full; the app refuses an agent's raise, so the owner runs `recto permission set full` themselves if it's found Off.

## Open bugs and loose ends (each has an entry in `docs/PLAN.md`, "Found 2026-11-12")

- **Suspected product bug, CI only:** the contents pane has no frame after a second restart. Under a dated CI-only exception until 2026-11-18 (`crates/recto-cli/tests/restore/ci_known.rs`); search a CI log for `ci-known exception used`.
- **Not explained:** two restore tests failed once under other agents' app tests (a quit rewrote a sleep file; an old picture stayed). Evidence is kept in the main checkout's `target/handover-2026-11-12/evidence-kept/` (untracked; the temp folder it came from doesn't survive a restart), with the comparison-leak evidence, the bindings comparison scripts and today's screenshots beside it.
- **Seen once on CI:** the UI thread waited over a second during a print.
- **Small product bug:** an update can leave a dead Launch Services record for its unpacking folder; the owner's Mac has one.
- **The push guard** won't stamp a fix for a red `main` until the red run is marked known. It works, but it's clumsy.
- **Follow-ups** from the Update now and comparison-leak merges are listed in the plan (Settings says "ready" twice; the thumb across isn't in the entry-points check and its drag has no test).

## The machine

- 16 GB Mac. The build queue (`tools/sysguard/heavy.sh`) starts a job only above 40% free memory plus 10% per running job. All day the Mac sat at 37 to 48% free, so one job ran at a time. **Start the guards first:** `tools/sysguard/sysguard.sh` and `tools/sysguard/heavy.sh --watch`, in the background.
- `xprotectd` had grown to 3.9 GB after 19 days of uptime. It can't be killed, even by root. **The cause isn't known.** The owner was promised a measurement: record its size after the restart (`top -l 1 -pid $(pgrep -x xprotectd) -stats mem,cmprs`) and after each full check. If it grows per check, our thousands of built and signed test binaries are the cause; if not, it's uptime.
- After the restart, drop any `MIN_FREE=34` prefix once free memory is back above 50%.
- GitHub was short of macos-26 runners all afternoon. A run whose jobs never got a runner is not a failure; rerun it.

## What went wrong today, and what I'd do differently

1. **I merged over a red `main` twice.** I watched each branch's CI and not `main`'s own run after the push. `tools/push-main.sh` now does the watching; use it for every push of code.
2. **I split CI into three parts to make it faster and made it unable to run.** Five macOS jobs a run exceeded what GitHub would supply. Measure a CI layout's queueing before adopting it.
3. **I ran CI in a loop of eight reruns to prove an intermittent fix,** holding `main` and three branches for an hour. A deterministic failing-then-passing test is the proof; later runs are free samples.
4. **A review claim I repeated was wrong** ("ten test files never run the binary"): the spot check used the same shallow search as the claim. Check a claim by a different method than the one that produced it.
5. **I lost track of a finished branch** (`build/engine-load`) for half a day when `main` went red. Keep a list of branches waiting to merge.
6. **Five workers at once produced real memory pressure** and a queue that ran one job anyway. Three at most, and fewer when they all build.
7. **A new guard costs several rounds before it settles.** The ratchet needed four rules added in one day (moved code, baseline merge conflicts, restated records, a replaced path). Each was fixed in the tool with tests, never by editing numbers. Expect the same of any new guard, and never accept a worker's workaround of one.
8. **Workers' estimates of "what's left" ran low.** R1.3 took four workers and isn't finished. Tell the owner the count of workers a step has taken, not only the step's name.

## Worker tally, for the process review

About 25 worker tasks. Sonnet did well on specified work (the ratchet, the two window bugs, the `Session` split, the docs split, the leak's diagnosis) and needed sending back for judgement at the edges before the call-back rule; after it, call-backs replaced most send-backs. Opus tasks cost 275k to 445k tokens each and needed no send-backs: the lifecycle fix for the red `main`, the command registry, the three `recto-app` slices and the core's move. The most expensive mistake of the day was the coordinator's (items 1 to 3), not a worker's.
