# Slow mode

> Part of the engineering notes: [`README.md`](README.md).

On 2026-11-11 `main` went red on CI three pushes running, each time on a path only CI's slower machine took (guide, "Code that's right only when it's fast"). `recto_platform::slow` makes every waiting point late on purpose, so one local run takes all those paths.

**The seams.** Each is one line where it waits, on a background thread, or a call to the host that arrives late on the slow mode's own thread:

| Seam | Where | How it's late | `ci` |
|---|---|---|---:|
| `state` | `begin_restore`, before the record's read | the restore thread waits | 1200 ms (with `launch`'s 300 before it, 1.5 s in all) |
| `remake` | the window that went up without the state, made again from it | `restore_ready` arrives late | 300 ms |
| `worker` | `DocRenderer::run_job_with`, a job's answer | the caller gets it late | 60 ms |
| `headings` | the generator's thread, after the helper's done | the session hears late | 1500 ms |
| `index` | the indexer's thread, after a build | the session hears late | 800 ms |
| `socket` | the control thread, bound but not yet answering | the first answer's late | 700 ms |
| `draw` | `EngineHooks::ops_ready`, layers to draw | `ops_ready` arrives late | 40 ms |
| `sleep_read`, `sleep_write` | `RecordStore::read_sleep` and `write_sleep` | the reading or writing thread waits | 150, 300 ms |
| `session` | the session thread, after the state's taken in and the host told | what it does next is late, as on a starved machine | 300 ms |
| `launch` | `Updater::launched`, on the restore thread, before the state's read | what this copy is (its bundle, its version) is known late | 300 ms |

`launch` and `state` wait on the same thread, one after the other, so they add up: in `ci`, 300 ms and 1.2 s are the state at 1.5 s, as before `launch` was there.

**The switch.** `RECTO_QA_SLOW` is a comma list: a number of milliseconds for every seam, `ci` for the profile above, `<seam>=<ms>` for one, and `jitter=<seed>` for delays drawn from half to one and a half times each, repeatably. `RECTO_QA_RESTORE_DELAY_MS` and `RECTO_QA_REMAKE_DELAY_MS` still work, and win for their seam. A switch that can't be read refuses the launch, so a slow run can't pass as a fast one. Each seam is capped (30 s for the state, 10 s for the rest).

**The seed, and running a failure again** (PLAN R0.7b). The slow stage runs `ci` exactly, so the gate fails the same way each run; `CHECK_SLOW_SEED=<n>` adds `jitter=<n>` and the stage says which it used. `Profile::switch` writes a profile back as the switch that gives it again, seed included (`a_profile_written_back_as_a_switch_gives_the_same_delays`), and a failing app test's evidence rebuilds it from `qa state`'s report, so its rerun line carries the seams and the seed as that run met them: `RECTO_QA_SLOW='state=1200,…,jitter=7' tools/check.sh --like-ci 'binary_id(=recto-cli::app) & test(=…)' --profile default --jobs 4`. Same seed, same delays at each seam's n-th wait. What it can't replay is the order the threads were scheduled in: the delays are the same, the machine isn't.

**A hold.** `RECTO_QA_SLOW_HOLD=<seam>:<path>` holds one seam until the file at the path is there, rather than for a time, for a test that has to act while the app waits at that seam: on a slow machine any fixed wait can be over before the test acts (CI, 2026-11-12: a 10 s wait at `launch` ran out first, run 43225124901). Its bound, 120 s, is for a test that never lets go. A bad value refuses the launch, as the switch's does, and an update's relaunch carries it over.

**Where it's never on.** A release build (`recto_platform::RELEASE_BUILD`, compiled in from `RECTO_RELEASE`, the one constant the updater and the renderer's host go by too), any launch that isn't a quiet run, and `recto` itself: the app arms it once, at its start, from its own arguments. Unarmed, each seam is one load of a `OnceLock`. A `pause` asked on the UI thread is refused and counted as `slow.ui_refusals` in `qa state`, which also has each seam's delay, how often it was waited at and how long in all. An update's relaunch of a quiet run carries the switches over. The thread that makes late calls exists only while one's waiting, and goes half a second after the last.

**The stage.** `tools/check.sh --full`'s `slow` stage, in place of the `late-state` stage it grew from, runs here and not on CI, which is the slow machine already: there the profile doubled up, and two tests passed their bounds for slowness alone, as they did here with the profile doubled (run 43080381613; the first frame shows on its patience without tiles, and Find's settle took about 50 s). It runs 25 app and restore tests with `RECTO_QA_SLOW=ci` (`SLOW_TESTS` in `check.sh`): whole-state comparisons across a quit and updates, what comes back, the launch and its first frame, a quit that has to write, sleep and wake, what acts on the window as soon as it's up, Find, copy, the index and the headings helper. `slow::every_test_the_slow_stage_names_is_there` holds the list to tests that exist, as a filter that matches nothing would drop one silently. The other `slow::` tests in `recto-cli/tests/app/slow.rs` check that a run is slow at each seam it passes, that a run without the switch never is, and that a bad switch refuses the launch. The build queue counts `RECTO_*`, `NEXTEST_*` and `RUSTFLAGS` as inputs (`.heavy-inputs`, `env` lines), so the same tests with another profile aren't refused as a failure already seen.

**What its first run found** (2026-11-11, the app and restore binaries once with `ci`, 21 of 194 failed): four product bugs, each with a test that names it, and one in the build queue. A place taken again from the host's pixel-rounded scroll (the top of a page saved as the foot of the one above: `a_view_the_host_rounded_to_its_pixels_is_still_on_the_place`). A quit that waited its 5 s behind queued sleep files and went without the state (`tabs_just_opened_come_back_when_sleep_files_are_slow`). A password sheet on the window a late state remade, whose Cancel was lost and whose count stayed open (`a_protected_tab_asks_again_when_the_state_is_read_late`). The session's news heard only once the socket's server ran (`find_comes_back_when_the_socket_answers_late`). And the rest were tests that assumed speed or were the slow mode's own report. The `session` seam, added after, reproduced a CI-only failure (Find's field without the keyboard after an update): the session thread told the host the state was in before bringing Find up to date (`finds_field_has_the_keyboard_again_when_the_session_is_late_after_the_state`).
