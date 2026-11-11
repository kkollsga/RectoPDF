# Build and check performance, 2026-11-11

The owner: "Before kicking off everything can we check if we can improve build performance? The rust builds are really slow."

**The short answer: the Rust builds aren't what's slow. Running the tests is.** Compiling every test target of the workspace takes 25 s when the
dependencies are built, and 1 to 8 s after an edit. A full check spends most of its time running tests: about 50 s in the workspace's tests
and 191 s in the app's. What changed, each in its own commit on `build/build-speed`:

| Change | Before | After |
|---|---|---|
| App tests four at a time, not two (`check.sh --full`) | 191 s | 118, 123, 140 s (three runs, all passing, at 2 to 4 times the load) |
| Slowest test binaries start first (nextest priority) | 88.7 s, 58.1 s | 50.6 s, 46.9 s (back-to-back pairs, same load) |
| Worktree seeded from main's `target/` (`tools/worktree.sh`) | about 105 s of builds for a first check | about 45 s |
| `dev-release` profile for the owner's dev copy (`app/build.sh --dev-release`) | 86 s for `app/build.sh --release` after an edit | 50 s |

And one test race fixed on the way, in its own commit: `recto-control`'s peer-refusal test could panic on `EINVAL` under load.

## How, and under what load

This Mac: 10 cores (4 performance, 6 efficiency), 16 GB. Every build and run went through the build queue (`tools/sysguard/heavy.sh`),
with other agents' full checks, app builds and release builds coming and going in the other slots. Load averages are given with each number;
they ranged from 5 to 45. Nothing was measured alone (`HEAVY_SOLO` is the coordinator's), so every number is one sample under load, and a
difference smaller than about 20 % between two runs at different loads means nothing. Where it mattered, before and after ran back to back.
At three slots the queue gives each job `CARGO_BUILD_JOBS=4`, which these numbers include. Tools: rustc 1.99.1, cargo-nextest 0.9.148,
`cargo --timings`, and `-Z time-passes` through `RUSTC_BOOTSTRAP=1` for single units only, in this worktree, to split out link time.

## Phase 1: where the time goes

### Compiling

| What | Time | Load |
|---|---|---|
| No-op: `cargo build` of the workers, clippy, the Windows check, `cargo test --no-run --workspace` | 0 to 1 s each | 11 |
| Every test target, dependencies built, whole workspace rebuilt (107 units) | 24.8 s | 7 to 15 |
| Every test target, empty `target/` (245 units: 100 dependency libraries, 38 build scripts) | 30.6 s, 40.0 s | 9, 8 to 18 |
| After an edit to `recto-cli`'s `main.rs` (2 units) | 1.1 s | 23 |
| After an edit to `recto-ffi` (2 units) | 1.3 s | 20 |
| After an edit to `recto-svg` (49 units: it reaches `recto-agent`, then 26 test binaries) | 7.5 s | 23 |
| After an edit to `recto-session`, the hub (50 units, 27 test binaries) | 7.1 s | 21 |

The workspace is 18 crates with 51 integration-test binaries and 21 unit-test binaries. Of the 24.8 s of units summed over 4 jobs, test
binaries are 27 s summed, about half a second each, linking included.

**Linking isn't the cost.** The linker is Xcode's ld-prime (`ld-27106.1`). `-Z time-passes` on single units: `recto-cli`'s `app` test binary
spends 0.16 s in `run_linker` out of 1.5 s, `socket` 0.09 of 0.6 s, `recto-session`'s unit tests 0.10 of 2.8 s. All 72 test binaries
together are about 10 s of link time spread over 4 jobs. No other linker would win anything worth a change.

**sccache does nothing here.** Two builds into empty target folders made 229 cache misses and no hits: the cache is keyed by paths that differ
per target folder, and it sits at its 10 GiB limit shared with other projects. The workspace's own crates are incremental, which sccache
never caches (263 of 455 non-cacheable calls). It's set in `~/.cargo/config.toml`, which isn't this repository's, so it's left alone; turning
it off for this repository, or trying sccache's base directories, is a question for the owner.

**Profiles were already right for speed.** `[profile.dev]` has `debug = "line-tables-only"`, dependencies without debug info, and macOS's
default `split-debuginfo = "unpacked"` (no dsymutil step); incremental and 256 codegen units are the defaults. `[profile.test]` inherits it.
Turning debug info off would cost the line numbers in a failing test's backtrace, for little.

### The check's stages

`check.sh --full` on a warm target, from the stage lines the coordinator gave (6 to 8 minutes on `main`), against what I measured in this
worktree:

| Stage | Reported | Measured here | What it is |
|---|---|---|---|
| clippy (both workspaces) | 12 to 25 s | 9 s seeded, 31 s cold | compiling, check mode |
| windows | 7 to 12 s | 4 s seeded, 15 s cold | compiling, check mode |
| workers | 5 to 9 s | 5 s seeded, 27 s cold | compiling |
| headings-tests | 7 to 8 s | 11 s | 3 s build, the rest running |
| other-tests | 110 to 150 s | 50 s (load 11), 47 to 89 s (load 22 to 27) | about 25 s compiling at most, the rest running 1,717 tests |
| app-build | 16 to 18 s | 21 s | Rust, Swift, signing |
| app-tests | 150 to 240 s | 191 s at `-j 2` | all running: 169 tests, each a real app |

The reported other-tests times are larger than mine because that stage compiles every test binary first, and on `main` it often ran beside
another full check: each nextest run starts 10 test processes, so two checks put 20 tests and two builds on 10 cores.

### Running the tests

**Workspace tests (other-tests):** 1,717 tests in 72 binaries, 213 test-seconds, 50 s wall on 10 threads, so the threads were busy about
40 % of the time. 38 tests of 2 s or more hold 195 of the 213 s: `socket`'s watcher test 24 s, the installer's kill-at-any-step 18 s, fuzz
`smoke` 15 s, the installer's new-app-won't-start 12 s. The run waited on whichever of those started last: in every run before the change,
the installer's 18 s test was the last to finish. The floor is about 24 s, the longest test.

**App tests:** 169 tests in 4 binaries (`app` 95, `restore` 66, `compare` 7, `long_session` 1), 383 test-seconds, 191 s at `-j 2`. The
longest is `long_session` at 38 s (52 to 65 s under heavy load).

**Why they ran two at a time:** the `-j 2` in `check.sh` dates from the first window (`7376790`, 2026-11-07), "each is a real app, and this
Mac is in use". It was about load, not about the tests meeting each other: quiet runs are off every screen, each with its own state folder,
socket and pasteboard (`App.swift`: a pasteboard per process in a quiet run). Nothing in `.config/nextest.toml` grouped them.

## Phase 2: what changed

### 1. `tools/worktree.sh`: a warm start for every worktree (`458db19`, `501ee16`)

`tools/worktree.sh new <branch> <path> [<start>]` makes the worktree (a new branch without an upstream, so it can't push to `main` by
accident), then clones the main checkout's `target/` into it with `cp -c -p`: an APFS clone, no disk used until a side writes. It takes the
built dependencies of debug, release, dev-release and the Windows check, the build scripts' output and PDFium. It leaves out the workspace's
object files and incremental caches: 390,000 of main's 418,000 files, 247,000 of them `.o` files of old builds of the workspace's own crates,
all rebuilt in the worktree anyway. That makes the clone 4 to 19 s (14,807 files, the slower times with other builds writing to the disk)
instead of about a minute.

**Why it's correct.** The clone keeps its files' times, and runs after the checkout, so every workspace source is newer than every seeded
output: cargo rebuilds all 18 workspace crates and only the dependencies count as built. Checked with `cargo test --no-run -v`: 95
dependencies fresh, 18 workspace crates compiled. Without APFS it seeds nothing rather than copy 30 GB.

| First build in a new worktree | Cold | Seeded |
|---|---|---|
| clippy, both workspaces | 31 s | 9 s |
| Windows check | 15 s | 4 s |
| workers and headings helper | 27 s | 5 s |
| every test target | 31 to 40 s | 25 s |
| **total** | **about 105 s** | **about 45 s** |

`tools/worktree.sh remove <path>` removes the worktree and its target. `AGENTS.md` and `worker-testing.md` name it as the way to start one.

### 2. Slowest binaries first (`d439392`)

`.config/nextest.toml` gives priority 50 to the binaries holding the slowest tests: `installer`, `smoke`, `socket`, `engine` and
`long_session`. Only the order changes, each test is still a process of its own with the same limits. Back-to-back pairs on the whole
workspace, same load (22 to 27): before 88.7 s and 58.1 s, after 50.6 s and 46.9 s. Nextest checks that each named binary exists, so a
rename fails loudly at the start of every run rather than quietly dropping the priority.

### 3. App tests four at a time (`f22a4cc`)

`check.sh --full` runs the app stage with `-j 4`. Three runs: 118 s, 123 s and 140 s, 169 of 169 passing each time, at load 25 to 45 with
release builds running beside them, against 191 s at `-j 2` and load 5 to 10. The test-seconds rose from 383 to 465 to 552 with the load, so
on a quieter machine the gain is larger. `long_session` reached 62 to 65 s under that load, past nextest's 60 s "slow" mark and well inside its
5-minute limit; it now starts first. `tools/check-test.sh` passes.

### 4. A `dev-release` profile (`501ee16`)

`[profile.dev-release]` inherits `release` with `lto = "thin"` and `codegen-units = 16`, in both workspaces, and `app/build.sh
--dev-release` builds the app with it. `release` stays fat LTO with one codegen unit: every budget is measured on it and it's what ships.

| After an edit to `recto-session` | release | dev-release |
|---|---|---|
| The app's five crates (`cargo build`) | 74 s | 32 s |
| The whole `app/build.sh` | 86 s | 50 s |

dev-release with every dependency cold took 85 s (load 45). Release from a seeded worktree, all workspace crates rebuilt: 92 s. The 5
minutes reported for release builds didn't show up here with dependencies already built; a cold release build wasn't measured. The
dev-release app passed the app and comparison tests, 102 of 102.

### 5. Not changed

- **The linker:** ld-prime already, and linking is about 10 % of a test binary's build.
- **`[profile.dev]` and `[profile.test]`:** already set for speed (above).
- **sccache:** the owner's global setting; no hits for this repository (above).

## Phase 3: one test binary per crate, prepared, not applied

`docs/reports/build-performance-consolidate.sh <crate>...` moves `crates/<crate>/tests/*.rs` into `tests/it/` as modules of one binary,
with their helper folders, and fixes the paths that move breaks (`include_str!` out of `tests/`, `CARGO_MANIFEST_DIR` paths into a moved
folder, a kept binary's includes, and the tests that run their own binary as a stand-in worker with `--exact <name>`, whose names gain
the module's: without that last one, 77 tests failed, 76 of them `recto-render-service`'s). Memory, scale and handle-count tests stay binaries of their own (`KEEP`), as AGENTS.md requires. Tried on
the scratch branch `scratch/test-consolidate` (local, not pushed): it compiles with no warnings and all 1,717 workspace tests pass. Not
run there: clippy, and the app tests, which need the app stage.

Measured there, back to back:

| | Separate binaries | One per crate |
|---|---|---|
| `recto-cli` alone: binaries | 22 | 3 (`it`, `memory`, the unit tests) |
| `recto-cli`: rebuild after an edit to `recto-session` | 9.0 s, 32 units | 3.7 s, 7 units |
| Workspace: rebuild after that edit | 6.2 s, 50 units | 4.9 s, 31 units (only `recto-cli` consolidated) |
| `recto-cli`'s tests, running (load 16 to 20) | 20.2 s, 18.4 s | 16.8 s, 17.2 s |
| Six crates consolidated (`recto-cli`, `recto-render-service`, `recto-core`, `recto-reader`, `recto-platform`, `recto-control`): binaries | 72 | 39 |
| Workspace: rebuild after an edit to `recto-session` | 11 s, 11 s, 50 units | 11 s, 9 s, 30 units |
| Workspace tests, running (pairs at load 27 to 29, then 40 to 42) | 67.9 s, 115.6 s | 61.9 s, 75.0 s |

**What it saves:** fewer, larger links and less of nextest's per-binary work: `recto-cli`'s edit-to-tests loop goes from 9 s to 4 s, and a
whole-workspace run gains about 10 % (more under heavy load, in one sample). Seconds per edit and per run, not minutes. Worth doing at a quiet
point for the edit-to-test loop; the full check's length is the app tests and the slow tests, not this.

**Steps when it's applied:**
1. On a branch from `main` with nothing open that adds test files: `docs/reports/build-performance-consolidate.sh recto-cli
   recto-render-service recto-core recto-reader recto-platform recto-control` (crates with one movable file gain nothing).
2. `tools/check.sh`: the app stage's `--test app --test restore --test compare --test long_session` becomes `-E 'package(recto-cli) &
   binary(it) & test(/^(app|restore|compare|long_session)::/)'`, and the corpus run's `--test index real_pages` becomes `--test it
   index::real_pages`. `tools/check-test.sh` after.
3. `.config/nextest.toml`: the priority filter names binaries that no longer exist, and nextest refuses to start until it's changed to
   `binary(installer) | binary(smoke) | (package(recto-reader) & binary(it) & test(/^engine::/)) | (package(recto-cli) & binary(it) &
   test(/^(socket|long_session)::/))`.
4. Any doc or script naming `--test <file>` (`grep -rn -- '--test ' docs tools app .github`), then `cargo fmt --all` and `--quick`.

## Not measured

- Anything alone: every number has other agents' work beside it, and single samples.
- A release build from an empty target, and the full `check.sh --full` end to end before and after (its stages were measured one by one; the
  hand-back run is the one `--full` on this branch).
- The CI runners: none of this was tried there. CI doesn't use `check.sh`'s `-j` for the app tests.
- sccache with base directories, or off: only that it gets no hits as set.
- Raising dependencies' `opt-level` in dev to speed CPU-heavy tests (fuzz smoke, the corpus tests): it could change timing-sensitive tests,
  so it wasn't tried.
- Capping nextest's threads per queue slot, as the queue does for `CARGO_BUILD_JOBS`: two full checks at once run 20 test processes on 10
  cores. That's a change to the shared queue, so it's a suggestion only.
- The main checkout's `target/` is 33 GB, 418,000 files, mostly stale object files of old builds. Not touched: it's shared.
