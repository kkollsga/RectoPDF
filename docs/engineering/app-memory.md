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
