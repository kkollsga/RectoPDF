# Engineering

## Crate graph

A crate may depend only on crates below it in this list. `recto-platform` sits under everything and is the only crate that names an OS.

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-platform` | **Started** | File identity, process footprint, handle count, `isolate`: child process, channel with handles, shared memory, loaded library. `surface`: IOSurface, and the link that carries one to a worker. Later `fswatch` | Hold policy |
| `recto-core` | **Started** | Lazy document structure over the lopdf fork's `IndexedReader`: facts, page count, contents. Later the append writer | Know about tiles, threads or an OS |
| `recto-proto` | **Started** | Job and tile wire types, no dependencies. The host validates everything from the renderer | |
| `recto-render` | **Started** | `render_tile(page, rect, scale, &mut [u8], cancel)` behind `RenderBackend`, page text. PDFium, by raw FFI | Allocate surfaces, name an OS, decide policy |
| `recto-render-service` | **Started** | Library: the host's side, `DocRenderer`, limits, validation. Binary: the worker. Plain child process for now; sandbox is a separate spike | Link PDFium into the host |
| `recto-index` | **Started** | Line order, text normaliser and hidden-text report: pure functions on the host's side. Later the page-text corpus | Touch the UI |
| `recto-svg` | Planned | Streaming SVG writer | Build a document in memory |
| `recto-reader` | **Started** | The page view with no toolkit: where pages sit, which tiles are wanted, what's kept, what the host is told, dark paper. One thread that runs the renderer | Name a UI toolkit; wait on the UI thread |
| `recto-sign` | **Started (placeholder)** | Final seams for PAdES creation and the honest report; `PlaceholderSigner` always refuses | Depend on `recto-render`; produce anything that looks like a signature |
| `recto-session` | **Started** | Tabs, tiers, sidebar and view models: all the rules | Name a UI toolkit |
| `recto-agent` | **Started** | Typed, versioned commands, permission levels, the JSON envelope. `table.rs` is the one list of commands: the CLI's grammar, `recto help`, the level checks, `SKILL.md` and `REFERENCE.md` all come from it | Hold state of its own |
| `recto-control` | **Started** | The long-running side: the server on the local socket, the level's file, the activity log, the state folder, the thin client. A `Host` trait, so the app can carry the same socket | Name a UI toolkit; hold a rule about documents |
| `recto-ffi` | **Started** | UniFFI, pinned to what the C# generator supports (0.31); every call cheap or handed to a parked thread. Joins the app to the session and socket. Hang watchdog, the way QA commands reach the UI thread | Hold rules |
| `recto-cli` | **Started** | `recto`: a thin client of the session on the socket, plus an in-process route for tests that uses the same dispatch, levels and log | Link AppKit |
| `app/` | **Started** | AppKit host, QA channel. A Swift package, built by `app/build.sh`, ad-hoc signed | Hold rules |

## Quality bar

- `unsafe` is forbidden in every crate except the OS seams (`recto-platform`, `recto-ffi`) and the PDFium seam (`recto-render/src/pdfium.rs`). Every `unsafe` has a SAFETY comment.
- Clippy with `-D warnings`.
- No panics on untrusted input. Collections grown from outside input have a cap.
- A struct held per item documents its size and asserts it in a test.
- Dependencies count: `default-features = false`, few crates, and a line below saying why each one is here.

## Per-item sizes

| Struct | Held per | Limit |
|---|---|---|
| `TabRecord` | Every tab, including cold | ≤ 256 B plus title |
| `TocEntry` | Contents item, warm tab only | ≤ 32 B |
| `TileKey` | Tile in the cache | ≤ 16 B |
| `RenderJob`, `TileReply` | Job in a capped queue, reply in flight | ≤ 64 B |
| `CharRecord` | Character of a page in hand | 4 B |
| `DrawNote` | Recent draw, at most 256 | ≤ 32 B |
| `edit::Entry` | Step of an edited tab's undo history, at most 256 and their undone ones | ≤ 32 B |
| `Conn` | Connected client, at most 128 | ≤ 320 B plus what it's sending and being sent |
| `recto_index::order::Line` | Line of a page in hand, at most 16,384, only while it's being read | ≤ 72 B |
| `recto_reader::layout::Override` | Visited page whose size isn't the document's usual one, at most 65,536, hot and warm tabs only | ≤ 32 B |

## Dependencies

| Crate | Why |
|---|---|
| `lopdf` (fork) | The lazy `IndexedReader`, and later the append writer. Pinned to a git revision |
| `serde`, `serde_json` | The agent JSON envelope |
| `uniffi` | Swift bindings for the app, and C# later from the same definitions. Held at 0.31. `default-features = false`, so its command-line generator, with `clap` and the like, is only built behind the `bindgen` feature for `uniffi-bindgen`, and not linked into the app |
| `libc` | `proc_pid_rusage` for footprint, process, channel and shared-memory calls, and the control socket's `poll` and `getpeereid`, `recto-platform` only |
| `flate2` | Compresses `render`'s PNG. Already in the tree through lopdf, so it adds no crate. `recto-agent` only |
| PDFium (not a crate) | Draws pages. `bblanchon/pdfium-binaries` release `chromium/8125`, PDFium 158.0.8125.0, no V8, no XFA. Loaded at run time, by the worker alone. `tools/pdfium.sh` fetches it from that release's URL to `target/pdfium/` and stops unless the tarball is SHA-256 `4939bd93…139d` and the library `a45c4464…373a`; `--attest` also runs `gh attestation verify`. It links AppKit; see the sandbox note below |

**No `pdfium-render`.** The spike used it only for raw bindings, because its high-level loaders can't take a `pread` callback and `PdfDocument::handle()` is crate-private. 27 signatures in one file cost no crate; `pdfium-render` costs 24 in the tree. The worker is 0.48 MB here against the spike's 1.3 MB, which also carried JSON.

## Renderer

**The process.** One worker per document, in a plain child process on a socket pair, with shared memory for pixels and one shared word for cancelling. It inherits nothing but its channel (`POSIX_SPAWN_CLOEXEC_DEFAULT`) and an empty environment, gets the document as a descriptor, and never learns a path. No thread in the host: the call blocks its caller, which is never the UI thread, and watches while a job is in flight. A cold tab has no `DocRenderer`, so it pays for none of this.

**Limits**, all held from outside the worker, because it may be compromised:

| What | Default | Why |
|---|---|---|
| Hard memory limit | 512 MiB footprint, read every 5 ms during a job, then the sequence below | The worst real page reaches 493 MiB. macOS has no OS limit for this: `setrlimit` refuses `RLIMIT_AS`, `RLIMIT_DATA` and `RLIMIT_RSS` with `EINVAL` |
| Page deadline, with kill | 5 s | Platform §2.6. The slowest real page at 2x took 939 ms |
| Retirement | 64 MiB after a job; the worker goes before its next job for another page | Only exit returns PDFium's memory (spike §6). Tiles of one page share a worker: retiring between them cost 72 respawns and 14 s on the USGS map |
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

**Sandbox: not in place yet.** The worker has `RLIMIT_CORE`, `RLIMIT_FSIZE` and `RLIMIT_NPROC` at zero and nothing else, so a compromised worker can read what the user can. `docs/research/2026-11-07-sandbox-spike.md` has the plan, with its code in `spikes/sandbox/`: not XPC, which allows one process per app and delays a restart by 10 s, but this same child process in a nested `.app` signed with the App Sandbox entitlement, which also drops its Mach send rights once PDFium has loaded, because the sandbox alone lets it launch an app through LaunchServices. About 4 ms more per start and 0.12 MiB per worker, the same pixels, and no change to the transport or the limits. The worker takes no path to a document, and argv names only limits and the library.

**Search text.** `recto-index::normalize` turns PDFium's characters into text `rg` can search. Its rules and their limits are in that crate's header. In short: at a line-end hyphen it follows the rest of the page if the page shows a form, keeps the hyphen for capitals, one- or two-letter tails and a short closed list of compound words, and otherwise joins. It's a guess, so the other spelling of each decision goes in `also_spelled`. Lines stay lines. On bench100 it joined 12,297 and kept 1,627, with no `simulationbased` or `datadriven` left, and line hits equal to the spike's normaliser on every common word checked but one (`office`, 633 against 634), plus 4 more for `simulation-based`. A hyphen on a page number or a marker doesn't join: a number between hyphens, `-17-`, whatever follows; a real roman numeral or one letter between hyphens, `-iv-`, unless a small letter follows; or a number or one letter alone on its line, `17-`, above a capital. Without that, `wbD34442284` p. 17 read `-17-PBC`. Real tokens still join: `10-` above `year`, `X-` above `ray`, `-civil-` above `ian`, a range of numbers alone on its line, and a number in a sentence, since that's how `21-cm` breaks. Over all 5,872 pages of bench100, compared line by line with the rule switched off, this changes 47 lines, all page numbers in 4 World Bank documents.

**Line order.** A PDF can draw a line's styled words after the rest of the page, and PDFium returns drawing order, as `pdftotext -raw` does. RFC 9110 p. 110 read `A recipient ignore If-Modified-Since`, with `MUST` at the bottom of the page: a dropped `MUST NOT` turns a sentence round, and a search for the real words finds nothing. `recto_index::read_page` is the one place `text` and `search` read a page, so they can't disagree. It uses each character's box. A later run goes into an earlier line if its baseline matches and it fits a word-sized gap, or sits a word's space past either end (`Section` at a line's end, `2.` at its start). Everything else stays in drawing order: no layout sort, since the spike (§10.2) found that right for two-column text on 79 of 84 pages.

The rules that keep it from doing harm: each side of a run with a neighbour must be within 0.8 of a character height, the middle of the line's characters so one tall symbol doesn't widen it. On RFC 9110, 99% of real gaps are under 0.76, with a median of 0.53. A run on the end of a line must not start or end level with the lines drawn around it, since that's what a table's cells do down a column: `wbD34442277` p. 11's gutter is 2.9 pt beside a 3 pt word space, so width alone can't tell them apart, and it read `utilizing at least utilizing at` before this. All of a line moves or none of it, and a run doesn't pass anything drawn since that sits on or against its line, so a fraction or a superscript stays where it is in an equation. Text running backwards, upwards or on a slope is left alone, a line-end hyphen stays at its line's end, and letters touching across the end of a line aren't joined. Only whitespace is ever added or dropped.

**What's left in drawing order:** a whole line drawn late with nothing on its baseline, such as RFC 9110's code samples and page headers. And reference lines whose punctuation is drawn first: `rfc9293` p. 85 reads `, , ,` then `eid1572>.`, because the later run overlaps those gaps rather than fitting them. Mathematics mostly stays as broken up as PDFium makes it, at every raised symbol. Not tested on a real right-to-left PDF: none of the 454 under `distillPDF` has any.

`crates/recto-agent/examples/line_order_corpus.rs` runs it over a folder, against `pdftotext`. On bench100, 5,872 pages: no page gained or lost a non-space character, 2,016 pages changed and 13,840 runs moved. Scored by the share of a page's runs of 8 characters, spaces taken out, that `pdftotext -layout` also has:

| Pages | Changed | Better | Same, within 0.1 of a point | Worse |
|---|---:|---:|---:|---:|
| Standards (RFCs) | 909 | 897 | 7 | 5 |
| Everything else | 1,107 | 69 | 994 | 44 |
| All | 2,016 | 966 | 1,001 | 49 |
| All, with the looser first rule | 2,353 | 1,104 | 1,060 | 189 |

**That score isn't a verdict.** It hid the table merge above as -0.015, so pages were read too: the first lines of the diffs of 31 changed pages on this build, including all 3 changed policy pages, 5 gov and 10 math and econ, and not the 1,985 others. 2 read worse to a person, neither by a column: `wbD34442284` p. 17, `-17-PBC`, where putting the page number's parts together let the normaliser's rule for a hyphen at a line's end join it to the next line (since fixed), and `cs_CV_2607.28611` p. 20, where a chart's axis ticks became one line, `18 10 19 10 20 10`. About 6 are no better and no worse, being display equations and map symbols. The rest read better: `which converts O2 and water (H2`, `Since KAi · Ri < 0`, `<https://standards.ieee.org/ieee/802.11bh/10525/>.`. The tighter rule gave up 156 pages that the looser one had scored better, 52 of them gov: the ones looked at are mostly map scale bars and figure labels, plus a few papers' superscripts (`in S′ we express S′ as`), and none of the RFCs' gains.

**Tried and rejected:** taking short raised runs along too, which joined superscripts to their lines (`Let F′ be`), added 51 better pages and 164 worse, by rearranging display equations.

**Cost**, release, this Mac, whole process, median of 7 runs, `recto local F search zzzqqqxx`, so every page is read and none drawn. Before is the build without any of this.

| | ms a page, before | ms a page, now | `recto`'s peak before | Peak now |
|---|---:|---:|---:|---:|
| `rfc9110`, 194 pages | 1.13 | 1.37 | 3.39 MiB | 4.31 MiB |
| NASA, 383 pages | 3.43 | 3.65 | 5.88 MiB | 6.53 MiB |
| `math_AG_2607.28411`, 83 pages | 1.70 | 2.05 | 3.19 MiB | 4.19 MiB |

`read_page` itself, normalising included, is 91 µs at p50 and 409 µs at p99. The rest of the rise should be asking PDFium for each box and 16 more bytes a character on the wire, which wasn't measured apart. `text` on one page is about the same (34 ms against 34, 50 against 47, 40 against 43), because it already drew the page and read the boxes for its hidden-text check.

**A hostile page of 2,000,000 characters**, by `examples/line_order_hostile.rs` under `/usr/bin/time -l`, whole process: the page itself takes 41.3 MiB.

| Shape | Time in the repair | Peak | Runs moved |
|---|---:|---:|---:|
| A line of a million into a line of a million, each landing in a gap of the first (review's shape) | 11 ms | 49.4 MiB | 0 |
| The most lines it will look at, on one baseline | 18 ms | 51.5 MiB | 0 |
| Many later lines with runs that fit | 26 ms | 66.0 MiB | 122,880 |

A line takes at most 32 runs and gives at most 64, and past 16,384 lines a page stays as it came. Before those caps, review's shape was about 5e11 comparisons by arithmetic, some 10 minutes, and the search's 30 s cap only applies between pages.

**Encrypted files.** `facts` says when a file is encrypted and opened with no password, because `encrypted` alone reads as though its contents are protected from whoever has the file. Checked on `un_report_un_sdg2023` and an AES-128 fixture.

**Hidden text.** The spike's prototype: draw the page at 2x, mark a character with no ink under its box (range under 16, box padded by 2 px). `hidden_text.flagged` means a run of 3 or more such characters; shorter ones are thin glyphs, and stay in the count. It's a flag: every character stays in the text, under `untrusted`. In plain `text` a line holding a flagged run starts with `! ` rather than `| `, and the run is inside `[hidden: ...]`. Every line has one of those marks first, so a page can write `[hidden:` but can't print a line that's marked or unmarked other than it is. `hidden_spans` gives the same places in characters of `text`, and moving words can't help anything hide. The flag is judged in the order the file gave, so a run can't lose it by being scattered into a line's gaps, and in reading order, so pairs of hidden letters can't assemble into a word unflagged. On a line a move has touched, every character with no ink counts, however short its run. On bench100 that adds nothing: of the 2,016 changed pages, 13 are flagged in the file's order, and none only once words moved. Its limits: hidden text next to visible text passes, because the box has ink. An OCR layer over a scan is expected to pass, but this corpus has no scan to prove it on. Annotation text the page draws isn't in page text at all (spike §10.4). If the extra render stops the worker, the text comes back with `checked: false`, which never reads as clean.

**How far the flag can be believed, reviewed 2026-11-07.** `examples/hidden_corpus.rs` flags 129 of bench100's 5,872 pages, with 19,833 characters with no ink. 37 of those pages were drawn and looked at, by a box round the two longest runs of each: 7 arXiv, 8 NASA, 5 USGS, 6 World Bank and 11 UN. On all 37 the text is really not there to see: the box is blank, or off the page. **No false alarms in the sample**, and the check wasn't changed. MuPDF, as a second opinion on all 129 pages: of 19,752 no-ink characters, 10,465 are off the page, 9,081 have no ink in its drawing either, and 206 (1%) do, scattered single characters at the edges of neighbouring ink, and 99.7% of the characters the check passed have ink there. Those two renderers agreeing isn't proof that nobody could ever see these, and the runs not looked at weren't.

What made them look wrong is that **most of it is harmless, and the check can't tell.** `Am[hidden: bient Air]` on `ntrs20220005969` p. 159 looks like a chart label wrongly marked, and it's a second copy of the label. The file has `Ambient Air` twice, at x 386, drawn, and at x 415, not drawn, and pdftotext's boxes show both. The second copy's first letters overlap the first's `Air`, so they have ink, and the mark starts partway through the word. What's in the sample: copies of figure labels left outside a crop (arXiv, NASA), text off the page's edge (the World Bank's `For Official Use Only`, a NASA back cover), designers' notes (the UN's `Two different guide layers for two- and three-line titles`), and white text in a blank margin (the World Bank's `Public Disclosure Authorized`). So the words say what's known and no more: text that looks hidden, in the file, with nothing drawn where it sits, often harmless, sometimes planted, and this can't tell which.

**A missing renderer** names itself: the error says `recto-render-service` or `libpdfium.dylib`, and `next` says how to get it.

**Fetching PDFium.** Run `tools/pdfium.sh`, which `tools/check.sh` also runs. The worker finds the library beside itself or in `../pdfium/`. The worker binary is `recto-render-service`, found beside `recto`. `RECTO_PDFIUM_LIB` and `RECTO_RENDER_SERVICE` override, in debug builds only: nothing shipped loads code from a path in its environment. The digest is checked when the script fetches, not at each load; code signing covers that in the app. `cargo build -p recto-cli` alone doesn't build it: use `--workspace`.

## The app

`app/` is a Swift package with AppKit and no SwiftUI. `app/build.sh` makes `app/build/RectoPDF.app`: Rust, then the bindings, then Swift, then an ad-hoc signature, with `recto`, the worker and PDFium beside the app's executable for now (at release they move into the nested helper: `docs/research/2026-11-07-sandbox-spike.md`). `--quick` needs no Xcode. `--full` builds the app and drives it. Where a worker comes from is in one place, `Worker::start`.

**One session.** The app holds a `SessionHost`, runs the same `Server` on a thread blocked in `poll`, and wraps it in a `Host` of its own, so `recto tabs` and `recto goto` act on what's on screen. What the app does itself goes through `with_session`, as the owner's. A socket another session holds doesn't stop a normal run, and a QA run refuses. Things the reader does that aren't commands reach watchers through `Server::announcer`.

**Threads at rest, all parked or asleep:** main, `recto-control` in `poll`, `recto-session` on a condvar, `recto-pages` parked, `recto-watchdog` parked, the reaper while a worker lives, and AppKit's own. The UI thread calls Rust for three things: `set_viewport` on each scroll (a short lock and a few map lookups, at most 8 surfaces reclaimed), `drain_ops` when told there are some, and pulling a published snapshot of the sidebar. Anything that can take time goes on `recto-session`, because waking a cold tab reads its file with the session's lock held. So it can stall the session thread, and `recto status` and the UI thread both still answer. No display link: tiles arriving hop to the UI thread once each, which needs no tick at rest and works off screen.

**How a tile reaches the screen, with no copy.** The host makes the surface, so it's charged to the host and outlives the worker. A socket pair can't carry an IOSurface, since it crosses as a Mach port, so each worker is started with a send right in an exception-port slot that nothing raises, set by `posix_spawn`. It sends back a send right to a port of its own and clears the slot. There's no name in any registry, and after that the host never receives from it. A job's surface goes to that port just ahead of its frame, the worker maps it, draws, and unmaps, and the layer's `contents` is that same surface. Measured with 64 tiles: the host's footprint rises 64 MiB, the worker's by nothing, and creating, sending, looking up, filling and hearing back is 0.13 ms a tile. A copy into a surface would have been 15 µs warm a tile, plus its first touch, so this wasn't for speed: it's so there's one copy of a tile, and so a worker's limit doesn't count tiles. **In the sandbox:** the same two programs in a helper `.app` with the App Sandbox entitlement and the spike's Mach drop run correctly: rendezvous, 64 surfaces round trip, pixels verified, host charged. The drop leaves receive rights alone. The first surface takes 64 ms and adds about 4 MiB to the worker, loading IOSurface after the drop. That's the C test only, with no PDFium in it. **What this gives up:** "pixels are copied out of shared memory" no longer holds on this path. A compromised worker can keep its right and rewrite a tile that's on screen. It could already draw a wrong picture, but it can now change it later. `recto render` and everything else that writes a file keeps the shared-memory path.

**Regions** are one surface and one layer, with no cutting up. Blank tiles get no surface, only a colour, or nothing if they're the paper's colour. Grey tiles are still BGRA: `L008` would take a text page's tiles to a quarter, and it's only known correct in `render(in:)`, not on a real screen, so it waits for the MacBook spike.

**What's kept**, all in `recto_reader::tiles`: what's on a layer stays. Off a layer it goes discardable the moment the host says it's applied, which takes it out of the footprint: 64 tiles go from 66.6 to 2.7 MiB. Kept: 256 MiB and 1,024 surfaces of tiles, least recently used first, and 128 previews, which outlive their tiles. **A surface serves one job.** There is no pool: a worker can read and write a surface it was sent for as long as it lives, so reuse would be a way from one file's worker to another file's pixels. It costs 51 µs more a tile, 67 against 16 p50, nearly all the first touch of its pages. Attached: at most 8 a turn, with every visible page's preview first. A cold tab has no entries and no renderer, and a closed window has neither. The page layout costs nothing per page: pages are taken to be the size of the first one known until asked about, and only pages of another size are stored, so a file claiming 8 million pages costs nothing until it's read.

**A worker past the retirement line goes 3 s into a pause** (`BIG_WORKER_REST`), not the idle rule's 30. It's the only timer in the page view, and it's there only while such a worker is. The USGS map at rest goes from 433 MiB to 36 MiB, and the first move after that pays for the image again.

**Pinch and scroll.** Layers are page-relative in PDF points, under one transform per page, so a pinch moves one transform and a change of fit moves no tile. Tiles stretch during a pinch, and nothing's drawn until it ends. New tiles go on top of old ones, which come off when the page's visible tiles are all there. Pages are snapped to whole device pixels. `NSScrollView` does the scrolling, so rubber-banding, momentum and the scrollers are the system's. **Not checked on a screen:** a quiet run has none.

**Dark paper** is done to the tile's pixels in `recto-reader`, not by a layer filter: a filter makes the compositor take an offscreen pass every frame, and `render(in:)` leaves it out, so a shot wouldn't show it. Lightness turns over and each pixel keeps its distance from grey, so a blue link is lighter blue. Images under 40% of the page stay themselves, dimmed, from where the renderer says they are. A scan turns over like any page, and so does a page that's mostly one map: the USGS band goes mint and its black lines white. Before that threshold the map stayed itself with a hard edge at its white margins. On a low-resolution page the renderer isn't asked for its images, so that whole picture turns over too. **Its limits:** a full-page photograph comes out a negative, and vector charts turn over with the text, where the concept keeps their colours. The turn costs about 1 ms a colour tile, and text tiles use a table. The renderer isn't asked on a low-resolution page since asking might cost as much as drawing.

**Warm paper** is white paper with a tint over it. Each page has one extra layer of a single colour (`WARM_PAPER`, a named constant in `recto-session`, with its reason and a WCAG AA test) and the compositor's multiply filter, so going to it or from it draws no tile and keeps nothing per tile: a test holds to no draw. Black stays black. A multiply is the same sum everywhere, so a photo or map under it takes R×0.96, G×0.93, B×0.86, the same as the paper: a white in a photo goes cream, and no colour turns over. A shot does that sum on the bitmap, since `cacheDisplay` leaves compositing filters out. **Not checked on a screen:** the multiply itself and its cost a frame. Exports and snapshots never see any paper. With none chosen, paper goes with the window, white or dark. `recto view --paper` sets it, for the app, in the state folder.

**The contents area.** `sidebar_heights` in `recto-session` shares the sidebar's height: other tabs and the footer stay, room is kept for up to five other tabs, and a longer tab list scrolls by itself. Contents are one `NSTableView` pulling rows from Rust with `contents_rows(first, count)`, so a 10,000-entry outline holds about 29 row views.

**Reposition and rest.** Owner, 2026-11-07: "It can whoosh with smooth acceleration at both ends. It can have a \"dead-zone\" so it doesn't react to small scrolls within the window, so basically it repositions, stays still (while user is reading slowly scrolling) then repositions after the user is scrolling a bit." `ContentsFollow` in `recto-session` is the whole rule, and the host only draws where it says. Inside the dead zone, 12% to 75% of the area's height, nothing moves. Leaving it starts one move, to put the place 30% of the way down. Its shape comes from a third-order chain, a lag into a critically damped spring, so speed starts and ends at about zero with no step when the target changes on the way. It takes 0.6 to 1.1 s by distance. While a move runs it's the move's target that counts, so sections going by fast are one long move and not a row of them. The reader's own scroll stops it, right where it is, and following comes back when the pointer's out, 3 s have passed and the section next changes. With Reduce Motion it goes straight there, and only if the place has left the view. Frames come from a display link on a screen and a timer off one, started for a move and stopped when it's over, so nothing runs at rest, and passing a section costs one comparison.

**A chapter closing makes no second move.** As you leave a chapter it closes and the next opens. If its rows were above the place, the place would drop up the list by that many rows, past the dead zone, and the contents would move straight back, opposite to the move before. So Rust works out how far a change carried the place (`place_shift`, from the two lists), and the scroll takes it up in the same transaction: the place doesn't move on screen, and rows going in the view fade over in 0.15 s. Rows that change only below the place still slide. With the reader in the contents themselves it's the top of the view that stays, and rows in the view slide. **Before and after,** `rfc9110` at 6 pages a second, 10 s, by `qa sweep`: before, a move forward at each change of chapter was followed about 0.1 s later by a move back of 196 to 223 points. After, 6 moves all forward, 690 ms each, at least 0.31 s apart.

**What the moves come to,** `qa sweep`, release, this Mac with other agents' builds running, a window off every screen, so path and timing, not frames seen. Peak speed is taken over 25 ms, since steps aren't evenly spaced.

| Sweep | Moves | Each | Peak | Rests | Place in view |
|---|---|---|---|---|---|
| `rfc9110`, 1 page a second, 30 s | 5 | 690 ms, 340 points | 2,136 points/s | 3.3 s to 10 s | every step |
| `rfc9110`, 6 pages a second, 10 s | 6 | 690 ms, 310–340 points | 2,035 | 0.31 s to 1.1 s | every step |
| `rfc9110`, 40 pages a second, 4 s | 1 | carries on: 3,609 ms, 3,048 points | 3,482 | | 431 of 478 steps |
| 10,000 entries, 1 page a second, 40 s | 3 | 640 ms, 364 points | 2,262 | 12 s | every step |
| 10,000 entries, 6 pages a second, 10 s | 4 | 650 ms, 364 points | 2,587 | 1.5 s | every step |
| 10,000 entries, 40 pages a second, 6 s | 1 | carries on: 5,656 ms, 6,342 points | 2,383 | | every step |

The `rfc9110` 1-page run also has one move back of 168 points, 5 s after the one before, not one chasing it. A 10,000-entry outline holds 29 row views and the app 13.4 MiB. The UI thread's step stays at 0.17 to 0.61 ms p99. The worst single step is 4.7 ms on a chapter closing, from reloading the rows: 15.8 ms in one run under more load. How it feels can't be judged from any of this.

**Showing a shortened name.** One tracking area on the sidebar. After a short rest over a cut-off row, a label opens in place over it, past the sidebar's edge if it must, and goes on leaving. Rows don't move. A timer exists only during that short wait. Rust gives the text (`row_full_text`): cleaned of control and direction characters and held to 400. `recto qa hover` and `qa reveal` report what it would show.

**Text isn't an `NSTextField`.** On macOS 27 each `-[NSTextField layout]` makes a view, whose `-[NSKeyValueDependencyInfo initWithOwner:]` leaves records in AppKit for good. It came to about 7 KiB for each tab opened and more while reading, and took 1,000 tabs of a drawn PDF to 5.7 MiB. `TextLine` draws an attributed string instead: 0.8 KiB a tab, found with `heap` and `malloc_history`.

**Colours aren't dynamic `NSColor`s:** there's one per appearance, told again when it changes.

**The QA channel.** `recto qa start` runs the app with `--qa` on a state folder set with `RECTO_STATE_DIR`, and the app refuses anything else. It has the activation policy `.prohibited` from before AppKit does anything, so no Dock icon or menu bar and it never activates. Its window is never ordered in, sits off every screen, and draws as key at 2x. There's a 10-minute limit. `recto qa state` reports what the app says it's showing. `recto qa shot` draws the window's frame view with `cacheDisplay` into a 2x bitmap, which needs no permission. `recto qa wait-settled` goes through the session thread's queue first, then waits for what's visible to be on layers. Drive verbs call what the event calls, and answer after that's been through the session thread and laid out, so no script sleeps. `recto qa run` runs a script and gives PASS or FAIL, and a 2 s stall fails it. `recto qa sweep` reports each contents move, with its length and peak, and the rests between. `qa` rows are Read-only or Control, and a session that isn't a quiet app answers `qa_needs_app`. They're in `recto help`, and left out of the skill. **Not covered:** the step from a real event to the method it calls has no unit test, only the method. Run-loop blocks from Rust are in their own autorelease pool, as AppKit only drains its pool after an event.

**The watchdog.** The UI thread says when it wakes and sleeps, from a run-loop observer. One thread watches only while it's awake, looking every 250 ms, and notes 1 s and counts 2 s. `recto qa stall` proves it fires.

**Measured** 2026-11-07, release build, this Mac (M4, 16 GB, other agents building at the same time, load 8 to 15), quiet run: a window off every screen, so **frames reaching a display, what a visible window's backing adds, and 120 Hz aren't in any of this**. `recto qa budget --file F --heavy H`, whole tree unless it says.

| Budget (`SPEC.md` §2) | Target | Measured | |
|---|---|---|---|
| Cold start to first sharp page, `rfc9110`, process start to settled, 21 runs | ≤ 300 / ≤ 500 ms | **134 / 154 ms** p50 / p95. 82 ms of it is the window | Met |
| One document, text page | ≤ 60 MiB | **32.5 MiB**: app 24.5, worker 8 | Met |
| Colour page: NASA `ntrs20240002628` p50, images | ≤ 100 MiB | **37.7 MiB** | Met |
| The same: NASA `ntrs20220005969` p175, shaded backgrounds | ≤ 100 MiB | **210 MiB**: the worker's 184, PDFium's transparency buffers, as already measured in `budget viewport`. It goes after 3 s at rest | **Missed** |
| Scrolling, peak: `rfc9110` at 6 / 40 pages a second | ≤ 170 MiB | **40 / 48 MiB** | Met |
| The same: NASA images | ≤ 170 MiB | **98 / 93 MiB**, up to 117 in other runs. 61 / 90 on a quieter machine before | Met |
| UI thread's work per scroll step, 120 steps a second | ≤ 2 ms p99 | **0.50 / 0.31 ms** p99, worst 0.75. 0.21 / 0.24 on a quieter machine before | Met, off screen |
| Blank on a seen page, during those sweeps | 0 | `rfc9110` **0**. NASA **0** at 6 a second, **14 to 179 steps** at 40 | **Missed at 40 a second past 128 pages.** 128 previews are kept, so a page 128 pages back has lost its own. Replayed: blanks fall only on the pages seen earlier, and only after that many. The first measure of 0 was before every page got a preview. Disk previews are what fixes it |
| Sharp after scroll stops, on pages not drawn yet | ≤ 100 ms | **18 / 21 ms** p50 / worst, one client round trip included | Met |
| Idle CPU, one document, 20 s, app only | ~0%, ≤ 1 wake-up/s | **0.0%, 0.35 to 0.65** | Met, off screen. A worker parked in `poll` wasn't counted |
| Window closed | ≤ 20 MiB | **11.1 MiB**, no worker | Met |
| 1,000 tabs of a PDF, over 2 tabs, app's own | ≤ 4 MiB | USGS map **0.75**, NASA **1.19**, 2 GiB file **0.06**. Each was drawn when opened | Met |
| Sidebar opens, 1,000 tabs: focus off to settled | < 150 ms | **2.7 to 6.2 ms** | Met |
| Handles at 2 tabs and 1,000 | same | 22 to 25, varying with what's warm. 3 documents held at most | Met |
| 2 GiB, 10,000 pages, page 1 sharp, warm | ≤ 250 ms | **182 ms** p50, from process start | Met |
| The same, cold | ≤ 600 ms | **352 ms** worst of 11, its first run after writing the file. Not a true cold start: purging the file cache needs root | Not tested as such |
| Main never stalls over 16 ms on that file | ≤ 16 ms | **20 ms** at worst over the whole run, which is one stretch of work in launch or layout, not a page. 20 ms with `rfc9110`, 23 with NASA | **Missed by 4 ms** |
| USGS map, peak, under the named exception | 415–450 MiB accepted | Fitted **377 MiB**. Zoom 3, about 2x: **434**. Zoom 4.5, about 3x: **490**, worker 454 and app 36. Before, the worker alone at 2x and 3x peaked at 394 and 452 | **Over at 3x** by the tree, as the surfaces were in the worker's number before |
| USGS map, first view, scroll, zoom | No target | **700 ms** from process start. Zoom to 2x 575, a quarter window 98, 1.5x more 709. Before, by `budget viewport`: 534, 101, 379 | The first and last draw at higher scales than before |
| 120 Hz, ≥ 99% of frames on time | | **Not tested**: needs the MacBook and a real screen | |

**Before and after on the app's own fixes,** same runs. `TextLine` for `NSTextField`: 1,000 tabs of NASA 5.68 to 2.48 MiB over 2 tabs, and a 400-tab run's heap growth from 2.9 MiB to 0.1. Letting a big worker go: USGS map at rest 433 to 36 MiB. Previews outliving tiles: 84 blank steps on seen pages at 40 pages a second, in a debug run, to 0.

**About the off-screen numbers.** A real window has a backing store and a window-server share that this doesn't, so treat the memory figures as a floor until the MacBook spike. The platform report's sRGB window alone cost 17.6 MiB in a visible state.

## The agent interface

`recto` is the interface every test and agent uses from here on, and the Mac app and agent sidebar sit on it. It's three layers: the command table and dispatch (`recto-agent`), the session on a socket (`recto-control`), and a thin client (`recto-cli`).

**One table.** `TABLE` in `crates/recto-agent/src/table.rs` has a row per command: its level, its arguments, what `data` holds, what it tells watchers, and its examples. The CLI's parser reads the row, so a flag can't exist without being documented, and `SKILL.md` and `REFERENCE.md` are generated from it. Tests walk it: every command has a row, every row runs, every example parses, every argument reaches its command, and every row is enforced at every level over the real socket.

**The server: one thread, blocked in `poll`.** It owns every connection, so a connection costs a small struct, and a client in `wait` or `watch` costs that and nothing else. There's no thread or task per client, per tab or per wait, and no timer: `poll` is given the next real deadline, or none, in which case it sleeps until a socket has something. Work that can take time runs on a short-lived thread per job, capped at 8, of which at most 2 may draw or read pages, since each of those can use hundreds of megabytes. A job tells the loop it's done through a socket pair. With nothing happening there's one thread, asleep. `status` is answered on the loop's own thread and takes no lock, so it answers even while a slow file holds the session.

**Why no async runtime.** A runtime would have given this exactly one thing, multiplexing, in exchange for a large dependency and a thread pool, and the multiplexing is 40 lines of `poll` in `recto-platform`. The real work here isn't async anyway: PDFium's worker, the lazy reader and file reads all block, so they'd need a blocking pool either way. `poll` over `kqueue` because the connection cap is 128, where `poll`'s cost per call doesn't matter, and it's the one call with the same meaning everywhere. **Measured** below: an idle server does nothing at all.

**The session lock is held per step.** `execute` takes the session through `SessionAccess`, only as long as each step needs. `open` and `page insert` read the file with no lock held, then take it to make the change, so a slow or hostile file holds nobody else up. **What isn't solved yet:** switching to a cold tab reads its file with the lock held, as `Session::tap` and `close` do, so a file on a dead volume could hold the session until the volume answers. `status` still answers, and every client gives up with exit 7 rather than hanging.

**Hostile input.** A line is capped at 64 KiB, a selector at 1 KiB, and half a line has 5 s. Commands parse with `deny_unknown_fields`, so a mistyped or added field is refused. A silent connection goes after 60 s, a client that doesn't read its answer after 30 s. A client past 1 MiB unread has its next command left unread until it catches up, so nobody works for answers nobody's taking. An answer is capped at 32 MiB. All unread answers together are capped at 128 MiB for large ones, and at 144 MiB for anything: small answers can go past the first, so `status` still answers, and whoever would take it past the second is dropped. A watcher that stops reading is dropped past 1 MiB of backlog, and one that writes anything at all goes at once, since it could otherwise hold the only working thread. A panic in a job is an error for that one client. There's no rate limit beyond these caps yet.

**Ending a watch.** `recto watch stop` ends every watch on this session, at the level `watch` needs, and logs how many. An agent's `pkill` was refused, and its background watch stayed for its whole 10 minutes. It's for one agent to clear up after itself, and it can end another's, which only has to start again: a watch holds nothing that could be lost.

**What must always get through.** At most 128 connections, of which at most 64 may be in `wait`, `watch` or a waiting `permission request`. Past 128, up to 16 more may connect for `status`, `stop`, `permission show` and the message after `permission set`, and nothing else: anything else gets `busy`, exit 10, and each new one pushes out the oldest of these. A new connection reads the level before anything else, so a lowered level ends waits and watches even with every slot full, and a wait's look reads it again.

**`stop`.** It closes the door to anything but quick answers, refuses what's still queued, waits up to 5 s for what's running, and only then looks for unsaved or undone edits. An edit can't slip past that check. If something stays stuck, it says so and the session carries on.

**Out of descriptors.** On macOS, an `accept` that fails for want of descriptors drops the waiting connection (measured: 20 connections at a limit of 12, 8 accepted, 12 failed once each, then nothing was ready even after freeing some). So the listener doesn't stay ready, and there's nothing to spin on: those clients see their connection end. `out_of_descriptors_the_session_doesnt_spin` holds that. It would need a pause on an OS that behaves otherwise, and Windows is a different seam anyway.

**Local only.** The socket is mode 0600 in a 0700 folder that's checked, never repaired: a link, someone else's folder or an open one is refused. The peer's user is checked on every connection, and the client checks the socket and its folder the same way. There's no TCP listener, and a test holds the source and the running process to that. A client that's gone before it's accepted can't pass the peer check, so what it sent is dropped.

**Starting and stopping.** A server takes an `flock` on `run/server.lock`, held until it ends however it ends, so a second start is refused, and a socket file found while holding the lock is known to be left over. Only a socket of ours is removed. Clients take a socket nobody's listening on as "not running", exit 6. `recto serve --background` starts another copy of itself and blocks on its ready line. A session's tabs and unsaved edits end with it, and a new one starts empty.

**The app later.** `Server::bind` and `Server::run` take a `Host`, and `Stopper` ends it from another thread. The Mac app runs the same server on a background thread with a host that posts to its own session, so `recto` works the same against either, and it can show `permission request` as a prompt rather than a line on stderr. `SO_NOSIGPIPE` is set on every socket so a host needn't have ignored `SIGPIPE`.

**Asking for help.** `recto --help`, `-h`, `recto help`, `recto help page`, `recto page --help` and `recto help "page move"` all work, the last because the help's own list prints names that way. `recto --version` gives the version. `--flag=value` means `--flag value`. **A near miss gets the closest form that works, driven by the table, on every row: the whole line, mended.** An unknown flag becomes one named or spelled nearly that, or the place a value goes that has no flag, and a stray word gets its flag or the tab it could be. Every other word stays where it was, so `page delete --pages 3 --tab report` advises `recto page delete 3 --tab report`, and not a line that deletes page 3 of the active tab. More than one wrong word is mended in turn (`page rotate --page 1 --degrees 90` advises `recto page rotate 1 --by 90`). Each mend goes through the real reading of a line, and if it can't all be mended, as past `--`, or with a page given twice, no whole line is offered, only the flag and what it takes. A test walks every flag and value of every row, mistyped with all the others present, and holds the advice to the same fields as the right line. Every word is quoted so a shell passes it on unchanged, since a tab's name can come from a file, and one that can't be printed safely isn't printed. Every refusal also gives the command's usage line. `text` and `render` also take a bare page number (`recto text 60`), since three agents in three rounds tried it. A range stays refused, and says it's one page at a time. A mistyped command lists the nearest rows first, by spelling and then by the words of their summaries, so `recto doc` offers `facts`. A row that isn't built says so in `help`, the skill and `not_built`, in place of a level that reads as though it works.

**Levels.** `Off`, `Read-only`, `Control`, `Full`, default `Off`. Each command's level is in its row, read fresh from the file for every command, so lowering applies at once on connections already open, and to waits and watches, which it ends. `status` says how many PDFs are open at every level, Off included, so an agent can tell whether asking for access is worthwhile. A count isn't document content and holds no name, so this doesn't touch the rule that nothing from a document reaches Off. It fails closed: a missing, malformed, linked or open file is `Off`. Nothing over the socket can raise it: no command writes the file, and `level_changed` carries no level. No variable or flag does either.

**How the user sets it, in the headless phase.** `recto permission set <level>`. Lowering needs nothing. Raising needs stdin and `/dev/tty` to be a real terminal, and a word typed back that's different every time, read from the terminal. **The app's settings take this over later,** through the same `levels::store`. A refusal's `ask` is `recto permission request <level>`, which grants nothing. It answers at once, and tells the agent what to tell the user and to carry on with what it can. With `--timeout` it blocks until they've answered, and its help says that's only for when someone's at the keyboard: an agent in the blind test blocked for 2 minutes and then 5. It writes a line for the user only for a level higher than it's said since the level last changed, so asking in a loop can't fill that file, and a write that fails can't end the session.

**`recto skill install`** takes the same check (coordinator's decision, 2026-11-07): it writes into the user's agent's own files, and `--replace` keeps no backup. `recto skill show` stays open at Off, so an agent can read it. With a test state folder it won't write into the real home, going by where the path really leads.

**Below Full, an agent can throw away only agents' edits, never the owner's** (SPEC decision 20). Each step on a tab's undo and redo lists records its `Author`, and keeps it through undo and redo. Everything through `recto_agent::execute` is `Agent`. `Session::edit_pages` and `insert_pdf` are the app's own and are `Owner`, and `SessionHost::with_session` is how the app reaches them. `PageEdits` starts as the owner's and only the session changes that, so a path that forgets to say gets the more protected answer. It costs 8 bytes a step, nothing per page and nothing on a tab with no edits: at most about 4 KiB of a tab's history, on the 256 steps it can hold, only while it has edits.

At Control: an edit that clears undone steps, `discard` and `close` with undone steps go ahead if all that would be lost is agents'. If any of it is the owner's, it's exit 5 (`would_clear_redo`, or `would_discard_user_edits` for `discard`), the message says they're the user's edits, and `recto redo` or asking for Full are the ways on. Undoing the owner's steps is allowed, since `redo` brings them back. It doesn't hide them: `user_edits_undone` and `saved: false` say so in every answer, the sidebar model has `undone_owner_steps` so the app can show it, and `Session::close` itself refuses until they're redone or discarded, whoever undid them. So the app's own close needs `discard_edits` once it's been told "Don't Save". A tab with unsaved edits doesn't close at any level, so closing never loses edits silently: `discard` first, then `close`. A refused `close` names all it would lose and each way on that's real. `stop` still counts undone steps as unsaved, whoever's they are. At Full, what's lost is the holder's call.

**Edited means it differs from the file.** A step that does nothing isn't a step: `page move 3 --to 3`, or a whole turn. And four quarter turns, or a page moved and moved back, read as not edited, with their steps still there to go back through. It's a look along the list after each step, stopping at the first page out of place, and asking is free. **Measured** on a release build, this Mac, at the worst place, rotating the last page of an otherwise untouched document, over 2,000 edits: 80 µs at p50 and 91 µs at most at 200,000 pages, and 0.5 µs at 1,000. Reading a tab's state costs nothing more, and an edited tab's list holds 16 bytes more, with nothing per page.

**The undo cap.** When a step falls off the capped undo list its effect stays in the page order, so the cap hasn't erased it, and `is_edited` keeps saying the tab differs from its file. But `discard` would throw that effect away, so an owner's step that fell off still counts as the owner's (`owner_step_dropped`) and `discard` below Full stays refused. A redo step dropped by the byte cap is gone for good, and an agent could otherwise cause that by undoing, so the trim stops at an owner's step, and at any under it, because a redo step only applies to the state just before it. That can't take the list far over: what redo holds is the pages of undone inserts, which a document's length and the cap bound to `MAX_HISTORY_BYTES` plus one document, and `undoing_never_drops_an_owners_undone_step` holds it there. **Limit:** an agent can still delete a page the owner rotated, then make 256 more edits so the delete can't be undone. Nothing was thrown away by `discard` or the like, and the rest of the owner's edits stay, but that one is out of reach until the session ends. A cap on the agent's own steps would trap ordinary work, so this is stated rather than closed.

**What this does and doesn't protect against** (agent report, threat T16). It shuts every ordinary route: an agent running commands through a tool has a pipe, not a terminal. It can't stop a process of the same user that goes to the length of faking a terminal or writing the file itself: that process could read the PDFs directly anyway. A variable can point a run at a fresh state folder, which starts at Off, and writing a file into it gives access only to the throwaway session there, not the user's. The app's settings, behind a human gesture, are the stronger answer.

**Signing** is out of reach at every level. `recto sign` always refuses, and nothing in the control path links a signer. `recto signatures` reports facts through `recto_core::signatures`, a bounded walk of the form fields, with `recto-sign`'s wording: "Signature present, not checked", never "valid". Stated names and times go under `untrusted`, because they're only what the file claims.

**State folder.** `~/Library/Application Support/RectoPDF/agent`, under the real home from the user database, not `$HOME`. `RECTO_STATE_DIR` overrides it for tests and QA, and refuses the real home, anything holding it, and anything inside it unless that's in the OS's temp folder (from `confstr`, not `$TMPDIR`), through links too. If the path is too long for a socket (the kernel's limit is 103 bytes), the socket goes in `/tmp/recto-<uid>/`, checked the same way and named for the state folder.

**Activity log.** One line per action with its time, command, level and outcome, refusals included. It holds numbers, and cleaned file names under `untrusted`, and never document text: `Command::log_details` is the one place that decides, and a tab's name as typed and a search's words aren't kept. `tab` is the `#id` the command really acted on, found before it runs because `close` leaves nothing to ask afterwards. If it found no one tab, as with an ambiguous name, it says `by name`. A refusal for the level is counted rather than written out, so it keeps no details at all. At a real terminal `recto log` reads the files with no session or level, for the user. Otherwise it needs Read-only, even with no session.

An agent mustn't be able to erase its own record, so there are two files, 1 MiB each and rotated once. `activity.jsonl` has what changed something: `open`, `close`, page edits, `render`, `stop` and `permission set`, when they worked. `activity-other.jsonl` has the rest, so no flood of reading, refusals or other failures can reach the first. `status`, bad lines, asks and refusals are written once, then as one line with a count, at most once a minute each. **Limit:** an agent at Control can still push out its own changes with enough of them, since each one is an action to record.

**Contents.** A first look at an outline needs its sections, and a root can hold only the title, as on RFC 9110, where `--depth 0` gives `RFC 9110` and `HTTP Semantics` and nothing else. So `sections_at` is the first depth whose entries are on more than one page. With no `--depth`, `contents` shows down to the deepest level that fits in 200 entries, never above the sections, and `depth_rule` says which that is, which entries are the top-level sections, and what to ask for instead, ahead of the list in plain output. An agent took two calls to work out what "top-level" meant. On RFC 9110 that's depth 2 of 4, 154 of 311 entries, in one call. Paging keeps to the same entries: the depth is decided from a fixed room of 200, not from `--limit`.

**A file only the renderer can read.** lopdf refuses some damaged files PDFium repairs, or finds no pages in them, and commands go through `session.open`. So `DrawableLoader` asks PDFium, in its worker, when lopdf says "not a PDF" or "no pages": the tab opens with PDFium's page count and `renderer_only` set. Facts stay trustworthy because nothing is made up. `facts` says `structure: renderer_only`, `contents` says `available: false`, and `signatures` says `structure_read: false`, each meaning "couldn't be read" and never "none". Editing it, or inserting it into another document, is refused, since only the structure reader's writer may change a PDF. A password-protected, missing or forbidden file stays that way: only those two answers get a second opinion.

**Search for now.** `recto search` reads each page's text through one worker at a time, in order, ignoring case and line breaks, with no index: at most 200 hits and 30 s, stopping only between pages so `next_page` always moves on. Snippets are built only for hits that will be shown, and the count stays whole. It's slow on a long document, and the Phase 3 corpus goes behind the same command.

**Added files in search.** Every hit says `from_added_file` and `source_page`, whatever they are, and one from an inserted PDF also says `added_file` under `untrusted`. Plain output says so only when it isn't the number shown. Plain output also opens with what it covered: hits shown, pages really read of how many, how many couldn't be read or had no text, and whether there's more. `pages_searched` counts only pages read, and "Nothing found" isn't said if any page couldn't be. An agent read past "stopped early" at the bottom. That stays at the bottom too, with where to go on from.

**Hidden text in search:** only a page with a hit is drawn to check it. Drawing every page costs too much for what it adds, since a page with no hit is never read: rfc9110, 194 pages, went from 1.14 to 3.2 ms a page, and NASA's 383 pages from 3.5 to 16 ms a page (release, this Mac, 3 runs each, `recto local F search zzzqqqxx`, with the check switched on in code for the second). Each hit says `hidden_text: { checked, flagged }` and `page_has_tag_chars`. `overlaps_hidden` says whether its own words include hidden text, so a page's flag doesn't fall equally on the innocent hit beside them: `true`, `false`, or `null` if that can't be said. That needs nothing more than the draw a hit already pays for.

**Characters past 16 bits.** A font's ToUnicode gives these as two halves, and PDFium reports each as a character. `recto-render` joins them, or an emoji is lost and a tag character, the plainest hidden instruction there is, never raises its flag.

**Files that change.** A tab, and each PDF added to it, keeps the size and modification time its page numbers were recorded from. If an added file, or an edited tab's own file, has changed or gone, `text`, `render`, `contents` and `signatures` refuse with `source_changed`, and `search` lists those pages as failed and names the file. An unedited tab has no numbers to get wrong, and reads the file as it now is.

**Edits and pages.** Every edit's answer starts by saying what it did (`did`), in plain output too: "Rotated page 34 by 270 degrees. 34 pages now, 34 in the file." `tabs` and `where` give each tab's `pages` with edits and its `file_pages`. A tab's place follows the page it's reading, and if that page is removed it lands on the nearest and says so once. A draw's note goes when the page order changes. Page numbers on every command mean the document as edited: `text` reads the page now in that place, from whichever file it came from, and says `source_page` and `from_added_file`. `render` doesn't show an unsaved rotation, and says so in `unsaved_rotation_degrees`. Deleting a page is `Control` rather than the report's `Full`: nothing reaches a file, and it can be undone. The two-step plan and confirm belong to `save`, when that exists.

**Stack depth in the session's own process.** The structure reader runs in the session, where an overflow would abort it, unsaved edits and all, and `catch_unwind` can't help. lopdf's fork already stops at 100 levels of nesting, 4 MiB an object and 256 levels of page tree, and our own walks keep explicit stacks. Arrays and dictionaries nested 1,000,000 deep, page trees, outlines, field trees and name trees 500,000 deep, object streams, and chains of 100,000 `/Length` references and `/Prev` sections all returned on a release build, through `facts`, `contents` and `signatures`. `crates/recto-core/tests/deep.rs` keeps these at 100,000 on a test thread's 2 MiB in debug. **For the fork, not changed here:** a `/Prev` chain has no cap, and 100,000 sections in an 8 MiB file take about 4 s to open, in proportion, with no lock held and no thread lost except a job's. `IndexedReaderOptions` could take a maximum number of sections.

**Measured** 2026-11-07, release build, this Mac, on the 330 MiB `gov_usgs_usgs70277338.pdf`.

| | Before any session: one-shot in-process | First socket build | Now | Budget |
|---|---:|---:|---:|---:|
| One answer, whole process start to exit, 200 runs, p50 / p95 / max | 4.09 / 4.62 / 4.90 ms (`echo '{"cmd":"tabs"}' \| recto session`) | 4.66 / 6.34 / 7.83 ms | 3.35 / 5.98 / 7.35 ms (`recto status --json`) | ≤ 30 ms |
| The same, with 1,000 tabs open | | 5.44 / 6.17 / 6.55 ms | 4.94 / 6.28 / 7.19 ms | |
| Footprint, idle | | 1.66 MiB | 1.67 MiB | |
| Footprint, 1,000 tabs of the 330 MiB PDF (`recto budget tabs --file F --count 1000`) | 2.58 MiB, from 1.22 MiB | 2.91 MiB, from 1.67 MiB | 3.05 MiB, from 1.69 MiB | ≤ 4 KiB a tab |
| Thread count, idle or with 1,000 tabs | | 1 | 1 | |
| Over 30 s idle, by `top`: CPU, idle wake-ups, context switches | | 0.0%, 0, 0 | 0.0%, 0, 0 | |
| One open of that PDF, over one connection | 5.10 ms | 1.57 ms | 1.50 ms | |
| One open through the real client, process start included | | 7.22 ms | 6.60 ms | |

**Re-measured after `status` began giving its count at Off,** 2026-11-07: `recto status --json`, whole process, 200 runs, two runs each of this build and the one before it, in turn, on one Mac. The count was already read from one atomic, so no difference is expected, and none shows beyond the runs' own spread: at p50 / p95 / max, idle 3.19 / 3.49 / 6.18 and 3.19 / 3.45 / 3.91 ms against 3.20 / 3.75 / 6.70 and 3.21 / 3.40 / 4.22 before, and with 1,000 tabs 3.23 / 4.95 / 7.67 and 3.34 / 6.05 / 9.32 ms against 3.21 / 5.30 / 7.76 and 3.25 / 4.51 / 7.46 before. The session's footprint with those 1,000 tabs was 2.97 and 3.02 MiB, against 2.97 and 2.89 before, after opening them over one connection.

The two per-open numbers for that PDF aren't comparing like with like: the earliest run came first, on a colder file cache. `recto-cli`'s `tests/memory.rs` holds the idle and per-tab properties, in its own binary. **What this isn't:** a soak, or tabs that have been read from. Drawing is what `recto budget render` measures, in the renderer section.

**Descriptors**, by `lsof` on the release build. A tab holds none: hot, warm or cold makes no difference, and the count is the same with 1 tab or 1,000.

| | Server | Worker |
|---|---:|---:|
| Idle, nothing logged yet | 8: stdin, stdout, stderr, lock, a log, listener, wake pair | none |
| With tabs of any tier | 10: the second log file, once it's been written | none |
| During a job that draws or reads a page | 17: adds the worker's channel, shared memory, the document, a kqueue | 8: stdin, stdout, stderr to `/dev/null`, its channel, the document, shared memory, the system's font folder, one more that PDFium holds |
| After the job | 10 | gone |

## Machine notes

Lints and profiles are set once in the root `Cargo.toml`. `tools/check.sh` is the one local check, and takes the shared heavy-job lock.
