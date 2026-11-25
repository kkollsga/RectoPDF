# Startup stalls: the menu phase and the first sidebar/text layout, split (2026-11-25)

Measurement only. No product behaviour changed. Build: `build/startup-stalls` commit A (94718f3d), release (`app/build.sh --release`, fat LTO), with the layout probe in a temporary commit B that was dropped (`git diff` against A is empty).

## What was measured, and under what

- Four marks added to the startup trace after `host_ready` (ids 15 to 18): `menu_model_ready`, `services_menu_ready`, `menu_built`, `window_model_ready`. CAP 16 to 32, and the last slot is reserved for `host_ready`. Whole-type growth is 16 points x 24 B = 384 B (Kept 400 to 784, Startup 432 to 816, Short 1224 to 1608, Shared 1520 to 1904).
- Why 32: the brief's 22-point worst case was wrong. **A restored document builds the menu three times** (launch, just after `restore_queued`, and at the window's first model change), so a launch keeps 25 points: with CAP 24 ten of eleven R launches lost `host_ready`. With 3 restored documents the count stays 25 and 3 menu builds (two launches), so builds do not grow with tabs.
- Scenario E: empty window. Scenario R: one restored document (a copy of `standards_ietf_rfc9110.pdf` in a temp folder: a quiet run refuses files outside the temp folders; `RECTO_BENCH100` is not set on this machine, the file came from `distillPDF/benchmarking/bench100`). Each batch: 1 discarded cold launch + 10 runs, `recto qa start`, `wait-settled`, `qa state --json`, `qa quit now`, wait for "isn't running", temp state removed.
- **Load: every number here was measured under load.** Machine load average (1 min) 5.6 to 6.9 for the first control attempt, 12 to 13 at the control re-runs, 8.5 to 9.5 at the probe runs, 6.5 to 8 at the input-to-frame runs; other workers share it. Control and probe batches ran at different loads, one after the other (the probe build replaced the control build), so a control-vs-probe difference of a few ms is not a finding. Tables take the 10 runs after the discarded one; medians of 10, one machine.
- Raw JSON: `/Volumes/EksternalHome/Koding/Rust/rectopdf-evidence/startup-stalls-2026-11-25/` (`control-E-*`, `control-R-*`, `probe-E-*`, `probe-R-*`, `control-R3-*` for 3 documents; `-00` is the discarded cold launch; load lines in `*-load.txt`).
- Intervals are consecutive points by first occurrence; the repeated menu marks of later builds are not part of any interval (their time sits inside the interval they fall in: `restore_queued` to `restore_begin` and `window_opened` to `window_model_ready`). Overlapping intervals are never added. CPU is the UI thread's own CPU.

## Findings

1. **The menu phase (launch_begin to menu_ready) is about 14.8 ms wall in E and in R, and it is CPU, not waiting**, in these runs: `menu_model_ready` to `services_menu_ready` 3.3 to 3.6 ms (the `core.menuBar()` call plus building the menu up to the Services item), `menu_built` to `menu_ready` 10.5 to 10.8 ms of which 8.5 to 9.2 ms is CPU (this is `NSApp.mainMenu = bar`, plus the services registration, `quitOnTerminate` and `watchMemoryPressure` run after `installMenu` returns; `NSApp.mainMenu =` is most of it). The 114 to 128 ms phase of the 2026-11-18 attribution did not recur in the 20 runs; the only slow menu phase seen was the first launch after a build (E control cold launch: `menu_ready` 48.7 ms after `launch_begin`, against 13.1 to 13.4 ms for the other cold launches in the probe batches).
   - Crossing the `mainMenu =` line cannot be asserted in a test without pinning a timing, which we do not do. The opening test asserts the order of the marks; a mark moved across that one line is not caught (seen: moving `.menuBuilt` after `NSApp.mainMenu =` leaves the test passing). A mark moved against `services_menu_ready` is caught (seen failing).
2. **Window construction**, release, in the existing phases: `window_body` to `native_window_ready` is the largest, 15.7 to 20.7 ms wall (11.5 to 12.9 ms CPU, so up to a third is waiting), then `page_ready` to `sidebar_ready` 6.1 to 7.2 ms and `sidebar_ready` to `window_layout_ready` 4.2 to 4.8 ms, `window_begin` to `window_body` 6.2 to 6.8 ms.
3. **`window_opened` to `window_model_ready`** (the first `modelChanged()`, which refreshes the model, the menu a third time in R and a second time in E, the settings and the update timer) is 1.8 to 2.1 ms median, 10.0 ms worst in E (control).
4. **The first sidebar cells come after `host_ready`** (see probe table): the first cell is made 11 ms (tab) and 19 ms (contents) after `host_ready`, and the first cell layout 12.5 ms after it. The one costly call is the first `NSAttributedString.size()` of the tab name at 14 pt Medium (`.AppleSystemUIFontMedium`), the 9th measuring call in every R run: **5.2 to 6.6 ms, in 10 of 10 R runs**, 5.4 ms median, which is nearly the whole of `sidebar_cell_layout` (median max 5.6 ms, up to 7.0 ms). Every other `text_size` call is under 0.4 ms (E's largest is 0.36 ms). This is a first use of a font in the process (not a repeated per-row cost): calls 10 to 18 are cheap.
5. **The 18.6 ms cell layout of `short-stalls-2026-11-18.md` did not recur in 10 R runs** (cell layout max 5.3 to 7.0 ms). Reported, not hunted (call-back item 4 of the brief).
6. **Nothing over the 16 ms budget after `host_ready`** in any of the 44 launches (`hangs.over_budget` after host_ready: none in E or R, control or probe). `over_budget` entries that exist are before `host_ready` (a 35 to 55 ms busy `scroll`-kind stretch at about 40 to 70 ms in the window build) and are in the JSON.
7. **Probe effect:** launch_begin to host_ready, control vs probe medians: E 58.0 vs 59.7 ms (+2.8 %), R 62.9 vs 55.0 ms (-12.6 %); both inside the brief's 20 %. The R difference is the machine's load changing between batches, not the probe making the app faster.

## Tables

### control E: 10 runs (cold first launch excluded)

| from | to | median wall ms | max wall ms | median CPU ms |
|---|---|---|---|---|
| launch_begin | menu_model_ready | 0.2 | 0.3 | 0.2 |
| menu_model_ready | services_menu_ready | 3.5 | 7.2 | 3.4 |
| services_menu_ready | menu_built | 0.2 | 0.3 | 0.2 |
| menu_built | menu_ready | 10.8 | 20.5 | 8.5 |
| menu_ready | services_ready | 0.1 | 0.1 | 0.1 |
| services_ready | restore_queued | 0.1 | 0.2 | 0.1 |
| restore_queued | restore_begin | 2.1 | 3.1 | 1.5 |
| restore_begin | appearance_ready | 0.0 | 0.0 | 0.0 |
| appearance_ready | window_begin | 0.0 | 0.0 | 0.0 |
| window_begin | window_body | 6.8 | 19.9 | 6.3 |
| window_body | native_window_ready | 20.7 | 39.5 | 12.3 |
| native_window_ready | page_ready | 0.9 | 2.3 | 0.9 |
| page_ready | sidebar_ready | 7.0 | 15.3 | 6.9 |
| sidebar_ready | window_layout_ready | 4.8 | 10.9 | 4.8 |
| window_layout_ready | window_constructed | 1.3 | 3.5 | 0.8 |
| window_constructed | window_opened | 0.0 | 0.0 | 0.0 |
| window_opened | window_model_ready | 1.9 | 10.0 | 1.7 |
| window_model_ready | host_ready | 0.0 | 0.0 | 0.0 |
| launch_begin | host_ready | 58.0 | 116.5 | 47.8 |

points per run [22], lost [0]

### control E: over_budget stretches after host_ready (all 11 launches)

| run | at_ms | wall ms | CPU ms | kind | host_ready at_ms |
|---|---|---|---|---|---|
| none | | | | | |
- control E cold first launch: launch_begin to host_ready 287.5 ms wall, 59.2 ms CPU; menu_ready at 48.7 ms after launch_begin

### probe E: 10 runs (cold first launch excluded)

| from | to | median wall ms | max wall ms | median CPU ms |
|---|---|---|---|---|
| launch_begin | menu_model_ready | 0.2 | 0.3 | 0.2 |
| menu_model_ready | services_menu_ready | 3.6 | 5.9 | 3.5 |
| services_menu_ready | menu_built | 0.2 | 0.2 | 0.2 |
| menu_built | menu_ready | 10.5 | 13.0 | 9.2 |
| menu_ready | services_ready | 0.1 | 0.5 | 0.1 |
| services_ready | restore_queued | 0.1 | 0.1 | 0.1 |
| restore_queued | restore_begin | 2.9 | 7.9 | 1.4 |
| restore_begin | appearance_ready | 0.0 | 0.0 | 0.0 |
| appearance_ready | window_begin | 0.0 | 0.0 | 0.0 |
| window_begin | window_body | 6.2 | 11.9 | 5.8 |
| window_body | native_window_ready | 15.7 | 28.4 | 11.8 |
| native_window_ready | page_ready | 0.8 | 2.6 | 0.8 |
| page_ready | sidebar_ready | 6.3 | 11.3 | 6.3 |
| sidebar_ready | window_layout_ready | 4.2 | 8.2 | 4.2 |
| window_layout_ready | window_constructed | 1.7 | 4.9 | 0.9 |
| window_constructed | window_opened | 0.0 | 0.0 | 0.0 |
| window_opened | window_model_ready | 1.8 | 3.4 | 1.6 |
| window_model_ready | host_ready | 0.0 | 0.0 | 0.0 |
| launch_begin | host_ready | 59.7 | 87.5 | 46.1 |

points per run [22], lost [0]

### probe E: over_budget stretches after host_ready (all 11 launches)

| run | at_ms | wall ms | CPU ms | kind | host_ready at_ms |
|---|---|---|---|---|---|
| none | | | | | |
- probe E cold first launch: launch_begin to host_ready 49.3 ms wall, 44.0 ms CPU; menu_ready at 13.1 ms after launch_begin

### probe E: layout probe keys (10 runs)

| key | median first us | max first us | median max us | max max us | median count | median total us | first at, ms after host_ready (median; negative = before) |
|---|---|---|---|---|---|---|---|
| contents_cell_make | 0 | 0 | 0 | 0 | 0 | 0 | never |
| sidebar_cell_layout | 0 | 0 | 0 | 0 | 0 | 0 | never |
| sidebar_update | 225 | 870 | 225 | 870 | 2 | 359 | -1.8 |
| tab_cell_make | 0 | 0 | 0 | 0 | 0 | 0 | never |
| text_size | 40 | 357 | 77 | 357 | 6 | 163 | -6.8 |
| window_layout | 1917 | 3243 | 1917 | 3243 | 4 | 2185 | -6.8 |

top 8 text_size calls over the 10 E probe runs

| us | font | pt | call index | run | at ms after launch_begin |
|---|---|---|---|---|---|
| 363 | ? | 0 | 1 | 3 | 79.0 |
| 171 | .AppleSystemUIFontBold | 28 | 5 | 8 | 83.1 |
| 103 | .AppleSystemUIFontBold | 28 | 5 | 2 | 57.6 |
| 103 | .AppleSystemUIFont | 14 | 6 | 8 | 83.3 |
| 100 | .AppleSystemUIFontBold | 28 | 5 | 5 | 57.9 |
| 91 | .AppleSystemUIFontBold | 28 | 5 | 3 | 85.2 |
| 78 | .AppleSystemUIFontBold | 28 | 5 | 6 | 63.0 |
| 76 | .AppleSystemUIFont | 14 | 6 | 10 | 47.6 |

sidebar_cell_layout max per run (us): [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]

### E: control vs probe, launch_begin to host_ready wall ms: control median 58.0 (min 50.8, max 116.5); probe median 59.7 (min 49.0, max 87.5); probe is +2.8 %

### control R: 10 runs (cold first launch excluded)

| from | to | median wall ms | max wall ms | median CPU ms |
|---|---|---|---|---|
| launch_begin | menu_model_ready | 0.2 | 0.7 | 0.2 |
| menu_model_ready | services_menu_ready | 3.3 | 4.5 | 3.3 |
| services_menu_ready | menu_built | 0.2 | 0.2 | 0.2 |
| menu_built | menu_ready | 10.5 | 12.6 | 8.8 |
| menu_ready | services_ready | 0.1 | 0.1 | 0.1 |
| services_ready | restore_queued | 0.1 | 0.1 | 0.1 |
| restore_queued | restore_begin | 2.9 | 6.3 | 2.1 |
| restore_begin | appearance_ready | 0.0 | 0.0 | 0.0 |
| appearance_ready | window_begin | 0.0 | 0.0 | 0.0 |
| window_begin | window_body | 6.3 | 9.6 | 5.9 |
| window_body | native_window_ready | 19.2 | 23.3 | 12.9 |
| native_window_ready | page_ready | 0.8 | 1.0 | 0.8 |
| page_ready | sidebar_ready | 7.2 | 8.6 | 7.0 |
| sidebar_ready | window_layout_ready | 4.8 | 7.4 | 4.7 |
| window_layout_ready | window_constructed | 1.4 | 4.4 | 0.8 |
| window_constructed | window_opened | 0.0 | 0.0 | 0.0 |
| window_opened | window_model_ready | 2.1 | 8.5 | 1.9 |
| window_model_ready | host_ready | 0.0 | 0.0 | 0.0 |
| launch_begin | host_ready | 62.9 | 77.1 | 48.5 |

points per run [25], lost [0]

### control R: over_budget stretches after host_ready (all 11 launches)

| run | at_ms | wall ms | CPU ms | kind | host_ready at_ms |
|---|---|---|---|---|---|
| none | | | | | |
- control R cold first launch: launch_begin to host_ready 74.0 ms wall, 55.1 ms CPU; menu_ready at 13.2 ms after launch_begin

### probe R: 10 runs (cold first launch excluded)

| from | to | median wall ms | max wall ms | median CPU ms |
|---|---|---|---|---|
| launch_begin | menu_model_ready | 0.2 | 0.3 | 0.2 |
| menu_model_ready | services_menu_ready | 3.5 | 4.0 | 3.5 |
| services_menu_ready | menu_built | 0.2 | 0.2 | 0.2 |
| menu_built | menu_ready | 9.8 | 12.0 | 8.9 |
| menu_ready | services_ready | 0.1 | 0.6 | 0.1 |
| services_ready | restore_queued | 0.1 | 0.1 | 0.1 |
| restore_queued | restore_begin | 2.6 | 3.2 | 1.9 |
| restore_begin | appearance_ready | 0.0 | 0.0 | 0.0 |
| appearance_ready | window_begin | 0.0 | 0.0 | 0.0 |
| window_begin | window_body | 6.4 | 8.1 | 6.0 |
| window_body | native_window_ready | 14.7 | 20.4 | 11.5 |
| native_window_ready | page_ready | 0.8 | 0.8 | 0.8 |
| page_ready | sidebar_ready | 6.1 | 7.8 | 6.1 |
| sidebar_ready | window_layout_ready | 4.5 | 5.7 | 4.5 |
| window_layout_ready | window_constructed | 1.1 | 5.0 | 0.8 |
| window_constructed | window_opened | 0.0 | 0.1 | 0.0 |
| window_opened | window_model_ready | 1.9 | 3.9 | 1.7 |
| window_model_ready | host_ready | 0.0 | 0.0 | 0.0 |
| launch_begin | host_ready | 55.0 | 67.1 | 46.3 |

points per run [25], lost [0]

### probe R: over_budget stretches after host_ready (all 11 launches)

| run | at_ms | wall ms | CPU ms | kind | host_ready at_ms |
|---|---|---|---|---|---|
| none | | | | | |
- probe R cold first launch: launch_begin to host_ready 53.4 ms wall, 45.8 ms CPU; menu_ready at 13.4 ms after launch_begin

### probe R: layout probe keys (10 runs)

| key | median first us | max first us | median max us | max max us | median count | median total us | first at, ms after host_ready (median; negative = before) |
|---|---|---|---|---|---|---|---|
| contents_cell_make | 194 | 375 | 206 | 375 | 2 | 293 | 18.6 |
| sidebar_cell_layout | 5644 | 7030 | 5644 | 7030 | 4 | 5834 | 12.5 |
| sidebar_update | 385 | 858 | 385 | 858 | 2 | 518 | -1.7 |
| tab_cell_make | 632 | 1070 | 632 | 1070 | 1 | 632 | 11.0 |
| text_size | 33 | 46 | 5436 | 6585 | 18 | 5974 | -5.6 |
| window_layout | 2137 | 3006 | 2137 | 3006 | 4 | 2837 | -5.7 |

top 8 text_size calls over the 10 R probe runs

| us | font | pt | call index | run | at ms after launch_begin |
|---|---|---|---|---|---|
| 6585 | .AppleSystemUIFontMedium | 14 | 9 | 2 | 78.3 |
| 6426 | .AppleSystemUIFontMedium | 14 | 9 | 7 | 87.1 |
| 6345 | .AppleSystemUIFontMedium | 14 | 9 | 3 | 68.2 |
| 5655 | .AppleSystemUIFontMedium | 14 | 9 | 4 | 61.6 |
| 5541 | .AppleSystemUIFontMedium | 14 | 9 | 8 | 74.9 |
| 5332 | .AppleSystemUIFontMedium | 14 | 9 | 6 | 58.8 |
| 5253 | .AppleSystemUIFontMedium | 14 | 9 | 10 | 57.5 |
| 5162 | .AppleSystemUIFontMedium | 14 | 9 | 1 | 68.7 |

sidebar_cell_layout max per run (us): [5471, 7030, 6597, 5866, 5298, 5548, 6704, 5741, 5271, 5505]

### R: control vs probe, launch_begin to host_ready wall ms: control median 62.9 (min 51.6, max 77.1); probe median 55.0 (min 47.3, max 67.1); probe is -12.6 %

## What this does not show

- Cold first launches are reported separately above in each section; the first launch after a build (E control: 287 ms launch_begin to host_ready) is a different thing from the 10 that follow (about 58 ms) and is not in the medians.
- One machine, one app copy, under load; medians of 10. No claim is made about a person's Mac at rest.
- The probe's `text_size` times include the `CACurrentMediaTime` calls (about 40 ns each) and the font name lookup only when a call is among the 8 longest.
- The `doing`/`verb` labels of the watchdog are not attribution and are not used as such.

## Input to frame (step 0 and step 8)

`input_to_frame` (release, headless, at 1, 100 and 1,000 tabs), before on main (`f86bc16c`) and after on commit A, in the same hour and under load 5 to 8 (the machine was never quiet). The p50 of every case is within the bound after <= 1.1 x before + 200 us in both after runs. **The tail quantiles are not**: after run 1 has 19 of 216 quantiles over the bound, run 2 has 11, never the same cases (run 1: pane focus, open new file, find typed at 1,000 tabs; run 2: open its tab, find step, scroll crossing an entry, open new file at 100 and 1,000 tabs), all p95 and p99 (`open new file` at 1,000 tabs p99 426.5 before, 1,050 and 1,292 after). The headless harness does not call the startup trace (the checkpoint needs the window's thread binding), and no code on the hot path changed, so this reads as load noise on tails, but it is a miss of the stated bound and is reported as one, not as a pass (call-back item 7). Files: `startup-stalls-itf-before.txt`, `startup-stalls-itf-after.txt`, `startup-stalls-itf-after2.txt` in the scratchpad.

### Interleaved before/after, release harness binaries side by side (one hour, load 7.2 to 8.5)

Step 1: the harness cannot reach the changed code. It builds one `Watchdog` per app (`core/start.rs:79`), which on commit A is 384 B bigger, and nothing calls `startup::checkpoint` (only the host's `hostStartupPhase` FFI does); the window's thread binding is never set in it. Both binaries (main `f86bc16c` and commit A) were built once, kept side by side and run in the order main, after, main, after, then after, main, main, after (8 runs, 5 s each), each pair compared on 216 quantiles (54 cases x ui p99, p50, p95, p99) against after <= 1.1 x before + 200 us:

| pair (before, after) | quantiles over the bound | same-binary spread in the same hour |
|---|---|---|
| 1 | 10 of 216 | main 1 vs main 2: 14 of 216 over the same bound |
| 2 | 34 of 216 | after 1 vs after 2: 46 of 216 |
| 3 (after first) | 0 of 216 | |
| 4 | 0 of 216 | main 3 vs main 4: 0; after 3 vs after 4: 0 |

No p50 is over the bound except two in pair 2 (`open new file` and `open its tab` at 1 and 100 tabs, where the same pair's tails are also over). Misses are in the first two pairs only, the after binary varies against itself by more than any of its misses (after 1 vs after 2, 46 quantiles), and the two later pairs, including one with the order reversed, have none; so this is not a change that moves with the code. It is reported as what it is: **2 of 4 pairs missed the bound, 2 met it**, and the median of the four runs misses on 5 of 216 quantiles (`open new file` 1 tab p95 309 to 856 us and p99 329 to 1,250 us; `tap row` 1,000 tabs p95 200 to 451 and p99 208 to 541; `compare click` 1,000 tabs p99 393 to 695), all tails of cases where the after runs 1 and 2 had the multi-ms outliers. Not relabelled as a pass: the harness measured on a machine other workers were using, and the verdict that this is noise rests on the same-binary spread and on pairs 3 and 4, not on a quiet machine.

## Seed proofs

- `only_the_first_real_begin_can_start_a_trace` with `decode` on `<=`: fails (startup.rs:59).
- `the_ready_point_is_kept_when_the_trace_overflows` with the reservation removed: fails.
- Opening test with `.menuBuilt` moved up to just after `.menuModelReady`: fails ("the menu's marks are out of order"). With it moved after `NSApp.mainMenu =` (the brief's seed) it still passes, see finding 1.
