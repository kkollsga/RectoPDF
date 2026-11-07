# Engineering

## Crate graph

A crate may depend only on crates below it in this list. `recto-platform` sits under everything and is the only crate that names an OS.

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-platform` | **Started** | File identity, process footprint, handle count, `isolate`: child process, channel with handles, shared memory, loaded library. Later `surface`, `fswatch` | Hold policy |
| `recto-core` | **Started** | Lazy document structure over the lopdf fork's `IndexedReader`: facts, page count, contents. Later the append writer | Know about tiles, threads or an OS |
| `recto-proto` | **Started** | Job and tile wire types, no dependencies. The host validates everything from the renderer | |
| `recto-render` | **Started** | `render_tile(page, rect, scale, &mut [u8], cancel)` behind `RenderBackend`, page text. PDFium, by raw FFI | Allocate surfaces, name an OS, decide policy |
| `recto-render-service` | **Started** | Library: the host's side, `DocRenderer`, limits, validation. Binary: the worker. Plain child process for now; sandbox is a separate spike | Link PDFium into the host |
| `recto-index` | **Started** | Line order, text normaliser and hidden-text report: pure functions on the host's side. Later the page-text corpus | Touch the UI |
| `recto-svg` | Planned | Streaming SVG writer | Build a document in memory |
| `recto-sign` | **Started (placeholder)** | Final seams for PAdES creation and the honest report; `PlaceholderSigner` always refuses | Depend on `recto-render`; produce anything that looks like a signature |
| `recto-session` | **Started** | Tabs, tiers, sidebar and view models: all the rules | Name a UI toolkit |
| `recto-agent` | **Started** | Typed, versioned commands, permission levels, the JSON envelope. `table.rs` is the one list of commands: the CLI's grammar, `recto help`, the level checks, `SKILL.md` and `REFERENCE.md` all come from it | Hold state of its own |
| `recto-control` | **Started** | The long-running side: the server on the local socket, the level's file, the activity log, the state folder, the thin client. A `Host` trait, so the app can carry the same socket | Name a UI toolkit; hold a rule about documents |
| `recto-ffi` | Planned | UniFFI, pinned to what the C# generator supports (0.31 in Seedwell); every call async or bounded | Hold rules |
| `recto-cli` | **Started** | `recto`: a thin client of the session on the socket, plus an in-process route for tests that uses the same dispatch, levels and log | Link AppKit |
| `app/` | Planned | AppKit host, QA channel | Hold rules |

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

## Dependencies

| Crate | Why |
|---|---|
| `lopdf` (fork) | The lazy `IndexedReader`, and later the append writer. Pinned to a git revision |
| `serde`, `serde_json` | The agent JSON envelope |
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

What's learned holds at that scale and above, since a higher zoom costs more, and a zoom under 70% of it gets its own try (`page_draw_state_at`). Failures clear by themselves in those ways, and when the tab closes; `forget_failures` is for the reader's "try again". To stop a file of many bad pages: at most 8 heavy retries per document, then pages go straight to step 3, and at most 128 kills, then `DocumentStopped`, until asked. A cancel between steps ends the sequence, taking no slot and starting no worker.

**Too large at this zoom** names the largest scale that fits, from the page's size and the caps (`largest_scale_that_fits`), to two decimal places, so an agent doesn't try 8, 4, 2 and 1 by hand. It's a statement about size alone: a heavy page can still be refused for memory at that scale, in words of its own.

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

**Sandbox: not in place yet.** The worker has `RLIMIT_CORE`, `RLIMIT_FSIZE` and `RLIMIT_NPROC` at zero and nothing else, so a compromised worker can read what the user can. The XPC sandbox is a separate spike, whose known problem is that `libpdfium.dylib` links AppKit. Everything crosses as frames and handles so XPC can replace the transport: the worker takes no path to a document, and argv names only limits and the library.

**Search text.** `recto-index::normalize` turns PDFium's characters into text `rg` can search. Its rules and their limits are in that crate's header. In short: at a line-end hyphen it follows the rest of the page if the page shows a form, keeps the hyphen for capitals, one- or two-letter tails and a short closed list of compound words, and otherwise joins. It's a guess, so the other spelling of each decision goes in `also_spelled`. Lines stay lines. On bench100 it joined 12,297 and kept 1,627, with no `simulationbased` or `datadriven` left, and line hits equal to the spike's normaliser on every common word checked but one (`office`, 633 against 634), plus 4 more for `simulation-based`.

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

**That score isn't a verdict.** It hid the table merge above as -0.015, so pages were read too: the first lines of the diffs of 31 changed pages on this build, including all 3 changed policy pages, 5 gov and 10 math and econ, and not the 1,985 others. 2 read worse to a person, neither by a column: `wbD34442284` p. 17, `-17-PBC`, where putting the page number's parts together let the normaliser's rule for a hyphen at a line's end join it to the next line, and `cs_CV_2607.28611` p. 20, where a chart's axis ticks became one line, `18 10 19 10 20 10`. About 6 are no better and no worse, being display equations and map symbols. The rest read better: `which converts O2 and water (H2`, `Since KAi · Ri < 0`, `<https://standards.ieee.org/ieee/802.11bh/10525/>.`. The tighter rule gave up 156 pages that the looser one had scored better, 52 of them gov: the ones looked at are mostly map scale bars and figure labels, plus a few papers' superscripts (`in S′ we express S′ as`), and none of the RFCs' gains.

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

**Hidden text.** The spike's prototype: draw the page at 2x, mark a character with no ink under its box (range under 16, box padded by 2 px). `hidden_text.flagged` means a run of 3 or more such characters; shorter ones are thin glyphs, and stay in the count. On bench100 that flags 126 of 5,689 pages, and 19,732 of 16.4 M characters have no ink; neither has been reviewed for false positives. It's a flag: every character stays in the text, under `untrusted`. In plain `text` a line holding a flagged run starts with `! ` rather than `| `, and the run is inside `[hidden: ...]`. Every line has one of those marks first, so a page can write `[hidden:` but can't print a line that's marked or unmarked other than it is. `hidden_spans` gives the same places in characters of `text`, and moving words can't help anything hide. The flag is judged in the order the file gave, so a run can't lose it by being scattered into a line's gaps, and in reading order, so pairs of hidden letters can't assemble into a word unflagged. On a line a move has touched, every character with no ink counts, however short its run. On bench100 that adds nothing: of the 2,016 changed pages, 13 are flagged in the file's order, and none only once words moved. **Marking made a weakness of the check visible:** on chart and map labels it can mark part of a word, as in `Am[hidden: bient Air]` on `ntrs20220005969` p. 159, whose text is plain on the page. Those were flagged before, and the check still hasn't been reviewed for false positives. Its limits: hidden text next to visible text passes, because the box has ink. An OCR layer over a scan is expected to pass, but this corpus has no scan to prove it on. Annotation text the page draws isn't in page text at all (spike §10.4). If the extra render stops the worker, the text comes back with `checked: false`, which never reads as clean.

**A missing renderer** names itself: the error says `recto-render-service` or `libpdfium.dylib`, and `next` says how to get it.

**Fetching PDFium.** Run `tools/pdfium.sh`, which `tools/check.sh` also runs. The worker finds the library beside itself or in `../pdfium/`. The worker binary is `recto-render-service`, found beside `recto`. `RECTO_PDFIUM_LIB` and `RECTO_RENDER_SERVICE` override, in debug builds only: nothing shipped loads code from a path in its environment. The digest is checked when the script fetches, not at each load; code signing covers that in the app. `cargo build -p recto-cli` alone doesn't build it: use `--workspace`.

## The agent interface

`recto` is the interface every test and agent uses from here on, and the Mac app and agent sidebar sit on it. It's three layers: the command table and dispatch (`recto-agent`), the session on a socket (`recto-control`), and a thin client (`recto-cli`).

**One table.** `TABLE` in `crates/recto-agent/src/table.rs` has a row per command: its level, its arguments, what `data` holds, what it tells watchers, and its examples. The CLI's parser reads the row, so a flag can't exist without being documented, and `SKILL.md` and `REFERENCE.md` are generated from it. Tests walk it: every command has a row, every row runs, every example parses, every argument reaches its command, and every row is enforced at every level over the real socket.

**The server: one thread, blocked in `poll`.** It owns every connection, so a connection costs a small struct, and a client in `wait` or `watch` costs that and nothing else. There's no thread or task per client, per tab or per wait, and no timer: `poll` is given the next real deadline, or none, in which case it sleeps until a socket has something. Work that can take time runs on a short-lived thread per job, capped at 8, of which at most 2 may draw or read pages, since each of those can use hundreds of megabytes. A job tells the loop it's done through a socket pair. With nothing happening there's one thread, asleep. `status` is answered on the loop's own thread and takes no lock, so it answers even while a slow file holds the session.

**Why no async runtime.** A runtime would have given this exactly one thing, multiplexing, in exchange for a large dependency and a thread pool, and the multiplexing is 40 lines of `poll` in `recto-platform`. The real work here isn't async anyway: PDFium's worker, the lazy reader and file reads all block, so they'd need a blocking pool either way. `poll` over `kqueue` because the connection cap is 128, where `poll`'s cost per call doesn't matter, and it's the one call with the same meaning everywhere. **Measured** below: an idle server does nothing at all.

**The session lock is held per step.** `execute` takes the session through `SessionAccess`, only as long as each step needs. `open` and `page insert` read the file with no lock held, then take it to make the change, so a slow or hostile file holds nobody else up. **What isn't solved yet:** switching to a cold tab reads its file with the lock held, as `Session::tap` and `close` do, so a file on a dead volume could hold the session until the volume answers. `status` still answers, and every client gives up with exit 7 rather than hanging.

**Hostile input.** A line is capped at 64 KiB, a selector at 1 KiB, and half a line has 5 s. Commands parse with `deny_unknown_fields`, so a mistyped or added field is refused. A silent connection goes after 60 s, a client that doesn't read its answer after 30 s. A client past 1 MiB unread has its next command left unread until it catches up, so nobody works for answers nobody's taking. An answer is capped at 32 MiB. All unread answers together are capped at 128 MiB for large ones, and at 144 MiB for anything: small answers can go past the first, so `status` still answers, and whoever would take it past the second is dropped. A watcher that stops reading is dropped past 1 MiB of backlog, and one that writes anything at all goes at once, since it could otherwise hold the only working thread. A panic in a job is an error for that one client. There's no rate limit beyond these caps yet.

**What must always get through.** At most 128 connections, of which at most 64 may be in `wait`, `watch` or a waiting `permission request`. Past 128, up to 16 more may connect for `status`, `stop`, `permission show` and the message after `permission set`, and nothing else: anything else gets `busy`, exit 10, and each new one pushes out the oldest of these. A new connection reads the level before anything else, so a lowered level ends waits and watches even with every slot full, and a wait's look reads it again.

**`stop`.** It closes the door to anything but quick answers, refuses what's still queued, waits up to 5 s for what's running, and only then looks for unsaved or undone edits. An edit can't slip past that check. If something stays stuck, it says so and the session carries on.

**Out of descriptors.** On macOS, an `accept` that fails for want of descriptors drops the waiting connection (measured: 20 connections at a limit of 12, 8 accepted, 12 failed once each, then nothing was ready even after freeing some). So the listener doesn't stay ready, and there's nothing to spin on: those clients see their connection end. `out_of_descriptors_the_session_doesnt_spin` holds that. It would need a pause on an OS that behaves otherwise, and Windows is a different seam anyway.

**Local only.** The socket is mode 0600 in a 0700 folder that's checked, never repaired: a link, someone else's folder or an open one is refused. The peer's user is checked on every connection, and the client checks the socket and its folder the same way. There's no TCP listener, and a test holds the source and the running process to that. A client that's gone before it's accepted can't pass the peer check, so what it sent is dropped.

**Starting and stopping.** A server takes an `flock` on `run/server.lock`, held until it ends however it ends, so a second start is refused, and a socket file found while holding the lock is known to be left over. Only a socket of ours is removed. Clients take a socket nobody's listening on as "not running", exit 6. `recto serve --background` starts another copy of itself and blocks on its ready line. A session's tabs and unsaved edits end with it, and a new one starts empty.

**The app later.** `Server::bind` and `Server::run` take a `Host`, and `Stopper` ends it from another thread. The Mac app runs the same server on a background thread with a host that posts to its own session, so `recto` works the same against either, and it can show `permission request` as a prompt rather than a line on stderr. `SO_NOSIGPIPE` is set on every socket so a host needn't have ignored `SIGPIPE`.

**Asking for help.** `recto --help`, `-h`, `recto help`, `recto help page`, `recto page --help` and `recto help "page move"` all work, the last because the help's own list prints names that way. `recto --version` gives the version. `--flag=value` means `--flag value`. **A near miss gets the closest form that works, driven by the table, on every row.** An unknown flag gets one named or spelled nearly that (`text --pages 19` says `--page 19`), or the place a value goes that has no flag (`goto --page 5` says `recto goto 5`). A stray word gets its flag (`text 1` says `--page 1`, `close #1` says `--tab "#1"`), or quotes for more words, or `recto open` for a file. A value is put in only if its argument takes it, so `text 1-3` advises nothing that would be refused, and every one of these also gives the command's usage line. A mistyped command lists the nearest rows first, by spelling and then by the words of their summaries, so `recto doc` offers `facts`. A row that isn't built says so in `help`, the skill and `not_built`, in place of a level that reads as though it works.

**Levels.** `Off`, `Read-only`, `Control`, `Full`, default `Off`. Each command's level is in its row, read fresh from the file for every command, so lowering applies at once on connections already open, and to waits and watches, which it ends. `status` says how many PDFs are open at every level, Off included, so an agent can tell whether asking for access is worthwhile. A count isn't document content and holds no name, so this doesn't touch the rule that nothing from a document reaches Off. It fails closed: a missing, malformed, linked or open file is `Off`. Nothing over the socket can raise it: no command writes the file, and `level_changed` carries no level. No variable or flag does either.

**How the user sets it, in the headless phase.** `recto permission set <level>`. Lowering needs nothing. Raising needs stdin and `/dev/tty` to be a real terminal, and a word typed back that's different every time, read from the terminal. **The app's settings take this over later,** through the same `levels::store`. A refusal's `ask` is `recto permission request <level>`, which grants nothing. It answers at once, and tells the agent what to tell the user and to carry on with what it can. With `--timeout` it blocks until they've answered, and its help says that's only for when someone's at the keyboard: an agent in the blind test blocked for 2 minutes and then 5. It writes a line for the user only for a level higher than it's said since the level last changed, so asking in a loop can't fill that file, and a write that fails can't end the session.

**`recto skill install`** takes the same check (coordinator's decision, 2026-11-07): it writes into the user's agent's own files, and `--replace` keeps no backup. `recto skill show` stays open at Off, so an agent can read it. With a test state folder it won't write into the real home, going by where the path really leads.

**Below Full, an agent can throw away only agents' edits, never the owner's** (SPEC decision 20). Each step on a tab's undo and redo lists records its `Author`, and keeps it through undo and redo. Everything through `recto_agent::execute` is `Agent`. `Session::edit_pages` and `insert_pdf` are the app's own and are `Owner`, and `SessionHost::with_session` is how the app reaches them. `PageEdits` starts as the owner's and only the session changes that, so a path that forgets to say gets the more protected answer. It costs 8 bytes a step, nothing per page and nothing on a tab with no edits: at most about 4 KiB of a tab's history, on the 256 steps it can hold, only while it has edits.

At Control: an edit that clears undone steps, `discard` and `close` with undone steps go ahead if all that would be lost is agents'. If any of it is the owner's, it's exit 5 (`would_clear_redo`, or `would_discard_user_edits` for `discard`), the message says they're the user's edits, and `recto redo` or asking for Full are the ways on. Undoing the owner's steps is allowed, since `redo` brings them back. A tab with unsaved edits doesn't close at any level, so closing never loses edits silently: `discard` first, then `close`. A refused `close` names all it would lose and each way on that's real. `stop` still counts undone steps as unsaved, whoever's they are. At Full, what's lost is the holder's call.

**The undo cap.** When a step falls off the capped undo list its effect stays in the page order, so the cap hasn't erased it, and `is_edited` keeps saying the tab differs from its file. But `discard` would throw that effect away, so an owner's step that fell off still counts as the owner's (`owner_step_dropped`) and `discard` below Full stays refused. A redo step dropped by the byte cap is gone for good, and an agent could otherwise cause that by undoing, so the trim stops at an owner's step, and at any under it, because a redo step only applies to the state just before it. That can't take the list far over: what redo holds is the pages of undone inserts, which a document's length and the cap bound to `MAX_HISTORY_BYTES` plus one document, and `undoing_never_drops_an_owners_undone_step` holds it there. **Limit:** an agent can still delete a page the owner rotated, then make 256 more edits so the delete can't be undone. Nothing was thrown away by `discard` or the like, and the rest of the owner's edits stay, but that one is out of reach until the session ends. A cap on the agent's own steps would trap ordinary work, so this is stated rather than closed.

**What this does and doesn't protect against** (agent report, threat T16). It shuts every ordinary route: an agent running commands through a tool has a pipe, not a terminal. It can't stop a process of the same user that goes to the length of faking a terminal or writing the file itself: that process could read the PDFs directly anyway. A variable can point a run at a fresh state folder, which starts at Off, and writing a file into it gives access only to the throwaway session there, not the user's. The app's settings, behind a human gesture, are the stronger answer.

**Signing** is out of reach at every level. `recto sign` always refuses, and nothing in the control path links a signer. `recto signatures` reports facts through `recto_core::signatures`, a bounded walk of the form fields, with `recto-sign`'s wording: "Signature present, not checked", never "valid". Stated names and times go under `untrusted`, because they're only what the file claims.

**State folder.** `~/Library/Application Support/RectoPDF/agent`, under the real home from the user database, not `$HOME`. `RECTO_STATE_DIR` overrides it for tests and QA, and refuses the real home, anything holding it, and anything inside it unless that's in the OS's temp folder (from `confstr`, not `$TMPDIR`), through links too. If the path is too long for a socket (the kernel's limit is 103 bytes), the socket goes in `/tmp/recto-<uid>/`, checked the same way and named for the state folder.

**Activity log.** One line per action with its time, command, level and outcome, refusals included. It holds numbers, and cleaned file names under `untrusted`, and never document text: `Command::log_details` is the one place that decides, and a tab's name as typed and a search's words aren't kept. `tab` is the `#id` the command really acted on, found before it runs because `close` leaves nothing to ask afterwards. If it found no one tab, as with an ambiguous name, it says `by name`. A refusal for the level is counted rather than written out, so it keeps no details at all. At a real terminal `recto log` reads the files with no session or level, for the user. Otherwise it needs Read-only, even with no session.

An agent mustn't be able to erase its own record, so there are two files, 1 MiB each and rotated once. `activity.jsonl` has what changes something: `open`, `close`, page edits, `render`, `stop`, `permission set`. `activity-other.jsonl` has the rest, so no flood of reading or refusals can reach the first. `status`, bad lines, asks and refusals are written once, then as one line with a count, at most once a minute each. **Limit:** an agent at Control can still push out its own changes with enough of them, since each one is an action to record.

**Contents.** A first look at an outline needs its sections, and a root can hold only the title, as on RFC 9110, where `--depth 0` gives `RFC 9110` and `HTTP Semantics` and nothing else. So `sections_at` is the first depth whose entries are on more than one page. With no `--depth`, `contents` shows down to the deepest level that fits in 200 entries, never above the sections, and `depth_rule` says which that is, which entries are the top-level sections, and what to ask for instead, ahead of the list in plain output. An agent took two calls to work out what "top-level" meant. On RFC 9110 that's depth 2 of 4, 154 of 311 entries, in one call. Paging keeps to the same entries: the depth is decided from a fixed room of 200, not from `--limit`.

**A file only the renderer can read.** lopdf refuses some damaged files PDFium repairs, or finds no pages in them, and commands go through `session.open`. So `DrawableLoader` asks PDFium, in its worker, when lopdf says "not a PDF" or "no pages": the tab opens with PDFium's page count and `renderer_only` set. Facts stay trustworthy because nothing is made up. `facts` says `structure: renderer_only`, `contents` says `available: false`, and `signatures` says `structure_read: false`, each meaning "couldn't be read" and never "none". Editing it, or inserting it into another document, is refused, since only the structure reader's writer may change a PDF. A password-protected, missing or forbidden file stays that way: only those two answers get a second opinion.

**Search for now.** `recto search` reads each page's text through one worker at a time, in order, ignoring case and line breaks, with no index: at most 200 hits and 30 s, stopping only between pages so `next_page` always moves on. Snippets are built only for hits that will be shown, and the count stays whole. It's slow on a long document, and the Phase 3 corpus goes behind the same command.

**Added files in search.** Every hit says `from_added_file` and `source_page`, whatever they are, and one from an inserted PDF also says `added_file` under `untrusted`. Plain output says so only when it isn't the number shown. Plain output also opens with what it covered: hits shown, pages searched of how many, and whether there's more. An agent read past "stopped early" at the bottom. That stays at the bottom too, with where to go on from.

**Hidden text in search:** only a page with a hit is drawn to check it. Drawing every page costs too much for what it adds, since a page with no hit is never read: rfc9110, 194 pages, went from 1.14 to 3.2 ms a page, and NASA's 383 pages from 3.5 to 16 ms a page (release, this Mac, 3 runs each, `recto local F search zzzqqqxx`, with the check switched on in code for the second). Each hit says `hidden_text: { checked, flagged }` and `page_has_tag_chars`. `overlaps_hidden` says whether its own words include hidden text, so a page's flag doesn't fall equally on the innocent hit beside them: `true`, `false`, or `null` if that can't be said. That needs nothing more than the draw a hit already pays for.

**Characters past 16 bits.** A font's ToUnicode gives these as two halves, and PDFium reports each as a character. `recto-render` joins them, or an emoji is lost and a tag character, the plainest hidden instruction there is, never raises its flag.

**Files that change.** A tab, and each PDF added to it, keeps the size and modification time its page numbers were recorded from. If an added file, or an edited tab's own file, has changed or gone, `text`, `render`, `contents` and `signatures` refuse with `source_changed`, and `search` lists those pages as failed and names the file. An unedited tab has no numbers to get wrong, and reads the file as it now is.

**Edits and pages.** A tab's place follows the page it's reading, and if that page is removed it lands on the nearest and says so once. A draw's note goes when the page order changes. Page numbers on every command mean the document as edited: `text` reads the page now in that place, from whichever file it came from, and says `source_page` and `from_added_file`. `render` doesn't show an unsaved rotation, and says so in `unsaved_rotation_degrees`. Deleting a page is `Control` rather than the report's `Full`: nothing reaches a file, and it can be undone. The two-step plan and confirm belong to `save`, when that exists.

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
