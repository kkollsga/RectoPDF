# Performance review, 2026-11-16

Every line of `SPEC.md` §2 measured on the release build (`app/build.sh --release`) of `main` at 50bd47d5 (its code is 4b4e027c's), beside the last report, `budgets-2026-11-08.md`. The owner asked: "Can you run a full performance review?"

**In short.** Memory is where it was or better: an idle tab still costs about 1 KiB, a closed window 12.8 MiB, a text page 36 MiB. The two memory misses are the old ones, and the big map at 2x and 3x zoom has got a little worse (456 to 515 MiB against the 450 accepted). Speed is mostly where it was, but **three numbers in `qa budget` now read slow for a reason that isn't the reader's**: since the page number and zoom marks began to fade (2026-11-08, after the last report), the tool waits about 190 ms for that fade before it calls a page drawn. Timed apart from the fade, a cold start draws its page in 274 ms (p50) and the sidebar opens in about 25 ms. The real misses are the old ones: a 10,000-page file's first open, a woken tab's first tile, and the window's thread stalling 31 to 69 ms. One thing is new: after an update, an edited 2 GiB file keeps a thread reading the whole file for its signatures for 3 to 5 s.

**How, and under what load.** This Mac (M4, 16 GB). Quiet runs, the window off every screen, so frames on a display, a visible window's memory and 120 Hz aren't in any of this. Every run went through the build queue beside two other workers' checks, at load average 2.1 to 15 (each run's load and the queue's state are in `performance-2026-11-16/load.txt`). Nothing was measured alone. Whole process tree unless it says. Every number below comes from a run of `docs/reports/performance-2026-11-16.sh` (its part named in brackets), its output kept in `docs/reports/performance-2026-11-16/`. One run each unless it says.

The runs, as the old report's where they still exist:
- **A** `[a]`: `recto qa budget --file rfc9110 --heavy NASA ntrs20240002628 --runs 11`, load 2.3 to 3.5.
- **B** `[b]`: `recto qa budget --file <made 2 GiB, 10,000 pages> --heavy USGS 70277338 (the 346 MiB map) --runs 11`, load 7.2 to 11.1. The old report didn't keep the made file's generator; `performance-2026-11-16-bigpdf.py` makes one of the same shape (classic xref, each page a line of text and a 210 KiB stream beside it): 2,154,110,753 bytes against its 2,155,039,645. B ran whole this time (the old 21-run B2 ran out of time and isn't repeated; `[b-again]` is 5 more starts and hand-offs).
- **Tabs again** `[tabs]`: two more `qa budget --file rfc9110 --heavy <map> --runs 1`, for 1,000 tabs of the map and the map's zoom.
- **Probes** `[probe-*]`, new: `performance-2026-11-16-probe.py` starts the app as `qa budget` does and polls `qa state`, so the page drawn and the window's animations are timed apart. Each poll is a `recto` process, 10 to 30 ms apart, so a probe's time is an upper bound to within one poll.
- The old report's extras, again: each colour page `[colour]`, 5 wakes `[wake]`, six sweeps `[sweeps]`, the idle and closed window `[closed]`, the cold open `[coldopen]`.

## The lines

| SPEC §2 | Target | 2026-11-08 | Now | | Moved |
|---|---|---|---|---|---|
| One more idle tab | ≤ 4 KiB, 0 handles, 0 threads, 0 layers, 0 tiles | 0.40 KiB (NASA), 0.64 to 0.90 (map), 4.05 once | 1,000 tabs over 2, app's own: NASA **0.0 KiB** a tab (25.86 MiB at 2 and at 1,000); the map **1.1, 1.3, 0.9 KiB** (B, tabs 1, tabs 2). Handles 25 at 2 tabs, 22 to 25 at 1,000. Threads 19 at 2, 12 to 18 at 1,000. Tile layers 12 and 12 (NASA), 1 and 1 (map) | **Met** | No |
| 1,000 tabs | ≤ 4 MiB over 2 tabs; sidebar opens < 150 ms | 0.39 to 0.88 MiB; sidebar 11.2, 11.6 ms | **0.0** (NASA), **0.89 to 1.26 MiB** (map). Sidebar, as `qa budget` times it: 311, 275, 297, 301 ms, but that is the marks' fade (below). Timed apart `[probe-sidebar]`, 1,000 tabs, 5 times: the command answered in **6 to 25 ms**, the page view drawn by **26 to 66 ms**, the fade over at 310 to 329. Restored across an update: 1,000 tabs at 26.1 MiB of app (NASA), 22.6 (map) | **Met** | No (the tool's number moved, not the sidebar) |
| Live documents | Active plus at most 2 recent | 2, 3, 3 | `documents_held` 2 at 2 tabs, 3 at 100 and 1,000, in every run | **Met** | No |
| One document, text page | ≤ 60 MiB | `rfc9110` 37.1 (app 26.5, worker 10.6); made file 28.4 | `rfc9110` **36.0** (app 25.8, worker 10.2); made file **29.3** and 27.7 | **Met** | No |
| Colour full-bleed page | ≤ 100 MiB | NASA p50 42.4; NASA `ntrs20220005969` p175 217, 45 after 4 s | NASA p50 **43.7**; shaded p175 **214.2** settled (worker 185.9), 45.0 after 4 s | **Missed** on shaded backgrounds, as before | No |
| Fast scrolling, peak | ≤ 170 MiB; the map 415 to 450 accepted | `rfc9110` 44 / 48; made 35 / 36; NASA 52. Map fitted 386, 2x 441, 3x 500 (worker 466) | `rfc9110` **43.7 / 47.4** at 6 / 40 pages a second; made **35.0 / 33.6**; NASA after scroll and zoom **52.4**. The map over three runs: fitted **386, 386, 386**; about 2x **457, 459, 456**; about 3x **455, 515, 511**, worker peak 421, 481, 478; 33 to 36 at rest 4 s later | **Met**, but the map is **over the exception at both 2x (by 6 to 9 MiB) and 3x (by 5 to 65)** | **Worse at 2x** (441 then); 3x the same spread |
| Cold start to first sharp page | ≤ 300 ms p50, ≤ 500 ms p95 | 237 / 353 | `qa budget` says **445 / 669**, which includes the marks' fade. Timed apart `[probe-cold]`, 11 starts: page drawn **274 ms p50, 317 worst**; first frame 71 to 87 ms from the process starting; the fade ends at 438 to 494 | **Met**, by the probe | 237 then and 274 now, by different methods (then `qa budget` with no fade to wait for; now a poll, an upper bound): perhaps +37 ms |
| Open 2 GB, 10k pages: page 1 sharp | ≤ 250 ms warm, ≤ 600 ms cold | Warm 273 p50; cold 1.07 to 1.09 s | Warm `[probe-made]`, 11 starts: **337 ms p50**, 630 the first; `qa budget` 536 p50 (fade in it). Cold `[coldopen]`, three fresh clones: **2.01, 2.04, 2.03 s** for `recto local F facts`, 0.08 s once cached; **744 of 776 samples in `lopdf`'s `IndexedReader::page_map`**, in `pread` | **Missed**: warm by 35%, cold by **3.4x** | Cold 2x the old time on a regenerated file; whether the file, the disk or the load, not known |
| Main never stalls > 16 ms | ≤ 16 ms | 20 to 27 ms, 119 once | Worst over each run: **40** (A), **69** (B), 31 (`b-again`), 35 and 59 (tabs), 33 (closed). No stall over 1 s | **Missed**, by more than last time | Worse, at higher load in B |
| Idle CPU | ~0%, ≤ 1 wake-up a second | 0.01%, 0.4 / s; 0.0%, 0.25 / s | **0.0%, 0.2 / s** (A); **0.0%, 0.25 / s** (B); 9 threads, 20 s at rest | **Met**, off screen | No |
| An update, quit to first correct frame | One quit and one launch | First frame 158 / 165 ms; page drawn 320 / 345 | One quit, one launch. `rfc9110` with an edit, 11 hand-offs: first frame **344 / 366** p50 / worst; "page drawn" 624 / 643 (fade in it). Made file: first frame **475 / 944**, and 760 / 799 `[b-again]`; "page drawn" **3,694 / 4,776**, which is not the page (below) | **Met** (one quit, one launch) | First frame about 2x then (158 to 344): cause not found |
| Scroll | 120 fps, ≥ 99% on time, 0 blank frames on seen pages | UI step 105 / 690 / 886 µs; 1 blank step on a seen page in 2 of 6 sweeps | UI thread a step, p50 / p99 / worst: **163 / 702 / 1,023 µs** at 6 pages a second, **195 / 460 / 644** at 40 (A); B 162 / 334 / 477 and 195 / 544 / 719; six sweeps p99 629 to 846, worst 711 to 1,334. **Blank steps on seen pages: 0** in all 10 sweeps | **Met** on what a quiet run sees. Frames not tested | p50 up about 60 µs; blank steps fixed |
| Sharp after scroll stops | ≤ 100 ms on a text page | 6.1 / 12.9 ms | `rfc9110` **5.6 ms p50**, worst **243** (one of 11); made **4.9 / 8.5** | **Met** at p50. The 243 is likely the page number's fade (a guess: the tool waits for it, and a scroll that changes the page shows it) | Worst up |
| Background work | Helper, background QoS, exits when done | Not measured | Not measured for QoS. Index builds `[index]`: `rfc9110` 0.47 s, NASA 4.95 s, made file 2.13 s; helper peak 4.8 to 6.8 MiB, worker peak 30.7, 115.9, 32.3 | Not measured | Builds as on 2026-11-15 (0.48, 4.89 s) |
| Window closed | ≤ 20 MiB | 14.5, 13.7 | **13.5** (A), **13.0** (B), **12.8** `[closed]`, no worker; 9 to 17 threads, 17 to 18 handles; **0.0% CPU and no idle wake-up** in 20 s closed (`top`) | **Met** | Better by about 1 MiB |
| Waking a sleeping tab | Its place on its paper in one frame; sharp ≤ 100 ms on a text page; paper until drawn (owner, 2026-11-15) | First frame 1.5 to 2.9 ms (from a picture), sharp 90 to 238 | `rfc9110` p40, 5 wakes `[wake]`: first frame with the page drawn **89, 95, 96, 99, 100 ms**; sharp **96, 102, 103, 105, 107 ms**; 2 frames of plain paper first in each, as the owner's rule now has it. On the headless core `[rust]`: on its paper in **0.4 to 1.2 ms**, first tile 23 to 63, sharp 29 to 70 | **Missed on sharp** in 4 of 5, by 2 to 7 ms. Paper in place: met on the core, not measurable in the real app's record | First drawn frame: ~2 ms then (a picture) to ~95 now (the first tile), by the owner's design. Sharp tighter: no 238 |
| Live documents across a session | Flat over 24 hours | The soak | Not run: a 24-hour run isn't a review's | Not measured | |
| Disk writes | Idle 0; reading at most a small write a minute; editing appended steps only | Idle 0; sweep 0; 200 turns 5.7 KB journal, 8 KiB process | Idle 10 s: **0**; a sweep of 599 to 600 steps: **0**; 200 turns: **12.4 and 12.7 KB** to the journal, 16 KiB by the process's counter | **Met** | Bytes a step about 2x (28 to 62): cause not looked at |

## What changed since 2026-11-08 that could cost

- **Waking a tab with no page picture.** In the real app the first frame with the page drawn is now the woken tab's new worker's first tile, 89 to 100 ms, after two frames of plain paper; it was 1.5 to 2.9 ms from a picture. Sharp is 96 to 107 ms, as tight as before but at the line. On the core the pages are on their paper in 0.4 to 1.2 ms. A tab woken after an update has no facts to wake from (`had: nothing` in the hand-off probe) and its pages come as their sizes do. All by the owner's rule of 2026-11-15; the cost is the new worker's first tile, already in `docs/PLAN.md` (55 to 130 ms of it is the worker's first `IOSurfaceLookupFromMachPort` after its walls go up, measured 2026-11-11, not re-measured here).
- **Find over the encrypted text** `[index]`, `budget index`, 7 runs each, p50: a word nothing has, from the index against `--scan`: `rfc9110` **1.15 ms** against 157; NASA **1.99** against 1,095; the made file **4.23** against 253. A common word: 1.57 against 16.7, 2.29 against 17.8, 4.29 against 31.0. The index p50s for `rfc9110` and NASA match `search-index.md`'s after the encryption (1.11 to 1.22, 1.97 to 2.12): no move.
- **Cold start**: see the table. The tool's 445 ms is the marks' fade; the page is drawn at 274 ms p50.
- **The idle app**: 0.0% CPU, 0.2 to 0.25 wake-ups a second with the window open, none in 20 s with it closed; 9 threads at rest, 9 to 10 closed; 12.8 MiB closed. CI's 80 wake-ups a second (`docs/PLAN.md`) can't be seen here.
- **1,000 tabs**: as above, about 1 KiB a tab and a sidebar that opens in tens of milliseconds. Switching to a cold tab: 91 ms (NASA), 890 to 984 ms (the map, its first view).
- **Flicking through ten large tabs** (`flick_cost`, the core): 42.9 to 43.4 MiB after each round, 6 or 7 threads, against 43.0 to 43.3 on 2026-11-15.

## Input to frame

`input_to_frame` (release, the headless core, 500 timings a case), load 2.2 to 2.5. The window's own call is 1 to 8 µs at p99 in every case and at every count. From the call to what the frame is made from published, p50 in µs:

| Action | 1 tab | 100 tabs | 1,000 tabs |
|---|---|---|---|
| tap a row | 43.0 | 94.8 | 186.3 |
| open its tab | 73.4 | 122.2 | 227.9 |
| contents jump | 45.2 | 50.4 | 94.2 |
| typed page | 51.0 | 52.1 | 96.4 |
| find step | 49.8 | 60.1 | 94.1 |
| compare click | 44.8 | 118.7 | 219.6 |
| pane focus | 44.6 | 82.5 | 177.1 |
| zoom | 29.1 | 29.4 | 46.7 |
| rotate | 54.0 | 58.6 | 95.8 |
| open a new file | 224.3 | 248.0 | 362.2 (p99 1,948.5) |
| scroll within an entry | 24.6 | 28.6 | 49.5 |

All far under a frame. But almost every action costs about twice as much at 1,000 tabs as at one, and tapping a row, a comparison click and pane focus four or five times as much, which the rule says shouldn't happen for tabs the action doesn't touch. No base in the same hour was run (this is a review of one commit), so whether this is new isn't known; the cause wasn't measured.

## Where the time and memory go

- **The marks' fade in `qa budget`.** `qa wait-settled` waits for the window's animations, and since `667bfb6b` (2026-11-08 15:34, after the last report's build) the page number and zoom marks fade for about 190 ms after a change. Every probe saw `chrome_fading` as the last thing running: the sidebar's command answered in 6 to 25 ms and the fade ended at 270 to 545 ms; a cold start's page was drawn at 242 to 317 ms and the fade ended at 438 to 494. So the cold start, the sidebar and the hand-off's "page drawn" in `qa budget` read 150 to 300 ms slow, and the worst "sharp after scroll stops" probably does too.
- **A 10,000-page file's first open**: `lopdf`'s `IndexedReader::page_map` resolves every page object, one `pread` each, scattered over 2 GiB: 744 of 776 samples of a cold `recto local F facts`. Warm, it's 0.08 s of the 337 ms; the rest of the warm open wasn't sampled. Tracked (`docs/PLAN.md`, budget item of 2026-11-08, and the owner's go-ahead to push the fork's lazy page tree).
- **After an update, an edited 2 GiB file reads itself whole for its signatures** `[probe-handoff]`, `[probe-signatures]`. The page is drawn 0.3 to 0.6 s after the start, but the app isn't at rest until 3.5 to 5.3 s: `qa wait-settled` says "1 look(s) at a file's signatures" all that time. The sample has one thread in `lopdf`'s `rewrite::signatures` scan (963 samples at the top of its stack, 67 in `pread`). The app's footprint stays at 18 to 19 MiB, so it costs time on one core, not memory. Without the edit it doesn't happen, and on `rfc9110` it's over in time to be unseen. This is what `qa budget`'s 3.7 to 4.8 s "page drawn" in B was. Whether Save's state waits for it wasn't looked at. Not in `docs/PLAN.md`.
- **Shaded backgrounds, 214 MiB**: the worker holds 186 MiB at the settled page and lets it go within 4 s. The cause given on 2026-11-08 (PDFium's buffers for transparency) wasn't re-sampled. In `SPEC.md` §2 as missed; no open item in `docs/PLAN.md`.
- **The map at 2x and 3x**: the worker's peak is 421 to 481 MiB of the 455 to 515; the app is 34. Not sampled. In `SPEC.md` as an accepted exception, which it now passes; no open item.
- **A woken tab's sharp time**: the new worker's first tile, as measured on 2026-11-11 and tracked.
- **The 31 to 69 ms stalls**: not found. `qa state`'s `hangs.worst` only names a stall of a second or more, so these say nothing of what the thread did. Tracked as "at a quiet time, the worst stall".
- **The update's first frame at 344 ms** against 158 on 2026-11-08: the old app is gone at 171 ms and the new one's first frame is 173 ms after its start (A's last hand-off). Neither part was measured on 2026-11-08, so where the time went isn't known.

## What to fix first

Ranked by what a person reading PDFs would notice, memory first. Gains are estimates.

1. **Make `qa budget` time the page apart from the marks' fade and the background looks** (a measuring fix, before the rest): cold start, sidebar and hand-off read 150 to 300 ms slow, and B's hand-off 3.5 s slow. Gain: true numbers in every later report. `crates/recto-cli/src/qa.rs`. 0.5 day.
2. **The map at 2x and 3x, 456 to 515 MiB** against 450 accepted: the worker's peak. Gain, an estimate: back under 450 by capping the worker's drawing at a zoom. `crates/recto-render-service`, `crates/recto-render`. 2 days.
3. **Shaded backgrounds, 214 MiB** against 100 for about 4 s on such a page. Gain, an estimate: about half, drawing such a page in bands. `crates/recto-render-service`. 2 to 3 days.
4. **A 10,000-page file's first open, 2.0 s** against 600 ms (and warm 337 against 250): read the page tree lazily as far as the page asked for. Gain, an estimate: under 0.3 s cold. The lopdf fork's `indexed_reader/page_map.rs`; needs the owner's go-ahead to push the fork. 2 to 3 days.
5. **A new worker's first tile, 55 to 130 ms**: a wake (sharp 96 to 107 against 100), a cold start and a tab switch (91 ms) wait for it. Gain, from the 2026-11-11 trial: a wake's p50 87 to 41 ms. `crates/recto-render-service`, `crates/recto-platform` (the walls). 2 to 3 days, as the trial's fix couldn't go in.
6. **The window's thread stalls 31 to 69 ms**: first, record the worst stall under a second with what the thread was doing, then fix it. `crates/recto-app/src/watchdog.rs`. 1 day to see, the fix unknown.
7. **The signature look on an edited big file, 3 to 5 s of a core** after an update or a first edit: look only where signatures can be (the AcroForm's fields and the trailer's revisions) before reading every byte. Gain, an estimate: from seconds to milliseconds on a file with no signature fields. The fork's `indexed_reader/rewrite/signatures`, `crates/recto-core/src/signatures.rs`. 1 to 2 days, and it touches rule 5, so with care.
8. **Input to frame grows with tabs** (2 to 5x from 1 to 1,000 tabs): find the per-tab pass in what each action publishes. `crates/recto-app/src/core`. 1 day.
9. **The update's first frame, 344 ms against 158**: find which part moved. `crates/recto-app/src/core/updates.rs`, the host's quit. 0.5 day to find.
10. **Journal bytes a step about doubled** (62 against 28): find what each step now carries. `crates/recto-session/src/record.rs`. 0.5 day.

## Development speed

- `tools/check.sh --slow`, its latest kept run (3d70504c, everything, pass): tiers 1+2 (core and process) **60 s wall** against 120; tiers 3+4 (window and lifecycle) **136 s** against 330; the slow stage 46 s; stages 0 + 11 + 60 + 10 + 1 + 136 + 46 + 2 s. Slowest tests: `slow_stage::everything_comes_back_after_a_quit_when_every_wait_is_late` 44.4 s, `hostile_state_records_do_no_harm` 36.4 s, `a_long_session_stays_flat` 36.0 s. Test-seconds by tier: core 434, process 160, window 345, lifecycle 192.
- `tools/check.sh --runs`: `main` **120 full runs, 102 passed, 18 failed, 42,038 s**; this branch 0.
- CI's check time, from `push-main.sh`'s kept times (`recto-ci-times`): the last five green runs **1,229, 1,183, 977, 1,121, 1,120 s, median 1,121 s (18 min 41 s)** against the 21-minute budget; `push-main.sh --budget` gives no warning.
- `tools/sysguard/heavy.sh --report` (24 hours): **994 jobs, 352 failed** (6,722 s of 39,971); waits median 0 s, p90 1 s, longest 209 s; runs median 6 s, p90 107 s, longest 726 s; at most 4 running at once, swap up to 2,007 MiB.

The checks are inside their budgets and CI is 2 minutes under its own, so development speed is not where the time goes. What stands out is failure, not speed: a third of the queue's jobs in the last day ended in failure (it counts every failing command, named tests while working among them), and 18 of `main`'s 120 full runs failed, 15% of them, each one run again.

## Not tested

Frames reaching a display and 120 Hz; a visible window's memory; a true cold start (emptying the file cache needs root: "cold" here is a fresh APFS clone); Launch Services and Gatekeeper in an update; helpers' QoS; the off-screen byte budget of 256 MiB under memory pressure; memory flat over 24 hours; CI's idle wake-ups; a wake's "on its paper in one frame" in the real app (its record times the first drawn frame, not the paper; the core says 0.4 to 1.2 ms); whether Save waits on the signature look; a worker's peak during a Find query on a 10,000-page file. The old report's made 2 GiB file couldn't be reused (its generator wasn't kept), so the cold-open times aren't the same file's.

## Measured again with nothing else running (the coordinator, 2026-11-16 10:02 to 10:11)

The same release build (`main` at 50bd47d5), the same script's parts, `HEAVY_SOLO=1` with the queue empty, load 2.4 to 4.1 (`performance-2026-11-16/quiet/`, with `run.sh`, each part's load in `parts.txt`). One reviewer agent was reading code meanwhile, building nothing. Run B, the 2 GiB file's eleven starts and hand-offs, was left out. **The misses are the reader's, not the load's:** every line that missed under load misses here by about as much.

| Line | Target | Under load (above) | Nothing else running |
|---|---|---|---|
| Cold start, page drawn `[probe-cold]`, 11 starts | 300 ms p50, 500 p95 | 274 p50, 317 worst | **265 p50**, 282 for ten of eleven, **413 once** | 
| 2 GiB file warm, page drawn `[probe-made]`, 11 starts | 250 ms | 337 p50 | **317 p50** (310 to 340, and 497 the first) |
| 10,000-page file's first open `[coldopen]`, three clones | 600 ms | 2.01, 2.04, 2.03 s | **2.03, 2.03, 2.04 s**; 0.08 s cached |
| Shaded backgrounds, settled `[colour]` | 100 MiB | 214.2 (worker 185.9) | **213.8 MiB** (worker 186.2), 44.0 after 4 s |
| The map, about 2x and about 3x `[tabs]`, two runs | 450 MiB accepted | 456 to 459; 455 to 515 | **455.2, 455.9**; **510.9, 511.4** (worker peak 477); fitted 385 to 386 |
| A woken tab sharp `[wake]`, 5 wakes | 100 ms | 96 to 107 | **100.0, 102.7, 103.7, 106.9, 107.2 ms**; first drawn frame 93 to 100; 2 paper frames each |
| The window's thread's worst stall `[a]`, `[tabs]` | 16 ms | 31 to 69 | **36, 31, 44 ms** |
| An update's first frame `[a]`, 11 hand-offs | | 344 p50, 366 worst | **360 p50, 372 worst** (the old app gone at 206 ms, the new one ready at 341, in the last) |
| The signature look on the edited 2 GiB file `[probe-signatures]`, two hand-offs | | at rest at 3.5 to 5.3 s | page drawn at 423 and 324 ms; **at rest at 3.5 and 3.2 s** |
| Input to frame `[rust]`, published p50 at 1 and 100 tabs | | tap a row 43.0 and 94.8 µs | tap a row **43.8 and 96.1**, open its tab 67.5 and 119.7, compare click 42.6 and 110.2 (the 1,000-tab rows are in `quiet/rust.txt`) |

So: the load cost the cold start about 10 ms and the warm 2 GiB open about 20, and nothing else. The stalls are 31 to 44 ms on a quiet machine too (the 69 was the load's). The cold start's one 413 ms start is inside the 500 ms p95. Not compared here: `main`'s parent for the input-to-frame growth with tabs, which `docs/PLAN.md`'s numbers of 2026-11-15 already show is not new.

## After the day's fixes (the coordinator, 2026-11-16 13:36 to 13:41, nothing else running)

A release build of `main` at 7 merges on (the worker's allocator keeps no freed large blocks, `MallocLargeCache=0`, `build/map-zoom-memory`; `qa budget` times the page apart from the window at rest), `HEAVY_SOLO=1`, load 2.6 to 2.9 (`performance-2026-11-16/quiet-after-fix/`). Parts `tabs` (two runs), `colour` and `a`.

| Line | Target | This morning, quiet | Now, quiet | |
|---|---|---|---|---|
| The map fitted | 450 MiB accepted | 385 to 386 MiB | **322.7, 322.9** | |
| The map at about 2x | 450 MiB accepted | 455.2, 455.9 | **345.5, 345.8** | **Met** (was missed) |
| The map at about 3x | 450 MiB accepted | 510.9, 511.4 | **335.7, 336.1**; worker peak 373.9, 374.3 (477 before) | **Met** (was missed) |
| Shaded backgrounds, settled | 100 MiB | 213.8 (worker 186.2) | **96.5** (worker 68.8, its peak 141.4) | **Met** (was missed), by 3.5 MiB |
| Shaded backgrounds, 4 s later | | 44.0 | **96.3**: the worker rests at 68.8 MiB and isn't let go | Worse at rest: the next step |
| Colour page (NASA p50) | 100 MiB | 43.7 under load | **41.2** | Met |
| Text page (`rfc9110`) | 60 MiB | 36.0 under load | **34.9** (app 25.7, worker 9.2) | Met |
| Cold start, page drawn, 11 starts, by `qa budget` itself now | 300 ms p50, 500 p95 | 265 p50 by the probe | **239 p50, 247 p95 and worst**; first frame 71 ms; the window at rest 434 | Met |
| Sharp after scroll stops, 11 | 100 ms | | **7.5 ms p50, 20.9 worst**; the window at rest 243.7 worst (the fade) | Met |
| Window closed | 20 MiB | 13.4 | **13.0** | Met |
| The window's thread's worst stall | 16 ms | 36, 31, 44 | **39, 40, 41 ms** | Missed, as before |

So of this morning's seven misses, two are gone (the map zoomed, shaded backgrounds), by one setting; the 10,000-page open and the signature look are asked of the lopdf fork (its inbox, 2026-11-16); a woken tab's sharp time and the stalls stand. New from the fix: a worker that has drawn the shaded page rests at about 69 MiB for its 30 s idle time where it used to be let go at 3 s.

## After the resting worker's rule (the coordinator, 2026-11-16 14:59 to 15:02, nothing else running)

A release build of `main` with `build/resting-worker` merged (a render worker resting over 48 MiB is let go after 3 s), `HEAVY_SOLO=1`, load 2.7 to 2.8 (`performance-2026-11-16/quiet-after-rest/`). Parts `colour` and `tabs`.

- **Shaded backgrounds:** settled **97.1 MiB** (two workers, 69.0 MiB between them: the page's own and the one left from the NASA page opened before it in the same run); **4 s later 42.6 MiB**, one worker of 14.8 (96.3 MiB at 13:39, 44.0 that morning before the allocator setting). The "worker rests at 68.8 MiB" in the section above was those two workers read as one.
- **The map, two runs:** fitted 322.7 and 322.8 MiB, about 2x 345.5 and 345.6, about 3x 335.8 and 335.9, worker peak 374.1; 33.4 at rest 4 s later. As at 13:36.
- The window's thread's worst stall in those runs: 40 and 41 ms.

## To measure again with nothing else running

The lines that miss or are close, for the coordinator at a quiet time, each with `HEAVY_SOLO=1` and `MADE` and `BENCH` set as the script says (`python3 -I docs/reports/performance-2026-11-16-bigpdf.py "$MADE"` makes the made file; `BENCH` is a temp folder with copies of the four `bench100` files the script names):

- Cold start and the warm 2 GiB open, timed apart from the fade: `docs/reports/performance-2026-11-16.sh OUT probe-cold` and `... OUT probe-made`.
- The cold open of a 10,000-page file: `... OUT coldopen`.
- The worst stall, the map at 2x and 3x, the update's first frame: `... OUT a`, `... OUT b`, `... OUT tabs`.
- Shaded backgrounds: `... OUT colour`.
- A woken tab's sharp time: `... OUT wake`.
- The signature look after an update: `... OUT probe-signatures`.
- Input to frame at 1, 100 and 1,000 tabs, with 4b4e027c's parent beside it in the same hour to see whether the growth with tabs is new: `... OUT rust`.
