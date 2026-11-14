# Review of the guide to building Rust desktop apps with agents, 2026-11-14

Owner, 2026-11-14: "I think we have learned a lot about rust app development. Especially around code structure, compartmentalization, cli
implementation at the core, the periodic code reviews, scaling of agent skills, etc. can you have a proper review and rewrite of the how we
develop rust apps guide? The idea is that someone using the guide will never have to repeat the refactor and redesigns we had to do"

Reviewed: `docs/agent-instructions/rust_app_development.md` at `main` = `13f54f88` (1,713 lines). Read beside it: `AGENTS.md`, `docs/PLAN.md`
(phase 7, phase R, the follow-ups), `docs/PROCESS.md`, `docs/reviews/2026-11-12-development-speed.md`, `docs/FEATURES.md`,
`docs/agent-instructions/feature-brief.md` and `worker-testing.md`, `docs/OWNER-RULES.md`, the merge messages of `git log --first-parent main`
since 2026-11-11, the unmerged `build/pilot-highlights` (R4.3) and `build/settled-gaps`, `tools/ratchet/baseline.txt`, the headers of
`tools/ratchet.sh`, `tools/push-main.sh` and the kept CI times (`recto-ci-times` in the git common dir).

## 1. Verdict

The guide holds most of what phase R taught, but a reader starting a new app on day one would still repeat phase R. Three reasons:

1. **The lessons are written as what happened, after the fact, not as what to build first.** §2.8 to §2.10, §5.5 and §6.6 are excellent
   post-mortems ("What happened", "Why", "Rules"), but they sit in the order they were learned. The checklist that should turn them into day-one
   work is §14, the last section, and its items are one line each with no reason.
2. **The second half of the lessons isn't in it at all.** Everything learned from 2026-11-13 on (whole-type counts, registries that pass parts,
   capabilities as types, headroom and extension points, one definition of settled, policy walks and their fixtures, the CI budget, scaling
   agents, review without the worker's report, guards before a move) is in `AGENTS.md`, `PLAN.md`, merge messages and briefs, or nowhere.
3. **About 15 sections are an unnumbered tail** after §11 (from "Run whole suites rarely" to "A fresh worker for each stage"), appended as
   lessons came. Several repeat earlier sections, and a day-one reader wouldn't find "Code that's right only when it's fast", which is the
   design rule behind the single largest class of fixes (72 of 196).

The guide also says nothing about what is still unsolved, so a reader would take a pattern like "one file and one line per command" as done.

## 2. What it gets right and should keep

- **Research before code (§1)** and its sign-off gate: the reason the core held up through dozens of features on both projects. Kept whole.
- **Rust decides, the host draws (§2.1)**, seams (§2.2), bindings pinned to the slowest generator (§2.3), the UI thread never waits (§2.4),
  one writer for shared state (§2.6), official routes only (§2.7). All still true on RectoPDF; kept, tightened.
- **§2.8 to §2.10** (compartments, one action, the app core crate): right, and RectoPDF's phase R proved each. Kept, with the numbers phase R
  measured since, and turned from post-mortem into instruction.
- **Performance and memory (§3)**: the marginal cost per idle item, power tiers, leased buffers, caps, `size_of` asserts, before-and-after
  numbers. RectoPDF's cold tab (304 B, 6 allocations, held at compile time through every move) is this section working. Kept.
- **The agent interface (§4)**: quiet runs, in-app shots, no synthetic input, assert on the model, local-only CLI with levels. Kept.
- **Hold every fix with a tool (§5.5)**: the strongest process finding of both projects (eight process reviews: every tool held, every
  sentence recurred). Kept, and used as the test for every rule in the rewrite: where a tool enforces it, the guide says what the tool checks.
- **Tiers and a time budget for the check (§6.6)**: kept; RectoPDF built it (R0.1 to R0.4) and it held.
- **The Mac, Windows, several computers, design, git and releases, the user**: kept, tidied. See §6 for what wasn't re-verified.

## 3. What is stale, contradicted, duplicated or buried

| Where | Problem | In the rewrite |
|---|---|---|
| §2.8 "a struct past about 12 fields or 40 methods" | A review rule with no tool, counted per struct. RectoPDF's ratchet counted per `file::Type` and missed `Reader` (about 140 methods over 11 files) and `Inner` (about 119 over 19). Budgets now 15 fields, 60 methods per whole type | §2.14: count the whole type across files, by tool, from the first commit |
| §2.8 "Registries, not switches" | Says nothing of a registry's signature; RectoPDF's took `&Arc<Inner>` and forced every feature to add a function coupled to all of it | §2.12 |
| §2.8 "add the extension point first, as its own commit" | Reactive: the pilot paid for it at feature time (10 move-only commits) | §2.12: extension points for each kind of feature before the first of that kind |
| §6.6 "Record today's values and let them only shrink" | Set at today's number with no headroom, every hub is at its ceiling the day after; the first feature after phase R hit about a dozen | §2.14: budgets with headroom, set before the feature arrives |
| §6.1 items 2, 3 and 7 (`check.sh --base master`, `--base public`, "`--full` only when the user asks") | Seedwell's check. RectoPDF's `--full` selects by what changed and is the hand-back gate; "full only when asked" contradicts AGENTS.md | §6.1 rewritten around what the check must do, not one project's flags |
| §6.3 "Timing tests run twice locally ... `taskpolicy -c utility`" | Later practice: `--like-ci` limits the test's own process; new tests run 20 times like CI before hand-back | §6.3 |
| §5.4 "Merge first", "Cap the review rounds", §11 tail "Run whole suites rarely", "Failures that only happen elsewhere" (suite counts) | The suite-count story is told three times (§6.1 item 8, the "Run whole suites rarely" tail, "Failures that only happen elsewhere" bullet 4) | Once, §6.8 |
| "A fresh worker for each stage" (tail) and §5.x | Duplicated, and doesn't say how a worker hands over when it runs out of room | §5.6 |
| "Tests that wait, and tests that guess" (tail) and §6.3 | Overlap | §6.3 |
| "Code that's right only when it's fast" (tail) | The most important design rule for lifecycle, buried at line 1604 | §2.13, Part I |
| "A red main hides the next failure" (tail) and §5.4 | Rule found in PROCESS four times before a tool held it | §6.9 with the tool (the push watcher refusing the next stamp) |
| §5.3 "Run a no-build review on every significant diff" | Didn't say the reviewer must not be given the worker's report; RectoPDF merged about 25 branches on reports and checks and the review that followed found an agent route to acting as the person | §5.3 |
| §0 item 6 "a full local check before hand-back or push, CI last" and §6.1 item 5 | RectoPDF's owner moved CI from gate to watcher on 2026-11-12 (GitHub had no macOS runners); local evidence decides a merge | §6.1, §12 say both arrangements and when each fits |
| §14 checklist | One line per item, no reasons, at the end | §0.2, with the section and the incident per item |
| §13 table | Seedwell's files and flags | §14: each tool by what it checks, so a new project can rebuild it |

## 4. What is missing

Each of these is in the repo (cited) and in no section of the guide:
- **Whole-type counts** (AGENTS "A structure number is reported for the whole type"; merge of `build/ratchet-by-type`).
- **Parts with private fields, capabilities as types with one constructor, the rule stated once with a source scan** (R3.5a, R3.7 S5 and S6
  merges: `KeepsRecord`, `StartsWriter`, `Carrier`).
- **Registries that pass the parts a row needs** (`e86442af` on `build/pilot-highlights`: 21 of 98 whole-`Inner` functions are registry entries).
- **Headroom and extension points per kind of feature** (`build/pilot-highlights`: 10 move-only commits and 2 raised ceilings).
- **Typed command payloads and a closed refusal table** (R2.5; the bug it found: `view` applied a layout and then refused the paper).
- **One definition of "settled"** shared by the test kit and the app's QA wait, waiters woken rather than timers (`0b0cdd61`, `64bf7033`).
- **A background thread holds the narrowest part** (R3.7 S5: three latent bugs when the record's threads stopped holding the whole app).
- **Guards before a move**, shown passing on the old code; byte-identical bindings; input to frame bounded (AGENTS; R3.5 to R3.7 merges).
- **Policy walks over every command, and their fixtures** (R4.2; `953d8f8c`: a Save by an agent below Full dropped the owner's undo steps
  for highlights, past walks that had only turns in their fixture).
- **Review without the worker's report, every branch; the coordinator reads the last fix diff**; "which input does no test use?" (AGENTS;
  PLAN R2.4 notes on the kit's 0-high viewport).
- **The CI-time budget with a median rule and the app-launch ratchet** (AGENTS "CI doesn't get slower"; `tools/push-main.sh`).
- **Scaling agents**: cheaper model first, when to move a kind of task up, design-is-the-work tasks start on the stronger model (AGENTS).
- **Decisions**: behaviour decisions checked against the owner's written rules first (the pilot's scope cut on undo past a Save, reversed by its
  review against the owner's rule of 2026-11-07); a denied permission goes to the owner.
- **Per-action cost counted, not timed** (`build/publish-per-tab`: 93 to 615 µs per undo at 1,000 tabs, past every check and two reviews).
- **What is unsolved** (§5 below).

## 5. The redesign we had to do, the day-one decision that avoids it, and where the guide says it

"Old" is the guide as reviewed; "New" is the rewrite's section.

| # | The redesign RectoPDF had to do (number or incident) | Source | The day-one decision that avoids it | Old | New |
|---:|---|---|---|---|---|
| 1 | The app's logic lived in the binding crate: 6,474 to 14,468 lines in four days, 100 unit tests; `reconcile` 500 lines and `session_thread` 353 with none; moved out in R1 (planned 5 to 7 d, several merges) | dev-speed review §2.5, PLAN R1.1 to R1.3 | Three crates from the first window: core, a toolkit-free app core with a headless kit, and a binding crate that only forwards, its size ratcheted | §2.10 (written after the fact), §14 item 1 | §2.10, §0.2 |
| 2 | Two implementations of every action: of 39 window calls about 14 repeated a command; zoom at three sites; about 25 of 73 QA verbs were stand-ins. R2.1 to R2.4: the bridge went from 19 action exports to one `act` | dev-speed review §2.5; R2.3 merge `59f75141` | One executor, `run(action, actor)`; the bridge exports `act` and views; a coverage test over every control | §2.9 (after the fact) | §2.9 |
| 3 | An agent's palette run ran menu items as the person (at Full it rotated a page of a password-opened document); a menu item became a command in two places | PLAN R2.3, "independent review of the night's merges" | The actor is passed in and each who-may rule is a method on it; an agent's action is the agent's by every route | not in the guide | §2.9 |
| 4 | Central types grew unseen: `Session` 180 to 193 methods, `Reader` 155, `Inner` 123 over 20 files; the ratchet counted per `file::Type`, so "138 methods to 39" was 140 over 11 files. R3.1 to R3.7: `Session` 192 to 103, `Reader` 139 to 82, `Inner` 113 to 44 in six slices | PLAN R3, AGENTS, `build/ratchet-by-type` | Ratchet the whole type across files (fields and methods) from the first commit | §2.8 as a review rule, no tool | §2.14 |
| 5 | Stringly typed payloads (`Option<String>`, seven flags, `format!("#{}", id)` parsed back, refusals routed by comparing code text); `view` applied a layout and then refused a bad paper | PLAN R2.5 | Payloads are types; the wire is their parse and print; refusal codes are a closed table | not in the guide | §4.2 |
| 6 | Lifecycle as threads and callbacks: 72 of 196 fixes, 3 to 4 follow-up merges per feature; R1.4 took five slices (slow stage 107 s to 38 s) | dev-speed review §2.3; PLAN R1.4 | Lifecycle is state machines with the clock and every wait injected; tested in every order | buried in the tail ("Code that's right only when it's fast") | §2.13 |
| 7 | Registries took `&Arc<Inner>`: 21 of the 98 functions taking the whole app were registry rows or answers; the pilot had to raise the count 98 to 100 | `e86442af` on `build/pilot-highlights` (unmerged) | A registry passes the parts a row needs, from the first registry | not in the guide | §2.12 |
| 8 | No headroom: the pilot feature (highlights) made 10 move-only commits and raised 2 ceilings (`save.rs` 2,105 lines, `record.rs` 1,561, `PageEdits` 17 fields and 68 methods, `Plan::write` 187 lines, `recto-ffi` 2,130, `host.rs` and `DocRenderer`, whole `Inner` 98, `Shared` 15); about 2.5 of 4.5 hours by the worker's estimate | `build/pilot-highlights` commit messages (unmerged) | Budgets with headroom; an extension point for each kind of feature (an edit kind, a page-drawing layer, a menu row, per-view state) before the first of its kind | §2.8 reactive only | §2.12, §2.14 |
| 9 | The fast check was blind and the full check grew 129 s to 514 s in five days; 46 of 156 failed queue jobs were compile errors | dev-speed review §2.4; R0.1 to R0.4 | Tiers, a compile gate before the queue, registration guards run for dependents, per-test durations kept | §6.6 (after the fact) | §6.6, §0.2 |
| 10 | 217 tests launched the app (10% of tests, 75% of test time), about 90 of them checking a Rust rule | dev-speed review §2.4 | A headless app kit in process, and a ratchet on the count of app-launching tests (225 to 200 so far) | §2.10, §6.6 partly; ratchet not | §2.10, §6.6 |
| 11 | CI red about 12.5 h over five stretches; each failure needed reruns to learn anything | PROCESS 2026-11-11; R0.7 | A failing test leaves an evidence folder and a rerun line; CI reruns a failure and gives its rate | tail ("Failures that only happen elsewhere") | §6.9 |
| 12 | No `rustfmt` and a hand-kept whole-state list: every pair of branches conflicted | PROCESS 2026-11-08; phase 7 steps 2 and 3 | `rustfmt` from day one; whole-state lists generated from types | §2.8 | §2.8 |
| 13 | Policy copied per feature: 61 of 196 fixes (Save As writing a hostile PDF to a shell's startup file) | dev-speed review §2.3; R4.2 | One function per policy, and a walk over every command for each policy | §5.5 (one function; walks not) | §6.10 |
| 14 | A quiet run started at Full and `open` took any path, so an agent on a copy at Off could rewrite the user's PDFs (in 0.1.0) | PLAN, `build/quiet-run-files` | The test mode's file wall is one statement, walked with an outside file in every value | §4.1 says isolated state, not the commands' reach | §4.1 |
| 15 | Two definitions of "settled" (kit and QA wait): about 1 in 5 failures on slow CI runners; the stricter one cost 424 to 490 test-seconds until waiters were woken | `0b0cdd61`, `64bf7033` on `build/settled-gaps` (unmerged); the after-figure from the worker's report | One definition of settled, shared, with waiters woken as the app comes to rest | not in the guide | §2.13, §6.7 |
| 16 | Threads holding the whole app kept it alive after it ended; holding only the keeper exposed three latent bugs | R3.7 S5 merge `baf3b227` | A background thread holds the narrowest part | not in the guide | §2.13 |
| 17 | Plan and engineering docs in 28 and 17 of 29 merges | dev-speed review; R0.5 | Workers don't edit the plan; reference docs per crate or feature | §5.5 | §5.2 |
| 18 | The ratchet's baseline conflicted on nearly every merge of two branches that each lowered a number (resolved by hand five times in a day) | PLAN, `build/ratchet-resolve` | The baseline merges itself (`--resolve`: the lower number per key) from day one | not in the guide | §2.14 |
| 19 | One 819-line QA channel and QA verbs as stand-ins (84 verbs, 7 stand-ins at the end) | R2.4, R3.4 | A QA verb table; verbs only for what only the host has; one file per area | §2.9 partly | §4.1 |
| 20 | Every action searched the tab list once per tab (undo 93 to 615 µs at 1,000 tabs), past every check and two reviews | `build/publish-per-tab` | Input to frame measured per change at 1, 100, 1,000 items; a guard that counts work per action, not time | not in the guide | §3.6 |
| 21 | Scrolling allocated 1,125 times in 50 steps (56 after) | R3.3 | An allocation-per-action guard from the first hot path | not in the guide | §3.6 |
| 22 | Tests that couldn't fail: the audits found tests that passed with their bug in place; 15 removed on proof in the first audit, 33 fixed in the second | PLAN "test audit"; AGENTS | Every test removed or rewritten on a seeded-bug proof; reviewers ask for it | §6.2 "check a test can fail" (partial) | §6.2 |
| 23 | CI crept to 33, 18, 28, 25 and 28 minutes in a day | PROCESS 2026-11-12 evening | A CI-time ledger and budget from the first CI run; a stern warning on every check when the median is over | not in the guide | §6.6 |
| 24 | About 25 branches merged in a night on reports and checks; the review after found an agent route to the person's actions, an action implemented twice, a ratchet hole | AGENTS; PLAN | An independent review of every branch, given the code and rules, not the report | §5.3 (no cadence, no "without the report") | §5.3 |
| 25 | The pilot's scope cut (a Save drops the steps that would undo a saved highlight) contradicted the owner's rule of 2026-11-07 ("after a save you might still have regrets") | `build/pilot-highlights` (unmerged) PLAN lines, `aa23152e`; OWNER-RULES | Behaviour decisions are checked against the written owner rules first; product decisions are the owner's | not in the guide | §5.8 |
| 26 | The test kit's view was 0 high, so a fix right after a jump was wrong during ordinary scrolling; every test moved by jumping | PLAN R2.4 notes | The test kit has real geometry; the reviewer asks which input no test uses | not in the guide | §5.3, §2.10 |
| 27 | Two or three full checks' app tiers at once took the machine to a load of 100 to 150 | PLAN, `build/one-app-tier` | One app tier at a time across worktrees, a lock with a bounded wait | §7.1 partly | §8.1 |

27 rows.

## 6. What the rewrite keeps without re-verifying

Out of reach from this repo, kept as written, tidied for wording and de-duplicated:
- §9 Windows (was §8): setup, the bundle-and-patch bridge, the PC never blocking, same design, Windows-specific lessons.
- §10 Agents on several computers (was §9): Seedwell's chat, skills layout, channels, watching, files, trust, a quiet agent.
- §1's Seedwell examples (tracker probe, File Provider spike, NordVPN route), §3's Seedwell numbers (cache sizes, `reqq`), §12's release flow
  and signing notes (§11.2.1, §11.2.2).

## 7. Length

Old: 1,713 lines. The rewrite's count is in its hand-back. Added sections, each justified:
- §0.2 checklist (moved from §14 and expanded with reasons: the day-one path the owner asked for).
- §0.3 what isn't solved (so a reader doesn't take an unsolved target as done).
- §2.11 to §2.14 (the skeleton decisions phase R paid for; rows 4 to 8, 15, 16, 18).
- §5.6 to §5.8 (agents and decisions; rows 24, 25).
- §6.7, §6.10 (settled, policy walks; rows 13 to 15).
- §7 restructuring safely (what to do when it slips anyway).
Cut or merged: old §14 (into §0.2), the unnumbered tail (each item into a numbered section), three tellings of the suite-count story into one.

## 8. Sources disagreeing with the lesson list it was briefed with

Each was put to the coordinator, who agreed and gave the details used here (2026-11-14).
- **R3.7**: five slices merged between 21:50 on 2026-11-13 (`build/inner-parts` started) and about 06:40 on 11-14 (S5 merged): about nine
  hours for five slices, with a sixth (S2b, the window's remaining reaches) and the quiet-machine measurement still open. Not "two days".
  Phase R as a whole: approved on 2026-11-12, planned at 45 to 62 agent-days, with its own dates running to Mon 16 Nov (PLAN).
- **`Inner` "113 methods over 20 files"**: the plan has 123 over 20 files when R3.7 was planned; S1's merge has 113 at its start, after R2.3.
- **The pilot** started at 06:13 on 11-14, before R3.7's last slice merged (independent work runs in parallel), so it met phase R's ceilings
  while phase R was still moving code. Its commits (an unmerged branch, `build/pilot-highlights`) show 10 move-only commits and 2 raised
  ceilings; "2.5 of 4.5 hours" and "six will be hit again" are the worker's estimates and are in no commit.
- **Registration lines per command**: `docs/FEATURES.md` lists about 8 to 11 one-line registrations (a `family!` variant, `pub mod`, a code line,
  `every_command`, a `UiAction` variant and its arm, a `ROWS` line, an `ANSWERED` line, a headless `mod` line, an ffi forwarding line, a
  contact-sheet step) plus `menus.rs`, where Find's take about a dozen lines in five places. The target was one file and one line.
- **Settled**: measured on a branch not yet merged (`build/settled-gaps`), 2026-11-14. Its commit `64bf7033` carries 424 against 490
  test-seconds for the first stricter definition and 162 to 135 for the seven slowest tests (three rounds). The figure after the fix, 425.9 s
  (the package's summed per-test medians, 20 rounds each side), is the worker's report to the coordinator, not in a commit.
- **"A script that could stop the owner's running app"**: commit `631a7a0f` on `main`. Corrected form: a QA script sent `recto stop` (and its
  exit trap) before it had set its temporary state, so the call went to the user's own socket. The app refuses `stop` from anything but a quiet
  run, so the person's app couldn't have been stopped, only a bare `recto serve` on the user's state. Two layers: the tool sets its temp state
  before its first call and is tested with a stand-in that records every call's state folder; the product refuses the dangerous verb from
  outside regardless.
- **"A count from memory was wrong three times in a week"**: PROCESS records one (a stale CI figure repeated to the owner); used as one.
- **CI times** agree: the kept green runs range 994 s (16:34) to 1,455 s (24:15); the median of the last five is 1,164 s (19:24) against the
  budget of 1,260 s (21 min).
