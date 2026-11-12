# RectoPDF: why development is slow, and the restructuring that fixes it

Reviewed 2026-11-12 at `main` = `9a842f1`, after the 0.1.0 release. Owner: "I have noticed extremely slow development speeds on this project, this is linked to our development process, slow testing, the fact that bugs and mistakes are not caught until late in the dev process (often durin final testing), and slow build times." And the goal: "after this project is done the codebase is easily maintainable and coding projects are efficient and fast."

This replaces the step list in `docs/reviews/2026-11-08-architecture.md` §4.6. That review's findings still hold and are not repeated here; its numbers are re-measured in §2.5.

**Sources.** `git log` (858 commits, 129 merges), the full-run ledger (182 runs), the build queue's report (932 jobs), `gh run list` (80 CI runs) and one green CI run's log for per-test times, the code-review graph, and builds timed on this Mac on 2026-11-12 at load 2 to 5. Three read-only investigations classified commits, tests and structure; where a number is their judgement and not a count, it says "estimate". §8 lists what wasn't checked.

---

## 1. Verdict

**The code gets written fast. Everything after it is slow, and the cause is one structural fact: the code that changes most can only be tested by launching the real app.**

- A feature's code takes 25 to 80 minutes. The branch then spends 1.5 to 5 hours in review rounds, fixes, full checks and the merge: 60 to 85% of its life (estimate, six branches).
- The reader was feature complete on 2026-11-08 at 13:13. The release came on 11-12. About 195 of the 209 commits in between were fixes, test fixes and tooling: 29% of all commits and about a third of the active hours (estimate).
- Every failing test in the 14 red CI runs since 11-11 launched the app. None of the 1,904 other tests failed. Locally, 29 of 52 failed full checks failed only after the app was built, 4 to 8 minutes in.
- `recto-ffi`, the app's core, is 14,468 lines of mostly rules with 100 unit tests. Its two central functions, `reconcile` (500 lines) and `session_thread` (353 lines), have none. The fast check never runs the code every feature changes.
- A person's click and an agent's command run through two separate implementations that meet only at `Session`'s methods. So the CLI's tests don't test what the window does, and bugs live in the difference.
- **Builds are not the problem.** After an edit to the session hub: 4 s to check the crate, 16 s for clippy over everything, 19 s to rebuild all 72 test binaries, 17 s for the app. Of a 514 s full check, about 55 s is compiling.

The restructuring below is ordered by that finding: first make the fast check reach the app's logic, then make one implementation serve both the window and the CLI, then split the hubs, and prove it on a real feature with numbers.

---

## 2. What was measured

### 2.1 Pace

| | |
|---|---|
| Calendar | 2026-11-06 to 11-12; work on five days |
| Commits | 729, plus 129 merges |
| Code | 164,556 lines of Rust and Swift |
| By class (commits) | feature 225, product fix 177, test fix 103, tooling/CI/queue 67, docs 135, release 20 |
| Fix + test fix + tooling against feature | 347 against 225 |
| Last feature merge | image copy and Print, 11-08 13:13 |
| After it | 209 commits, about 13 of them new capability |

### 2.2 Where a branch's time goes (estimate from commit times and ledger rows)

| Branch | Writing the code | From last feature commit to merge | Full checks in that tail |
|---|---|---|---|
| Links | 24 min | 82 min | 4 |
| Find | 24 min | 159 min | 5 |
| Save stage 1 | 43 min | 4.9 h | 4 |
| Comparison | 37 min | 4.5 h | 4 |
| Image copy and Print | 79 min | 107 min | 3 |
| Dev updates | not visible (squashed) | 229 min | 5 |

Time before a branch's first commit isn't visible, so writing is understated.

### 2.3 Where bugs were found (196 product-fix commits, classified by subject and body, ±10%)

| Found by | Fixes |
|---|---:|
| Independent review | about 72 |
| The worker while building | about 56 |
| Fuzz, corpus, real files | 22 |
| CI only | 13 |
| Full check under load | 8 |
| Slow mode (existed from 11-11 17:07) | 7 |
| The owner using the app | 9 commits covering about 25 items |
| Soak and budget runs | 5 |

| Kind | Fixes | Cheapest layer that could have caught it (estimate) |
|---|---:|---|
| Lifecycle and timing: launch, quit, late state, update, sleep, workers, sockets | 72 | about 60% a core test on a fake clock, if the lifecycle were a state machine in the core; 25% only the real app; 15% only another OS |
| Policy and security: levels, write paths, protected PDFs, signatures | 61 | about 80% a unit test on one shared check. Save As writing anywhere was `render`'s check not reused |
| PDF content | 31 | a CLI test and a corpus survey |
| State restore | 9 | a session test |
| UI behaviour and layout | 8 | the real app or a look at a screenshot |
| Memory | 6 | the memory binary |

Features that needed follow-up merges after landing: session restore 4, the updater 4, deep sleep 4, the sandbox 4, quiet runs 3, Save stage 1 3. Each tail ran 3 to 4 days. Five of the six are process and OS lifecycle. Find needed one, an hour later.

Of about 25 things the owner found, about 15 were taste (where the zoom level goes), which no test can know. About 10 were visible in a screenshot or in `qa state`: scanned pages white on dark paper, two tabs of one PDF, Check for Updates missing from the menu, the app flashing on screen in tests.

### 2.4 The checks

**Full check, from the ledger:** 182 runs, 14.2 hours. 130 passed (10.9 h), 52 failed (3.3 h).

| A pass took | 11-07 | 11-08 | 11-11 | 11-12 |
|---|---:|---:|---:|---:|
| Average, seconds | 129 | 294 | 356 | 514 |

It quadrupled in five days, and all of the growth is tests that launch the app.

| Failed at stage | Runs | Seconds in, average |
|---|---:|---:|
| app-tests | 24 | 314 |
| other-tests | 10 | 124 |
| changed-tests | 8 | 102 |
| clippy, fmt | 5 | 3 |
| registrations, late-state, slow | 5 | about 420 |

**The tests, from a green CI run (CI is slower than this Mac):**

| Layer | Tests | Test-seconds | Wall |
|---|---:|---:|---:|
| Workspace, no app | 1,904 | 515 | 172 s |
| Launching `RectoPDF.app` | 217 | 1,568 | 394 s at four at once |

App tests are 10% of the tests and 75% of the test-seconds. By what they check (classified from names and doc comments, ±10 tests):

| Class | Tests | Seconds | |
|---|---:|---:|---|
| A Rust rule or state | 90 | 341 | could run on an app core with no window |
| Swift drawing, layout, AppKit | 44 | 148 | needs the window |
| Process and OS: launch, quit, kill, update, Launch Services | 68 | 712 | 27 update tests are about 410 s; roughly a third check a Rust decision |
| Memory and performance | 15 | 367 | |

Every test launches its own app. A launch costs about 1 s, so launching isn't the cost: multi-restart update tests and deliberate waits are.

**`tools/check.sh --quick` misses what fails later:**
- It tests only crates with uncommitted changes. After a commit it falls back to the whole workspace.
- It never tests dependents. An edit to `recto-session` doesn't run `recto-agent`'s table guards or `recto-cli`'s wiring guards, which then fail about 2 minutes into a full check.
- It never builds the app. The whole-state restore guard, the most-failed test, runs only in app-tests, 4 to 8 minutes in.
- In the queue's last 30 hours, 46 of 156 failed jobs were compile, clippy or fmt errors: jobs queued to learn what `cargo check` says in seconds.

**CI:** 80 runs, 32 failed (40%); on `main` 23 failed against 31 passed. Median 12 to 15 minutes; a green run is about 25% building and 75% running tests. `main` was red about 12.5 hours up to midday on 11-11. Since 11-11, CI gates every merge.

### 2.5 Structure today, against 2026-11-08

| Thing | 11-08 | Today |
|---|---|---|
| `Session` | 34 fields, 180 methods | 36 fields, 193 methods, one `impl` of 2,732 lines |
| `Reader` (`recto-reader/src/tiles.rs`) | 40 fields, 141 methods | 43 fields, 155 methods |
| `Inner` (`recto-ffi`) | 34 fields | 45 fields |
| `AppCore` | not counted | 153 methods, 129 exported; Swift calls 136 of them at 218 sites |
| `Command` | 59 variants | 68 variants |
| `execute_held` | 262 lines | 300 lines |
| `CoreCall` | 34 variants | 39 variants |
| `session_thread` | 267 lines | 353 lines |
| `reconcile` | 191 lines | 500 lines |
| `recto-ffi` | 6,474 lines | 14,468 lines, 100 unit tests |
| `recto-agent/src/table.rs` | 1,918 lines | 5,358 lines; `TABLE` is one 3,440-line static |
| Swift QA verbs | about 50 | 73 |

Steps 1 to 3 of the old review are done and worked: lines going into hub files fell from about half of a feature to 2 to 15%. But no feature of links' size has landed since, and the hubs kept growing through fixes: the two late-restore fixes put 66 to 80% of their lines into `recto-ffi/src/lib.rs`. Steps 4 to 13 haven't started, except Find's own call enum.

**The two paths.** A person's action goes Swift → an `AppCore` method → a `CoreCall` → `session_thread` → `Session`. An agent's goes `recto` → socket → `recto-control` → `Command` → `execute_held` → `Session`. `recto-ffi` never calls `recto_agent::execute`. Each side has its own tab selection, waking, events and replies.

| Feature | Shared? |
|---|---|
| Follow, back, forward, go to page, close tab, rotate, undo and redo, compare, Find | Two wrappers over the same session methods. Events derived twice. Find's agent path is patched afterwards to wait as the app's does |
| Zoom | Three sites: Swift drives the engine, `recto-agent` refuses it, `recto-ffi` intercepts it |
| Save, the ⌘K palette | One implementation: the app builds a `Command`. This is the model |

Of `CoreCall`'s 39 variants, about 14 repeat a command, about 8 are actions an agent can reach only through `qa` (edit mode, contents folds, pane focus, synced scroll), and about 17 are internal news (estimate from reading the arms). About 25 of Swift's QA verbs are stand-ins for an action a command already does or should do.

**Swift is thin** (6,121 lines, no user-facing wording, enable rules and timer lengths from Rust). Its cost is concentration in three files and a 136-method bridge.

**Test homes.** `recto-cli/tests` holds 21,306 lines in 23 binaries against 1,674 lines of CLI source. Seven of those binaries test the app, which has no test home of its own. Ten (about 4,700 lines) call `recto-agent` in process and never run the CLI. Every edit to session, agent, control, core, index or platform relinks all 23.

### 2.6 Builds (this Mac, 2026-11-12, load 2 to 5, one sample each)

| After touching `recto-session/src/lib.rs` | Seconds |
|---|---:|
| `cargo check -p recto-session` | 4 |
| `cargo check --workspace --all-targets` | 15 |
| `cargo clippy --workspace --all-targets` | 16 |
| Its own 173 tests, built and run | 6 |
| Every test binary rebuilt | 19 |
| `app/build.sh` (Rust, bindings, Swift, signing) | 17 |
| `app/build.sh` after a Swift-only edit | 7 |
| The app tests (215, four at once) | 157 |

Check, clippy and test builds each keep their own artifacts, so a hub edit is compiled three times. That is the only build waste found, about 30 s a cycle. `docs/reports/build-performance-2026-11-11.md` found the same: linking is 0.1 s a binary and sccache gets no hits.

---

## 3. Root causes, by cost

1. **The app's logic is only reachable through the app.** 14,468 lines in the binding crate, tested by 217 launches. This makes the fast check blind (§2.4), puts every failure at minute 4 to 8 or on CI, and grows the full check with every feature. *Fix: R1.*
2. **Lifecycle is code that's right only when it's fast.** Launch, restore, quit, update and sleep are threads and callbacks, not state machines with a clock passed in. 72 of 196 fixes, 3 to 4 follow-up merges per feature, the most-failed tests. Slow mode found five more on its first run, on day 4. *Fix: R1.4.*
3. **Two implementations of every action.** The CLI-first rule holds at `Session`'s methods and no higher. *Fix: R2.*
4. **Review is the main bug finder and it comes after the code.** About 72 fixes, 11 to 26 findings a round, second rounds no smaller. Policy checks are copied per feature, so a new write path forgets them: 61 fixes. *Fix: one shared check per policy with a guard (R4.2), a test list in the brief, and structure that makes the reviewer's reading smaller.*
5. **Tests written against one machine's speed.** 103 test-fix commits. The written rule ("wait for a condition") was named in four process reviews and recurred after each. *Fix: fake-clock tests (R1), a ratchet on fixed sleeps (R0.3).*
6. **The check runs everything every time, and nobody budgets its length.** A Swift-only change runs 1,904 Rust tests; an updater change runs every window test. *Fix: R0.1, R0.2, R0.4.*
7. **The hubs still grow.** Rule 7 stopped features being threaded through them, but nothing shrinks them and nothing fails when they grow. *Fix: ratchets (R0.3), then R2 and R3.*
8. **Defects the owner found that a screenshot showed.** About 10. *Fix: a contact sheet of every feature's shots, light and dark, looked at before any hand-over (R4.2).*

One pattern across the process reviews: **a fix held by a tool stuck, and a fix that was only a written rule recurred.** The full-run budget, the pass memory, rustfmt and the queue's refusal all held. "Tests wait for a condition", "fix a red main first", "process reviews every fourth merge" and "evidence before explanation" each came back. So every process change below is a tool, a test or a ratchet.

---

## 4. Target structure

```
 people                                agents
   │                                     │
 RectoPDF.app (Swift: draws, OS)       recto (CLI)
   │ UniFFI                              │ socket
 recto-ffi  (adapters only)            recto-control (server, levels, records)
   └───────────────┬─────────────────────┘
                   ▼
              recto-app      the app core, no toolkit, no UniFFI
                ├ run(action, actor)   the one way anything happens
                ├ model                view models, a pure function of state
                ├ lifecycle            launch, restore, quit, update, sleep: state machines on a Clock
                └ <feature>.rs         each feature's view part, menu items, QA verbs
                   ▼
              recto-agent    commands: one registry, a module per feature
                   ▼
              recto-session  components: tabs, editing, journals, sleep, contents, trail, compare, view
              recto-reader   components: tiles, selection, links, zoom, panes, paper
                   ▼
   core · index · render · render-service · svg · update · sign · proto · platform
```

Five rules:

1. **One action, one implementation.** Everything a person or an agent can do is a command, run by one executor with the actor passed in. The window sends the same command the CLI sends. A test proves every menu item, palette entry, key and control maps to a command or is on a short host-only list.
2. **The app core is a crate with no toolkit.** `recto-ffi` converts types and forwards. A rule that can only be tested by launching the app is in the wrong place.
3. **Time and order are inputs.** Lifecycle code is `handle(input, now) -> intents`. Its tests deliver arrivals in every order on a fake clock, in milliseconds.
4. **A feature is a module per layer with the same name,** and one registration line in each: its state component, its commands with their table rows and wording, its view part and menu items, its pane in Swift if it draws something new. `docs/FEATURES.md` shows how in one page, and a scaffold creates the files.
5. **Budgets on structure, held by a ratchet.** A list of today's oversized files, functions, structs and enums that may only shrink, checked in seconds.

What stays: the crate layering with no cycles, the command table as the one list, the seams (`DocLoader`, `SessionAccess`, `Host`, `RenderBackend`), the published snapshot the UI thread reads, per-item size asserts, the exhaustive destructures, thin Swift.

---

## 5. Target test system

A test's tier is what it needs, and each tier has a time budget that is checked like a memory budget.

| Tier | What | Runs | Budget |
|---|---|---|---|
| 0. Gate | fmt, clippy on what changed and its dependents, the ratchets | before anything is queued | 20 s |
| 1. Core | unit tests and in-process tests: session, agent, the app core with a fake listener, clock and disk; every registration guard | `--quick`, on what changed, its dependents' guards always | 60 s |
| 2. Process | the CLI, sockets, the render worker, the installer | `--full` when a crate under them changed | 60 s |
| 3. Window | the real app, only for drawing, AppKit and the bridge | `--full` when the app, the bridge or the app core changed | 90 s |
| 4. Lifecycle and soak | updates end to end, Launch Services, long sessions, a thousand tabs | when update, platform, restore or app code changed; always on CI | 4 min |

- **`--quick` answers "is this ready for review?" in a minute**, because the app core's tests are tier 1.
- **`--full` runs what the change can affect.** CI runs everything on every branch and on `main`, so nothing is tested less before a merge.
- **The slow stage shrinks** to the few paths only a real launch has; orderings are covered in tier 1.
- **A new test names its tier in its doc comment.** A test that takes its tier over budget replaces something or moves.
- **Durations are kept per test** from every full run. A process review reads the 20 slowest and the totals per tier.

Targets, against today: quick 60 s (today it can't reach the app at all); a typical full check under 4 minutes (514 s); CI under 10 minutes (12 to 15); app-launching tests about 110 (217), none checking a Rust decision.

---

## 6. Target process

| Change | Held by |
|---|---|
| A brief names the feature's modules, the shared files it may touch, and its tests by failure and tier | the brief template in `docs/agent-instructions/`, the hub-touch report on the branch |
| Structure can't decay unseen | the ratchet in `--quick` |
| No fixed sleeps in tests | a count that may only fall |
| Compile errors never reach the queue | the gate |
| Registration lists fail in seconds | guards in tier 1, run for dependents |
| Every write path, level check and protected-PDF check is the one shared function | a guard test over the command table and `clippy.toml`'s disallowed methods |
| Screenshots get looked at | `app/qa/shots.sh` makes one contact sheet, light and dark; the hand-back names it |
| Plan and engineering docs stop conflicting | workers don't edit `docs/PLAN.md`; the coordinator ticks it in the merge. `docs/ENGINEERING.md` split per crate |
| Process reviews happen | `check.sh --runs` says how many merges since the last entry in `docs/PROCESS.md` |
| Check time stays in budget | the time ledger, read at each review |

---

## 7. The plan

Every step is its own branch, behaviour-preserving, mergeable alone, about a day where it can be, and reports its numbers before and after (quick time, full time, the sizes it was meant to shrink). An app test is deleted only when its in-process twin has been shown failing on the bug it names. No feature work starts until R4.3.

Sizes are one worker's days including review, by judgement.

### R0. Fast feedback and guards (tools only)

| # | Step | Proof | Days |
|---|---|---|---|
| R0.1 | `--quick` covers changes since the branch left `main`, runs dependents' registration guards, and a compile gate runs before a test job is queued | `check-test.sh` cases; a stale table row fails in under 30 s | 1 |
| R0.2 | `--full` selects by what changed; app tests split into window and lifecycle tiers; CI still runs all | `check-test.sh` cases; a session-only change skips the lifecycle tier | 1–1.5 |
| R0.3 | Ratchets: sizes of files, functions, structs, enums; fixed sleeps in tests; a hub-touch report per branch | each fails on a seeded growth | 1 |
| R0.4 | Per-test durations kept; `check.sh --slow` lists the slowest and each tier against its budget | the list matches a run | 0.5 |
| R0.5 | Workers stop editing `PLAN.md`; `ENGINEERING.md` split per crate | no merge conflict in either over R1 | 0.5 |
| R0.6 | CI's app tests in two parallel jobs | CI under 10 minutes | 0.5 |

### R1. The app core out of the bridge

| # | Step | Proof | Days |
|---|---|---|---|
| R1.1 | `reconcile`'s projection into a pure `model` function | 20 unit tests: notice precedence, the Save icon, pane labels, contents changes | 2–3 |
| R1.2 | `recto-app`: `Listener`, `Clock` and host-service traits; `recto-ffi`'s feature modules move; `Inner`'s 45 fields become narrow handles; `recto-ffi` adapts only | the same suites pass; `recto-ffi` holds no `with_session` | 5–7, in 3–4 merges |
| R1.3 | A headless app for tests (fake listener, clock, disk); the 90 rule-checking app tests ported; the whole-state restore guard in process | each ported test fails on its seeded bug; app-tests' seconds fall by about a fifth | 3–4 |
| R1.4 | Launch and late state, quit and hand-off, update, and the sleep writer as state machines on the clock; arrivals tested in every order; about 20 update tests ported | the late-state and update bugs fixed on 11-11 each fail a tier-1 test on the old code | 4–6 |

### R2. One action, one implementation

| # | Step | Proof | Days |
|---|---|---|---|
| R2.1 | A typed `Event` enum; session methods return what changed | "every event has a group" is a compile error, not a test | 1–2 |
| R2.2 | The command registry: a module per feature holding its payload, handler, table rows, wording and events; a `Route` column replaces four routing sites; one record-command implementation | a new command is one file and one line; no default arms for `draws`, `tab`, `reads_pages` | 5–7, a group per merge |
| R2.3 | A person's actions run commands: 14 parallel `CoreCall` arms go, 8 window-only actions get commands, internal news stays a small enum, Swift calls `run(action)` | the action-coverage test; the same reply and events from the window and the CLI for each action | 4–6 |
| R2.4 | A QA verb table; the 25 stand-in verbs replaced by commands; Swift's verbs checked against the table | "Swift answers exactly the table's verbs" | 2 |

### R3. Components

| # | Step | Proof | Days |
|---|---|---|---|
| R3.1 | `Session`'s `impl` and its 3,280 lines of tests split by concern (moves only) | none needed | 1–2 |
| R3.2 | `Session`'s fields into components, one per merge; a cold tab still ≤ 4 KiB | per-component tests; the scale test | 4–6 |
| R3.3 | `Reader`'s 43 fields into components the same way | its 60 tests, moved | 3–4 |
| R3.4 | Swift: `QAChannel` by area; a pane file per feature where one is next touched | app tests and shots | 1 |

### R4. Test homes, the feature kit, and proof

| # | Step | Proof | Days |
|---|---|---|---|
| R4.1 | In-process agent tests move from `recto-cli` to `recto-agent`; app tests get their own crate; the 15 fixed waits go; helper waits say what they saw | a session edit relinks under 15 test binaries (36 today) | 2–3 |
| R4.2 | `docs/FEATURES.md`, the scaffold, the brief template, one shared function per policy with its guard, the shots contact sheet | a scaffolded empty feature passes `--quick` | 2 |
| R4.3 | **Pilot:** one real feature from phase 4 built this way and measured, then a process review | the exit numbers below | 1–2 |

**Total: about 45 to 62 worker-days.** The old list was 24 to 37; this one adds the test system, the single action path, `Reader` and the pilot. For scale: phases 1 to 3 and 7 were sized at 31 to 47 and landed in about 57 active hours with three to five workers, so this is roughly one and a half to two working weeks. That is a projection from one data point, and refactors of shared files parallelise worse than features.

### Order

R0 first, all of it, in a day: it makes every later step cheaper to check. Then three tracks, about three workers:

```
 A (app):      R1.1 ─ R1.2 ─────── R1.3 ─ R1.4
 B (commands): R2.1 ─ R2.2 ─────── R2.3 ─ R2.4      R2.3 needs R1.2 and R2.2
 C (state):    R3.1 ─ R3.2 ─ R3.3 ─ R4.1
 then:         R3.4, R4.2, R4.3
```

R2.1 merges before R1.2 starts moving files, as events are emitted from 48 sites in 11 of them.

### Exit numbers, measured on the pilot

| | Today | Target |
|---|---|---|
| `--quick` after one crate's edit | can't reach the app's logic | ≤ 60 s, and it does |
| Typical local full check | 514 s | ≤ 240 s |
| CI | 12–15 min, 40% red | ≤ 10 min, under 10% red |
| App-launching tests | 217 | about 110, none checking a Rust decision |
| `recto-ffi` | 14,468 lines | adapters only, under 2,500 |
| `reconcile`, `session_thread`, `execute_held` | 500, 353, 300 lines | each under 80 |
| Largest struct outside per-item records | 45 fields | 15 |
| Shared lines a new command needs | 7 to 9 edits in 3 files | 1 line |
| Actions a person can do and an agent can't | about 8 | 0 |
| Pilot: shared files touched beyond registration | — | 0 |
| Pilot: follow-up merges in the week after | 3 to 4 for lifecycle features | 0 or 1 |

---

## 8. Risks, and what wasn't checked

- **A refactor this size can break behaviour silently.** Every step keeps the existing suites green, CI runs everything on each branch, and R1.3's ports are proved by seeding the bug. The owner's dev build is swapped after each track's merges, as real use finds what tests don't.
- **Affected-only checks can miss a dependency that isn't in Cargo's graph** (a script, a generated file, a QA script). The selection treats anything outside `crates/` as affecting everything, and CI runs all.
- **Running a person's actions through the command executor adds a hop on the UI's path.** It stays on the session thread as now; R2.3 measures input-to-frame before and after and keeps rule 3.
- **Memory.** Components must not scatter per-tab state: `TabRecord` stays one struct, and the 1,000-tab and cold-tab tests run on every R3 merge.
- **Not checked:** per-test times on this Mac (CI's were used); every app test's body (classified by name and doc comment); the roughly 35 failed full runs whose logs are gone; worker time before a branch's first commit; whether `recto-headings` or the fuzz crate need changes; Swift compile time beyond the 7 s measured.
- **Estimates** for "cheapest layer that could have caught it" and for branch time shares are judgement over classified commits, not measurements.
