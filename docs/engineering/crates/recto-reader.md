# recto-reader

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-reader` | **Started** | The page view with no toolkit: where pages sit, which tiles are wanted, what's kept, what the host is told, dark paper, a selection and what's been read for it. One thread that runs the renderer | Name a UI toolkit; wait on the UI thread |

## Parts, and how they're reached (R3.6, 2026-11-13)

`Reader` (`tiles.rs`, the crate's own, `pub(crate)`) is a composition of parts, each in `tiles/<part>.rs`: `Docs` (the tabs held and the tab showing) with each tab's `DocView` (its points, pages, place and zoom in `tiles/geometry.rs`), `Pointer`, `Place`, `Work`, `Waking` (with `SleepDraws`), `TileCache`, `HostQueue` and `Scratch`. What reads or changes one part is that part's method; `Reader`'s own methods (82, most of them `refresh`, `plan`, `activate` and the pointer's, place's and wake's operations) change several parts together. The engine reaches a part through `Reader`'s `pub(crate)` field and calls only that part's `pub(crate)` methods: no part has a `Default` or a constructor outside `tiles`, and `Reader` has none either (only `Reader::new`), so outside `tiles` a part can be built only by `Reader::new()` and so replaced only by assigning a field of one from another (`reader.docs = Reader::new().docs`), which nothing does and `surface.rs` would show as a new `impl` or field; making the fields `pub(super)` with accessors would stop even that, at about 14 more methods on `Reader` (a shared and a `_mut` one for each of seven parts) against the ratchet's 82, so it isn't done. And `src/surface.rs` lists what each lets other code reach. Outside the crate, `Engine` is the page view: its own methods for the view (activate, viewport, zoom, jumps, drain and applied, the window) and five facades, borrows of it built inline, for a concern a caller can take alone: `selection()`, `links()`, `points()`, `waking()` and `workers()`, each taking the page view's lock as the `Engine` method it replaced did. `engine/run.rs` is the thread that draws and `engine/keys.rs` the keyring.

**How the scroll step is measured, so it can be measured again the same way** (R3.6a, 2026-11-13). `tiles::tests::cost::scroll_step_time` in `crates/recto-reader/src/tiles/tests/mod.rs`, ignored, on a release build: the UI thread's whole part of a step over six pages drawn and kept (a viewport, a drain, the host's word that it applied them), 5,000 steps a round and five rounds, p50, p90 and p99 of a step printed for each round. It asserts nothing about time. The allocation counter of that test binary costs a thread-local look on each allocation, the same on both sides of a comparison:

`tools/sysguard/heavy.sh cargo nextest run --release --locked -p recto-reader --run-ignored only --no-capture -E 'test(=tiles::tests::cost::scroll_step_time)'`

The scroll's path was `(step % 25) * 280` points: it fell from 6,720 to 0 every 25th step, which is a jump over the whole document and not a scroll step, and with 4 steps in 100 it sat inside p90 and p99. It is now a scroll down and back up (24 steps of 280 points, then 24 up, a period of 48 with no repeated place). On 2026-11-13 at 73a6385d (the same page view code as `main`'s), load 8 to 19 on 10 cores, three runs of five rounds, the ranges over all fifteen rounds: with the jump, p50 6.33 to 6.42 µs, p90 9.67 to 9.88 µs, p99 11.71 to 11.88 µs; as a scroll both ways, p50 6.42 to 6.54 µs, p90 8.54 to 8.67 µs, p99 9.96 to 10.25 µs (the first round of the second run was 9.38 µs at p90 and 15.12 µs at p99, which the load made). The wrap was worth about 1.2 µs at p90 and 1.7 µs at p99; the step itself, p50, is the same.

**QA seams.** `Engine::hold_busy` (and the `qa_busy` flag it sets, read where the UI thread's calls try the lock) exist only with the crate's `qa` feature, which `recto-app`'s `testkit` and its tests turn on; a release app has no such field or method (without the feature a call to it fails to compile, 2026-11-13). `Engine::hold_drawing` stays in every build: `recto qa hold-drawing` uses it and release builds accept `--qa`.

## Per-item sizes

| Struct | Held per | Limit |
|---|---|---|
| `recto_reader::tiles::cache::TileKey` | Tile in the page view's cache: a key of `cover`, one for each tile an entry covers | ≤ 20 B, held at compile time in `tiles/cache.rs`; 20 B today |
| `recto_proto::TileKey` | Nothing yet: the key a tile cache on the host's side would hold, not on the wire | ≤ 16 B, held by `per_item_sizes_stay_small` in `recto-proto` |
| `recto_reader::layout::Override` | Visited page whose size isn't the document's usual one, or turned the other way from most, at most 65,536, hot and warm tabs only. Turning every page adds none | ≤ 32 B |
| `recto_reader::tiles::doc::DocView` | Hot or warm tab the page view holds; none for a cold one | ≤ 672 B, held at compile time in `tiles/doc.rs`; 672 B today, the same before and after it was cut into its place and its drawing |
| `recto_reader::tiles::scratch::Scratch` and the spare lists of `TileCache` | Page view (a window or a comparison's pane), not per tab: the lists a scroll step fills, kept so a step over drawn pages allocates only the ops it hands the host | At most 14,592 B of lists and 8 spare lists of 64 tiles between steps; 1,168 B after the scroll of the allocation guard; none with no tab or a closed window |
| `recto_reader::tiles::PageMap::turns` | Page of an edited tab, hot and warm only, and only once a page of it is turned; also every page of a tab whose view is turned (the state record's view rotation) | 1 B |

## Tiles, zoom and files that do not answer

**Regions** are one surface and one layer, with no cutting up. Blank tiles get no surface, only a colour, or nothing if they're the paper's colour. Grey tiles are still BGRA: `L008` would take a text page's tiles to a quarter, and it's only known correct in `render(in:)`, not on a real screen, so it waits for the MacBook spike.

**What's kept**, all in `recto_reader::tiles`: what's on a layer stays. Off a layer it goes discardable the moment the host says it's applied, which takes it out of the footprint: 64 tiles go from 66.6 to 2.7 MiB. Kept: 256 MiB and 1,024 surfaces of tiles, least recently used first, and 128 previews, which outlive their tiles. **A surface serves one job.** There is no pool: a worker can read and write a surface it was sent for as long as it lives, so reuse would be a way from one file's worker to another file's pixels. It costs 51 µs more a tile, 67 against 16 p50, nearly all the first touch of its pages. Attached: at most 8 a turn, with every visible page's preview first. A cold tab has no entries and no renderer, and a closed window has neither. The page layout costs nothing per page: pages are taken to be the size of the first one known until asked about, and only pages of another size are stored, so a file claiming 8 million pages costs nothing until it's read.

**A worker past the retirement line goes 3 s into a pause** (a heavy worker's idle rule, `Policy::heavy_idle_exit`, in `big_workers_rest`), not the idle rule's 30, and drawing for a tab off screen's place doesn't put that off: it's looked at between those jobs too. It's the only timer in the page view, and it's there only while such a worker is. The USGS map at rest goes from 433 MiB to 36 MiB, and the first move after that pays for the image again.

**Pinch and scroll.** Layers are page-relative in PDF points, under one transform per page, so a pinch moves one transform and a change of fit moves no tile. Tiles stretch during a pinch, and nothing's drawn until it ends. New tiles go on top of old ones, which come off when the page's visible tiles are all there. Pages are snapped to whole device pixels. `NSScrollView` does the scrolling, so rubber-banding, momentum and the scrollers are the system's. **Not checked on a screen:** a quiet run has none.

**One zoom path.** `Zoom::{In, Out, Fit, Actual, To}` in `recto-reader`, with `ZOOM_STEP` 1.25. ⌘+, ⌘-, ⌘0, plain + and -, the menu, the panel and `recto view --zoom` all reach it. ⌘-scroll goes through `zoom_after_scroll` about the pointer, in `PageScrollView`, which doesn't pass that wheel on, so ⌘ never scrolls.

**A file on a dead volume.** A file that doesn't answer holds a thread as long as it likes, so neither the session thread nor a page job waits on one. `open` and `Reader` reads go to one-shot `recto-read` threads, at most `MAX_FILE_READS`, 4, with the flush markers held behind them. A worker's file is opened on a one-shot `recto-open` thread, at most `MAX_OPENINGS`, 4, looked at for `OPEN_LOOK` and given `OPEN_PATIENCE`, 3 s, then the page says so (`NOT_ANSWERING`). Past either cap a file doesn't open and says why. Tested with a FIFO, which blocks an open the way a dead volume does: no real dead volume was used.

**What a worker can hold.** A surface costs the host, and the worker that was sent it can keep it as long as it lives, which no limit on its own memory sees. So each worker's surface bytes are counted: past `surface_bytes_cap`, 1 GiB, it's retired before its next page, and at twice that before its next job, so a one-page file can't dodge it. That's about 80 pages of text a tile at a time, for a restart of 17 ms.
