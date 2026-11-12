# recto-render-service

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-render-service` | **Started** | Library: the host's side, `DocRenderer`, limits, validation. Binary: the worker, a child process that runs sandboxed in its own helper app inside the app, and refuses to run there unsandboxed | Link PDFium into the host |

## The renderer worker

**The process.** One worker per document, in a plain child process on a socket pair, with shared memory for pixels and one shared word for cancelling. It inherits nothing but its channel (`POSIX_SPAWN_CLOEXEC_DEFAULT`) and an empty environment, gets the document as a descriptor, and never learns a path. No thread in the host: the call blocks its caller, which is never the UI thread, and watches while a job is in flight. A cold tab has no `DocRenderer`, so it pays for none of this.

**Limits**, all held from outside the worker, because it may be compromised:

| What | Default | Why |
|---|---|---|
| Hard memory limit | 512 MiB footprint, read every 5 ms during a job, then the sequence below | The worst real page reaches 493 MiB. macOS has no OS limit for this: `setrlimit` refuses `RLIMIT_AS`, `RLIMIT_DATA` and `RLIMIT_RSS` with `EINVAL` |
| Page deadline, with kill | 5 s | Platform §2.6. The slowest real page at 2x took 939 ms |
| Retirement | 64 MiB after a job, read from outside; the worker goes before its next job for another page. A footprint that can't be read, or reads as nothing, retires it after 8 such jobs in a row rather than never | Only exit returns PDFium's memory (spike §6). Tiles of one page share a worker: retiring between them cost 72 respawns and 14 s on the USGS map. On macOS 15 CI in the sandbox the footprint after each page never crossed the test's fixed 24 MiB though the peak did, so the test now puts its line halfway up the growth it sees, and prints each page's footprint |
| Cancel by generation | Kill 90 ms after a job goes stale, if it hasn't returned | A stale render is gone within 100 ms, by return or kill. PDFium's pause callback runs only between objects |
| Idle exit | 30 s after the last job, 3 s for a heavy worker | |

**The memory sequence** (owner, 2026-11-06), all decided in `DocRenderer`, with only the reading of free memory in `recto-platform`:

1. **512 MiB.** A worker with history gets one more try in a fresh one, because what stopped it may be what earlier pages left behind.
2. **That page alone at up to 1 GiB,** if `kern.memorystatus_level` is at least 30 and no other page has the one heavy slot. 30% is about 4.8 GiB free on 16 GB, and 2.4 GiB on 8 GB, over twice what the retry can use. The slot is per process and goes with the worker, so the worst case stays near 1 GiB. A viewer takes it without waiting. The heavy worker goes before any other page.
3. **On screen only: a lower-resolution version at a quarter of the scale,** under the normal limit, reported in `Tile::low_res` with its scale, rectangle and reason. A page lacking only memory or the slot may go sharp later (`may_clear`); one past 1 GiB won't.
4. **`PageFailed`,** with words saying even that couldn't be drawn, only once every step has really failed. If the larger limit was out of reach and the smaller version didn't fit, the page isn't given up on: `NotDrawableNow` says why, without running the kill again, until the slot or memory comes back.

What's learned holds at that scale and above, since a higher zoom costs more, and a zoom under 70% of it gets its own try (`page_draw_state_at`). Each state keeps the lowest scale it was learned at, and they only move down, so a kill at a lower zoom doesn't take away what a higher zoom learned, and going between two zooms runs no kill twice. A failed open is remembered too, so a search of a bad file doesn't start a process per page, for only as long as it holds: "not a PDF", a password or a worker the file stopped hold until the file is a different one, when it's opened anew, and a failure that isn't about the bytes, such as a failed read of its size, holds for 2 s (`open_retry_pause`). The app needn't remember to call `forget_failures`, which is for the reader's "try again". Failures clear by themselves in those ways, and when the tab closes; `forget_failures` is for the reader's "try again". To stop a file of many bad pages: at most 8 heavy retries per document, then pages go straight to step 3, and at most 128 kills, then `DocumentStopped`, until asked. A cancel between steps ends the sequence, taking no slot and starting no worker.

**Too large at this zoom** names the largest scale that fits, from the page's size and the caps (`largest_scale_that_fits`), to two decimal places, so an agent doesn't try 8, 4, 2 and 1 by hand. It's a statement about size alone: a heavy page can still be refused for memory at that scale, in words of its own. `render --scale max` draws at that scale and says which, and goes down the same limits as any export, so it's sharp or it isn't written.

**A refused export says the limit, and how long it took.** Whether a smaller scale is offered goes by the picture's own size: it must be at least an eighth of the 1 GiB limit, at a scale above the smallest there is. Otherwise it's the page's content using the memory, and the refusal says a smaller scale won't help. `form_bomb` is 625 KiB at 2x, 9.8 KiB at 0.25x and 9.8 MiB at 8x, and fails the same at each in 2.9 s. `render` gives `took_ms`, and says it in plain words too.

**Exports differ** (owner, 2026-11-07). Whole-page drawing (`recto render`, later print and snapshot) goes in strips under the same limits, with `DrawPurpose::Export`. Step 3 doesn't apply: an export is sharp or isn't written. If it needs the larger limit it sleeps for the slot, up to `export_slot_wait` (60 s), ahead of any viewer, and a cancel ends the wait at once. No memory free, the wait running out, or the page passing 1 GiB each refuse plainly with nothing written: `export_memory_tight`, `export_wait_timed_out` (exit 7), `export_page_too_heavy`. It can block for a long time, so it's a call for a job thread, never a UI thread. On-screen tiles still never wait. The slot's ticket lives in the heavy process's end action, so it frees when the process has really gone, however it went, with nobody needing to touch its document. An idle heavy worker leaves after 3 s (`heavy_idle_exit`), and is killed by 8 s if it doesn't. A worker the host doesn't watch would be free to grow, so during a job the worker also runs a small thread of its own that checks its footprint, its deadline and whether the host is still there. That thread starts with the first job and parks between jobs.

**Between jobs and at exit.** One reaper thread in the host, alive only while a worker is, sleeps in `kevent`. It collects a worker the moment it ends, and at most once per `idle_exit + idle_grace` (35 s) per live worker it checks the idle rule: past its time or its memory limit, the worker is killed. That's one wake-up per 35 s per live worker, none with no worker, and no thread or handle left behind. A compromised worker could still grow unseen between those wake-ups: closing that needs the sandbox's memory limit. Killing never blocks: a process stuck in a read from a dead network volume can't die until the read returns, so the host signals it and moves on, and the reaper collects it whenever that happens. This wasn't tested against a real dead volume; the property tested is that nothing waits.

**Measured** 2026-11-06, release build, this Mac, warm cache, against the spike. Per-page figures are medians of 3 runs over the same 5,689 pages of bench100, at 2x, whole pages, no retirement, as in the spike. `recto local F budget render --count 200 --samples --retire-mib 4000 --limit-mib 4096`.

| | Spike | Now |
|---|---:|---:|
| Idle worker footprint, 423 spawns | 2.27 MiB | 2.27 MiB |
| Worker binary | 1.3 MB | 0.48 MB |
| Text and vector pages, p50 / p95, as the host sees it, round trip | 2.06 / 8.64 ms, in process | 2.57 / 8.31 ms |
| All pages, p50 / p95 | 2.41 / 30.39 ms | 2.98 / 28.33 ms |
| Respawn, reopen, draw first page, `rfc9110` | 13.28 ms | 12.95 ms |
| `gov_nasa_ntrs20240002628`, peak with retirement at 64 MiB | 115.8 MiB | 83.2 MiB |
| Largest peak across files, p95, with retirement | | 153 MiB (257 without) |
| `form_bomb`, every step of the memory sequence | 4 GiB, killed by the harness in 6.3 s | 3.1 s in all, peak 1 GiB, then nothing: it's remembered |

The 0.4 ms at p50 is classifying a whole page's pixels (659 µs on a full grey page). The app draws tiles, at 92 µs a tile, so this is mostly a cost of whole-page runs. With retirement on, p95 rises to 10.04 ms from the respawns it causes.

**Regions.** When several tiles of a page are needed, one PDFium pass can draw them all, and the tiles are handed out of its surface, each classified by itself. Sampling the USGS map showed why it matters: its two 5104 x 6004 images aren't kept between passes, so each pass inflates the image's Flate stream again from the top, up to the rows it needs (over 80% of the time). That cost is per pass, not per pixel, so a pass per tile pays it again and again. `recto local F budget viewport --viewport 3024x1964` measures what a reader does, in the median of 5 runs, peak being the worker's:

| Page, 2x | Step | Tiles | ms per tile | ms now | Passes | Peak MiB before | Peak MiB now |
|---|---|---:|---:|---:|---:|---:|---:|
| USGS map | First view | 35 | 2,136 | 534 | 2 | 332 | 394 |
| | Scroll, a quarter window | 7 | 825 | 101 | 1 | 342 | 394 |
| | Zoom, 1.5x | 49 | 2,711 | 379 | 2 | 342 | 452 |
| NASA, images (`ntrs20240002628` p50) | First view / scroll / zoom | 18 / 3 / 28 | 66 / 9 / 43 | 68 / 9 / 43 | per tile | 64 | 64 |
| NASA, shaded backgrounds (`ntrs20220005969` p175) | First view / scroll / zoom | 18 / 3 / 28 | 513 / 4 / 689 | 514 / 4 / 689 | per tile | 176 | 173 |
| `rfc9110` p10 | First view / scroll / zoom | 18 / 3 / 28 | 17 / 0.8 / 9.8 | 17 / 0.8 / 9.6 | per tile | 8 | 8 |

Only pages with an image of 20 Mpx or more use regions: `REGION_MIN_IMAGE_PIXELS`, with its table, gives why. Below that a region costs more memory, and between 1 and 20 Mpx more time as well. 20 Mpx rests on 4 sampled pages from 2 documents above it and 17 pages just under. The owner accepted the USGS map's peak of about 415–450 MiB under the named exception to the 170 MiB budget (2026-11-07). A region is capped at 32 MiB (`MAX_REGION_BYTES`), which covers a 3024 x 1964 window and a third more, and takes more passes beyond that. It's mapped in the worker while drawing, so the memory limit and its sequence see it, and it goes down the same sequence as a tile, with the same cancelling and validation.

**What the limit does to the 28 pages above 170 MiB**, each exported whole at 2x in a fresh worker, with no heavy step: at 512 MiB all 28 draw, and the worst peaks at 396 MiB, down from 488 now that whole pages go in pieces. At 256 MiB, 17 are refused, and at 170 MiB, all 28. A kill overshoots by up to about 55 MiB on a large image's single allocation.

**The USGS map, before and after.** Whole page as tiles at 2x went from 3.75 s to 0.70 s, peak 339 to 426 MiB, and at 4x from 10.9 s to 2.95 s, peak 342 to 448 MiB. `recto render` at 2x is 0.83 s; at 4x it was stopped at the limit and is now 2.46 s, peak 468 MiB.

**The first pass's 450 ms** is about half inflating the two images' Flate streams and most of the rest stretching them (sampled at 0.25x). It's the same at any zoom, because every source row must be inflated. Nothing short of a change to PDFium or a preview cached from an earlier run avoids it.

**The renderer is hostile.** Replies go through `recto-proto`'s decoders and validators before anything uses them; any violation kills the worker. A frame's claimed length costs nothing until bytes arrive. Pixels are copied out of shared memory, never referenced in place, and shared memory can't be resized once sized on macOS, so a worker can't make the host fault. A compromised worker can still draw a wrong picture and rewrite a surface it was given earlier; that's the limit of this design until surfaces are per-job IOSurfaces.
