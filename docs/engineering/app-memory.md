# The app: memory, many tabs and measurements

> Part of the engineering notes: [`README.md`](README.md).

**Many tabs,** before and after these fixes, on one app growing to 10,000 tabs of `rfc9110`, at load 8 to 16. Off screen.

| | 1,000 before | after | 10,000 before | after |
|---|---:|---:|---:|---:|
| Opening them all, by `recto open` each | 121 s | 31 s | 1,301 s | 272 s |
| A `goto`, settled | 163 ms | 67 ms | 158 ms | 24 ms |
| Sidebar back after focus view | 42 ms | 91 ms | 72 ms | 32 ms |
| UI step p99, 6 pages a second | 670 µs | 760 µs | 704 µs | 342 µs |
| Steps got through, of 600 | 592 | 595 | 462 | 600 |
| Worst stall of the UI thread | 53 ms | 210 ms | **2,960 ms** | 605 ms |
| App | 29.4 MiB | 25.7 MiB | 35.9 MiB | 34.8 MiB |

The 1,000-tab "after" ran at a higher load than the "before". Run again later at load about 10, not alone: opening them all 18 s, a `goto` 19 ms, sidebar back 25 ms, UI step p99 461 µs, 600 of 600 steps, worst stall **81 ms**, 29.2 MiB. So sidebar back and most of the stall were load, but it isn't settled that 81 is no worse than the 53 before: a quiet run of both builds is a follow-up in `docs/PLAN.md`.

**The rise per tab,** 500 tabs of a file, by `heap` and `vmmap`, at load 10 to 20. The suspects were fresh surfaces, and tiles not yet discardable. Neither is growing: the app's `IOSurface` dirty memory doesn't move by a KiB from 2 tabs to 500, or after 20 s more, and the rise is small. NASA 23.8 to 24.1 MiB, the 2 GiB file 13.3 to 13.5, USGS 20.0 to 20.3, with the same largest classes in the same amounts on all three. Over 1,000 tabs, the 2 GiB file rose 0.25 and 3.31 MiB on the same build, the higher one with 2.27 of it already by 100 tabs and little after, and NASA 0.30, against about 3 before. So it isn't a cost per tab, and it was within the budget of 4 each time, but that one high run wasn't caught under `heap`, so what it was is not known.

**Surfaces made outside the reader's lock,** two builds differing by only that, side by side on the 2 GiB file at load 7 to 20, two runs each: worst stall of the UI thread 36 and 22 ms with the lock held, 18 and 17 with it let go. Two runs each isn't proof, and one surface takes 12.5 µs to make and drop, so this doesn't explain the 22 and 43 ms seen earlier by itself. UI step p99 at 40 pages a second: 1,548 and 237 µs before, 766 and 679 after, which is load.

**Measured** 2026-11-07, release build, this Mac (M4, 16 GB, other agents building at the same time, load 8 to 15), quiet run: a window off every screen, so **frames reaching a display, what a visible window's backing adds, and 120 Hz aren't in any of this**. `recto qa budget --file F --heavy H`, whole tree unless it says.

| Budget (`SPEC.md` §2) | Target | Measured | |
|---|---|---|---|
| Cold start to first sharp page, `rfc9110`, process start to settled, 21 runs | ≤ 300 / ≤ 500 ms | **231 / 300 ms** p50 / p95 at load 10 to 20 (252 / 300 at load 8), and **171 to 245** side by side with the build before these fixes at load 15 to 28, where that took **373 to 561** | Met |
| One document, text page | ≤ 60 MiB | **32.9 MiB** | Met |
| Colour page: NASA `ntrs20240002628` p50, images | ≤ 100 MiB | **37.7 MiB** | Met |
| The same: NASA `ntrs20220005969` p175, shaded backgrounds | ≤ 100 MiB | **210 MiB**: the worker's 184, PDFium's transparency buffers, as already measured in `budget viewport`. It goes after 3 s at rest | **Missed** |
| Scrolling, peak: `rfc9110` at 6 / 40 pages a second | ≤ 170 MiB | **40 / 43 MiB** | Met |
| The same: NASA images | ≤ 170 MiB | **97 / 91 MiB**, 99 / 103 before. 40 pages a second on `rfc9110`, side by side at load 15 to 28: 134 to 160 before these fixes, 44 to 45 after | Met |
| UI thread's work per scroll step, 120 steps a second | ≤ 2 ms p99 | **0.30 / 0.48 ms** p99 | Met, off screen |
| Blank on a seen page, during those sweeps | 0 | `rfc9110` **0**. NASA **0** at 6 a second, **12 steps** at 40, as many as 189 under more load | **Missed at 40 a second past 128 pages.** 128 previews are kept, so a page 128 pages back has lost its own. Replayed: blanks fall only on the pages seen earlier, and only after that many. The first measure of 0 was before every page got a preview. Disk previews are what fixes it. A page in view by less than a pixel, inside the part of one at its foot or right edge that `device_size`'s rounding leaves without a tile, has nothing in view to show and isn't counted (`shows_a_pixel`): it was the budget report's 1 blank step in 2 of 6 sweeps at 6 a second (2026-11-11) |
| Sharp after scroll stops, on pages not drawn yet | ≤ 100 ms | **10 / 13 ms** p50 / worst, one client round trip included | Met |
| Idle CPU, one document, 20 s, app only | ~0%, ≤ 1 wake-up/s | **0.0%, 0.4 to 1.35** over 20 s from 1 s after settling, with the end of settling in it. Counted in steps of 5 s from settling, `rfc9110` and NASA alike, three runs at load 24: 13 to 62, then 3 to 5, then 0 to 5 each, so under 1 a second from then on | Met, off screen. A worker parked in `poll` wasn't counted |
| Window closed | ≤ 20 MiB | **11.7 to 14.4 MiB**, no worker | Met |
| 1,000 tabs of a PDF, over 2 tabs, app's own | ≤ 4 MiB | USGS map **0.31**, NASA **0.30**, 2 GiB file **0.25 to 3.31**, varying with load: 1.19 and 0.06 in the run before. Each was drawn when opened | Met |
| Sidebar opens, 1,000 tabs: focus off to settled | < 150 ms | **1.4 to 16.6 ms** | Met |
| Handles at 2 tabs and 1,000 | same | 22 to 25, varying with what's warm. 3 documents held at most | Met |
| 2 GiB, 10,000 pages, page 1 sharp, warm | ≤ 250 ms | **198 ms** p50, from process start | Met |
| The same, cold | ≤ 600 ms | **468 ms** worst of 11, its first run after writing the file. Not a true cold start: purging the file cache needs root | Not tested as such |
| Main never stalls over 16 ms on that file | ≤ 16 ms | **17 to 18 ms** at worst over the whole run, at load 7 to 20, down from **22**, which is one stretch of work in launch or layout, not a page. 20 ms with `rfc9110`, 43 with NASA | **Missed by 6 ms** |
| USGS map, peak, under the named exception | 415–450 MiB accepted | Fitted **384 MiB**. Zoom 3, about 2x: **440**. Zoom 4.5, about 3x: **496**, worker 459 and app 37. Before, the worker alone at 2x and 3x peaked at 394 and 452 | **Over at 3x** by the tree, as the surfaces were in the worker's number before |
| USGS map, first view, scroll, zoom | No target | **901 ms** from process start. Zoom to 2x 513, a quarter window 67, 1.5x more 677, at load 10 to 20 (733, 372, 89 and 644 at load 8). Before, by `budget viewport`: 534, 101, 379 | The first and last draw at higher scales than before |
| 120 Hz, ≥ 99% of frames on time | | **Not tested**: needs the MacBook and a real screen | |

**Before and after on the app's own fixes,** same runs. `TextLine` for `NSTextField`: 1,000 tabs of NASA 5.68 to 2.48 MiB over 2 tabs, and a 400-tab run's heap growth from 2.9 MiB to 0.1. Letting a big worker go: USGS map at rest 433 to 36 MiB. Previews outliving tiles: 84 blank steps on seen pages at 40 pages a second, in a debug run, to 0.

**About the off-screen numbers.** A real window has a backing store and a window-server share that this doesn't, so treat the memory figures as a floor until the MacBook spike. The platform report's sRGB window alone cost 17.6 MiB in a visible state.

## The heavy map at a zoom, and what the worker holds (2026-11-16)

The USGS map (`gov_usgs_usgs70277338`, 346 MiB, one page, 66 images) was past the 415 to 450 MiB the owner accepted: 455 to 458 MiB of tree after a zoom to about 2x and a scroll, 510 to 513 after about 3x, the worker's peak 477 to 480.

**What the worker holds.** The page drawn in-process with PDFium as the reader asks for it (one region of at most 32 MiB, view 3024 x 1964, scales 0.681, 2.043, 2.043 a scroll down, 3.065), `vmmap --summary` and `malloc_history -allBySize` under `MallocStackLogging=lite` after each step, release, load 4 to 10:
- What PDFium really holds is **about 300 MiB, the same fitted and at 3x**: the page's 73 image streams read whole when the page loads (`CPDF_SyntaxParser::ReadStream`, kept with the document) **160 MiB**; its cache of decoded images (`CPDF_PageImageCache`, 27 bitmaps) **95**; one image stream's bytes read again to decode it **30**; paths and fonts about 10.
- The rest was **freed large blocks the allocator kept** ("Malloc Large (empty)"): 52 MiB after the fitted draw, then 109, 170, 197, as each zoom's buffers came in new sizes and were let go. So the footprint went 361, 422, 483, 511 while PDFium's own stayed at 300.8 to 301.0. `malloc_zone_pressure_relief` released none of it; `MallocLargeCache=0` stops it (`MallocDeferredReclaim=0` and `MallocAggressiveMadvise=1` didn't). With the cache off the same steps peaked at 317, 373, 395, 397 MiB, the footprint back to 297 after each: a zoomed pass needs about 67 MiB above what the page holds, besides the region's own 30.

**The fix:** the host starts each worker with `MallocLargeCache=0` (`crates/recto-render-service/src/worker_env.rs`). **What else the variable does** (read from libmalloc's source by a reviewer, not measured here): in the newer allocator (xzone, `xzone_malloc.c`) `MallocLargeCache=0` also sets `defer_tiny`, `defer_small` and `defer_large` false, so deferred reclaim for tiny and small sizes is off too, not only the large cache. Which allocator the worker uses on this OS isn't known. The CPU a sweep costs the worker is measured below (2026-11-16, the resting worker). It reaches the sandboxed helper (`ps -E` on the app's worker shows it). Release app, `recto qa budget --file rfc9110 --heavy <map> --runs 1`, three runs each, the base at 12:58 to 13:02 (load 3.4 to 6.0), the fix at 13:08 to 13:13 (load 2.6 to 6.5), other workers' builds in the queue beside both:

| The map | Before | After |
|---|---|---|
| Tree, fitted | 384.7, 385.8, 389.2 MiB | **322.8, 323.0, 322.9** |
| Tree after about 2x and a scroll | 454.6, 455.6, 458.0 | **345.6, 345.8, 345.8** |
| Tree after about 3x | 510.4, 511.1, 513.5 | **335.9, 336.1, 336.1** |
| Worker's peak over the run | 476.7, 477.4, 479.6 | **374.0, 374.1, 374.2** |
| App; tree 4 s later | 33.7 to 33.9; 33.5 to 33.6 | 33.6 to 33.7; 33.5 to 33.6 |
| First view from process start | 1,164, 1,187, 2,047 ms | 1,163, 1,065, 1,092 |
| Zoom to about 2x, settled | 410, 408, 507 ms | 410, 417, 408 |
| Scroll a quarter window | 65, 64, 78 ms | 65, 65, 65 |
| Zoom to about 3x, settled | 562, 550, 897 ms | 553, 545, 546 |

So the worst the tree can have reached is the worker's peak plus the app, about 408 MiB, against 511 before: under the 450 accepted at every step. No page was drawn at a lower resolution in any run. The zoom times are settled times, the marks' fade in them, and didn't move. In-process the pass itself is a little slower without the cache (each large block is fresh pages from the system): three runs each at load 8 to 10, median ms a pass with and without, fitted 518 and 565, 2x 260 and 291, the scroll 387 and 443, 3x 277 and 311, about a tenth, which the app's settled times don't show.

**Ordinary pages,** the same runs, before and after: `rfc9110`'s text page 35.4 to 36.0 MiB and 34.8 to 35.0 (worker 9.6 to 10.0 and 8.9 to 9.1); scrolling at 6 pages a second 43.4 to 43.7 and 42.2 to 42.8, at 40 44.3 to 45.7 and 43.5 to 43.8. A quiet run per build of each colour page, settled, three each: NASA `ntrs20240002628` p50 43.0 to 43.3 and 41.0 to 41.5 (worker peak 16.4 to 16.5 and 14.3 to 14.7). **Shaded backgrounds** (`ntrs20220005969` p175): settled **213.2 to 213.5 MiB and 96.2 to 96.5**, worker peak 185.3 to 185.5 and 140.5 to 140.8. But 4 s later it's 44.2 to 44.5 before and **96.0 to 96.3 after**: that page's worker now holds 53.5 to 53.8 MiB after drawing (the tree less the app and the p50 worker), under the 64 MiB at which a resting worker is let go after 3 s (`big_workers_rest`), so it stays until its 30 s idle exit, where before it held 168.8 to 168.9 and went at 3 s.

**The shaded page's peak**, by the same in-process way, drawn per 512 px tile at the app's 2.48 over a 1520 x 1400 view, `MallocStackLogging=full` for the freed blocks: what's held after the draw is 51 MiB; the peak, 123.5 MiB with the cache off, is one **soft mask in JPEG 2000 decoded by OpenJPEG at 4 bytes a sample, 71 MiB** (`CPDF_DIB::StartLoadMaskDIB` > `LoadJpxBitmap` > `opj_j2k_update_image_data`), with another mask's 17.8 MiB and three JPEG 2000 images' 3.4 to 4.5 MiB each. Not PDFium's buffers for transparency, as was said on 2026-11-08.

**Held by** `memory::zooming_into_a_heavy_page_costs_no_more_than_the_zoomed_view_alone`: a made page with the map's size and its two largest images (5104 x 6004, run-length encoded so the file is 2.9 MB), drawn through the reader's steps, may peak no higher than the 3x view drawn alone in a fresh worker plus 16 MiB. With the cache on it was 96 against 59 MiB, off 63 against 59. The made page holds tens of MiB where the map holds 300: it tests what a zoom adds, not what the map costs.

**Not measured:** a visible window; NASA scrolled and zoomed as run A has it (52.4 MiB in the report); the made 2 GiB file's scroll; any of this with nothing else running.

## A worker that rests holding a lot, and the line it is let go at (2026-11-16)

**What the rule was.** `big_workers_rest` (`crates/recto-reader/src/engine/run.rs`): when the engine's thread has nothing to do, a worker whose own footprint (`worker_footprint`, the same `phys_footprint` read as `qa state`) is over `Policy::retire_bytes` is let go after `heavy_idle_exit` (3 s); every other worker waits the 30 s of `idle_exit`. `retire_bytes` is 64 MiB and is also the line a worker retires at between pages in use, so it couldn't simply be lowered.

**Why the shaded page's worker stayed.** The rule read the right number and compared it right; the number was under the line. `memory.worker_bytes` in `qa state` is the **sum of every worker**, and the report's 68.8 MiB was two workers: the NASA p50 file's (14.5) opened first in the same quiet run, and the shaded page's own, which rests at **53.5 to 54.0 MiB** (nine runs). Opening the shaded file alone: tree 81.0 to 81.5, worker 53.6 to 54.0, still there 27 s later, gone at the 30 s idle rule (release app, three runs, load 2.7 to 3.4, `build-resting-worker-base-shaded-*`). With the NASA p50 file open first, 96.1 to 96.6 and two workers, as the report had (`...-base-shaded-after-nasa`).

**What workers rest at** (release app, quiet runs, one run a file and page, the worker's own footprint): `rfc9110` p40 9.5 to 9.8 MiB (3 runs), NASA `ntrs20240002628` p50 14.5 to 15.0 (3), after four sweeps of 6 pages a second (30 pages each) through `rfc9110` from p64 15.5 to 15.8 (3), through the NASA file from p20 22 to 42 (the worker is retired past 64 on the way, so what it rests at depends on where the last retirement fell; 6 runs), the shaded page 53.5 to 54.0, the USGS map 300.7 to 301.5 (let go at 3 to 4 s, both before and after).

**The change.** `Policy::heavy_rest` (`over_bytes`, with the 3 s `after` that was `heavy_idle_exit`), its own line for letting a resting worker go, **48 MiB**: above everything an ordinary page left (42 at most) and under the shaded page (53.5). A first try at 32 let go NASA workers after a scroll (34 to 42 MiB), which an ordinary page's worker mustn't be, so it went to 48; the margin on each side is about 6 MiB, so a worker that rests between 42 and 54 is a case to look at again with a new page. `retire_bytes` (64) is unchanged for what it decides between pages. Held by `a_worker_is_let_go_at_rest_over_the_rest_line_and_kept_under_it` (`recto-reader/tests/engine.rs`): seen failing with the rule on `retire_bytes`.

**Before and after** (release app, quiet, other workers' builds and tests running beside every run: load 3 to 25, written beside each file; the coordinator measures again with nothing else running). Files `build-resting-worker-*.txt` in the worker's scratchpad.

| | Before (main) | After |
|---|---|---|
| Shaded page, tree at rest / 4 s / 10 s / 35 s | 81.5, 81.2, 80.6, 26.8 MiB (the 30 s rule); 3 runs alike | **81.2, 27.2, 27.1, 27.1**; 3 runs alike |
| Shaded page after the NASA p50 file, 4 s | 96.2 (two workers kept) | **42.0** (the shaded worker gone, NASA's 14.3 kept) |
| USGS map fitted, at rest / 4 s / 12 s | 324, 22.9, 21.3 | 322.7, 21.4, 21.1 (3 runs alike) |
| `rfc9110` p40, worker at rest / 35 s | 9.7 MiB, kept; 26.2 at 35 s when the 30 s idle rule goes | 9.3, same |
| `rfc9110` 4 sweeps of 6 pages a second, workers started | 2, 2, 2 | 2, 2, 2 |
| NASA `ntrs20240002628` the same, workers started | 3, 4, 3 | 3, 3, 4 |
| Workers' CPU in those sweeps (sum of each worker's last `ps` time, polled at 5 Hz) | rfc 1.24, 1.31, 1.61 s; NASA 5.63, 5.90, 5.55 | rfc 1.25, 1.59, 1.55; NASA 5.56, 5.36, 5.81 |
| App CPU in 20 s at rest, shaded page / `rfc9110` p40 (`ps` time, `top` wake-ups) | 0.00 s, 0 wake-ups / 0.00 s, 0 | 0.00 s, 0 / 0.00 s, 0 to 1 |

A sweep rests at its end in the NASA file at 22 to 39 MiB, under the line, and no worker was let go in any run. The first sweep's worker is over 64 and retires either way.

**What scrolling on after a rest costs.** Shaded page p175, a rest of 5 s, then `goto` p176 and time to settled (the page's own fade included): worker kept 296, 315, 289, 304, 303 ms (main), worker let go 322, 309, 297, 291, 283 ms (after), at load 14 to 25. No difference can be seen at this load; a rest of 1 s with the worker kept is 26 to 35 ms in both, so the 300 ms is what a 5 s rest costs in the app either way and not the worker's start (PLAN: 55 to 130 ms). Not measured with nothing else running.

**Idle.** Nothing polls: the thread parks for the time left when a worker is over the line and for ever otherwise; the same rule, one more comparison. 20 s at rest, the app's CPU 0.00 s and 0 idle wake-ups on the shaded page and `rfc9110` before and after.

**The allocator setting's CPU** (coordinator's question, the worker's CPU over the same sweeps, `MallocLargeCache=0` off, on `main`'s parent of 1ead938b, against `main`): `rfc9110` 1.86, 1.53, 1.02 s against 1.24, 1.31, 1.61 (means 1.47 and 1.39, about 5 % less); NASA 5.79, 5.24, 4.59 against 5.63, 5.90, 5.55 (means 5.21 and 5.69, **9 % more with the setting**), at load 3 to 13 and 3 to 16. Within the noise of load, under the 10 % that was to be reported; the sum undercounts workers that lived under 0.2 s, and without the setting the NASA sweeps started 7 to 9 workers against 3 to 4 with it (the cache let a worker pass 64 MiB sooner, so it retired more often), so the real CPU without it is higher than the sum shows.

**Not measured:** anything with nothing else running; the cost of a worker started after a rest apart from the page's own fade; a Mac that isn't this one.

## A click after a pause, and the gentler rest for 48 to 64 MiB (2026-11-16)

Release app and release click harness (`core::headless::click_cost`, `RECTO_CLICK_ONLY=margin RECTO_CLICK_PAUSE_S=<s>`), quiet, other workers' builds and tests running beside every run (load 2 to 25, written in the files `build-rest-and-click-*.txt` of the scratchpad; the coordinator measures again alone).

**A click on a margin of page 1, three runs each, before:** the USGS map within a second of drawing: 0 workers started, the tree's peak 312.1 to 312.6 MiB, decided at once. After a 5 s pause (the tab's worker let go, 0 alive): **1 worker started, the tree's peak 186.4, 187.0, 186.0 MiB over 13.8 to 14.0 at rest, decided in 0.10, 0.08, 0.08 s**, the worker let go 3 s later. `rfc9110`, within a second and after 5 s: 0 workers started, 25 MiB, at once (its worker rests at 9 MiB and is kept). So "a click that picks nothing starts no worker" held only while the worker was alive.

**After:** the tab's worker, which has the page loaded for its first look (`JobKind::Traits`), reads the click's places in the same job (one walk, 0.6 ms on the map, at most 520 bytes a page, kept in the renderer under the 4,096-page cap), so a click after a pause is answered from what's kept: the map after 5 s, **0 workers started, peak 14.0, 13.9, 14.0 MiB, decided at once**; within a second and `rfc9110` as before. No job is added: the walk is inside the job a page's first look already is, so it is done as pages come into view, scrolling included. Cost of that, sweeps of 6 pages a second (4 of 5 s), three runs each, A (before) against C (after): `rfc9110` worker CPU 1.50, 1.46, 1.10 s against 1.38, 1.48, 1.14; NASA `ntrs20240002628` 5.56, 5.63, 5.41 against 5.68, 5.38, 5.23; shaded NASA 5.82, 5.60, 5.52 against 5.55, 5.70, 5.49; workers started identical (2, 3, 2 each); the UI thread's step p99 at most 0.3 to 0.9 ms in both, its worst 0.8 to 6.7 ms before and 0.9 to 14.7 ms after (one `rfc9110` run at load 6 to 9: not repeated in the two others); the shaded sweep's worker peak 124.9 to 128.6 before and 128.4, 131.1, 142.5 after. Not seen as a cost, and not proven to be none: the one 14.7 ms step and the 142.5 MiB peak are single runs under load.

**The turn after a pause on a page whose worker rests between 48 and 64 MiB** (shaded NASA p175 to p176 and p178 to p179, the two pages of the file that rest there; `qa state` polled until the reader is settled with a tile on show, not the fade; a fresh app for each; five runs each; load 6 to 14): with the worker kept (the old line, 64 MiB): p175 to p176 **47, 53, 53, 47, 55 ms** after 5 s, 56, 57, 47, 56, 57 after 1 s; p178 to p179 993, 1341, 1089, 1092, 1246 after 5 s, 968 to 981 after 1 s. With the worker let go (the 48 MiB line, 3 s): p175 to p176 **182, 234, 317, 333, 389 ms** (about 265 more), p178 to p179 1193 to 1308 (about 190 more on a 1 s page). Over about 100 ms, so a worker under the retirement line (64) is now let go only after **12 s** (`HeavyRest::gently_after`), one over it still after 3 s. 12 s: a reader turning a page of this kind keeps the worker through a pause of up to 12 s; one who stops for longer pays the turn once. Nothing measured says what a person's pause is. The shaded page's tree, alone in its app (3 runs): **81.2, 80.9, 81.1 MiB at drawing and 80.6 at 4 s, 80.2 to 80.7 at 10 s, then 26.9 to 27.2 from the 11th or 12th second**. So the SPEC line's "43 MiB four seconds later" is 81 MiB four seconds later and 27 after about 12 s (with NASA p50 open beside it, 96 then 42).

**Rewritten files** (`places_kept_of_a_file_are_not_served_once_another_app_has_rewritten_it`): the click's and dark paper's kept answers were given from the old file after another app's rewrite until a job's stamp check ran; they now check the stamp (one `fstat`, only when something is kept) before answering.

