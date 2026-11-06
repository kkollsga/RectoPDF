# R1 and R2: prior art and known failures for RectoPDF

Date: 2026-11-06. Track: R1 (prior art) and R2 (failures turned into rules and tests). No code exists yet. Nothing was
installed and nothing was benchmarked locally.

## 1. The findings that change the design

1. **Nobody in the source we read frees a background tab.** Folio keeps a full iframe, React app, pdf.js viewer, worker
   and file bytes per visited tab until it is closed. SumatraPDF only delays loading at session restore. Sioyek caches
   every opened document. Freeing a background tab, with a per-tab cost in kilobytes, is open ground.
2. **SumatraPDF's memory reputation does not come from an aggressive page cache.** Its cache is bounded by count (256
   tiles, about 2 GB in theory on a 1080p screen), and its own comment says it should be bounded by bytes. The savings
   come from a native UI, one cache across all tabs, reading files over 32 MiB on demand, and lazy restore. MuPDF's
   256 MiB store is per document on top of that. We can beat this.
3. **Search is the top memory incident across engines:** Zathura 4 GiB and 2.8 GiB, SumatraPDF 3 GB on an 80 MB file.
   pdf.js keeps all page text in memory after the first query, and sioyek keeps the whole document as one wide string.
4. **Annotation data loss comes from state paths, not from the file format.** The recurring cases are a reload that
   drops unsaved work, a dirty flag some edit type never sets, and tab or window moves. This needs a test matrix of
   edit types against exit paths.
5. **Page labels are absent or second-class nearly everywhere, and they can be garbage.** Folio has none at all. Its AI
   citations are physical page numbers matched by a regex. Its own screenshot shows "(p. 2, p. 4)" left unlinked.
6. **PDFKit is an unsafe writer on the machine we develop on.** Preview and PDFKit on macOS 27 permanently damage
   checkbox and radio appearance streams on save (Apple forum thread, September 2026, still open on 27.2). This Mac
   runs 27.0.1, so Preview must not be a test oracle and PDFKit must not write our files.
7. **SumatraPDF is moving into our space.** HEAD has an `ng` port for macOS, Linux and wasm, an AI chat sidebar, form
   filling and signature support. Stable is still 3.6.1 (April 2026). Those features are unreleased as stable. We did
   not find a sidebar tab model there.

## 2. Pinned versions

"Read" means source inspected at this commit. All `file:line` citations are at these commits.

| Project | Pin | Date | License | Read how |
|---|---|---|---|---|
| Folio | `3b6771c` (v1.16.0 plus 5 commits; v1.16.0 tagged 2026-11-05) | 2026-11-06 | MIT | open-source MCP, graph and read tools |
| sioyek | `1e2f803` (development, v2.0.0-1170). Last stable v2.0.0, 2022-12-16; alpha sioyek3, 2024-09-11 | 2026-11-04 | GPL-3.0 | open-source MCP |
| SumatraPDF | `075b05c` (3.7 pre-release). Latest stable 3.6.1, 2026-04-06. Vendors MuPDF 1.28.5 | 2026-11-06 | GPL-3.0 | open-source MCP, plus direct read of the clone |
| pdf.js | `50d1307` (v6.5.438 plus 60; Folio ships 6.5.438) | 2026-11-06 | Apache-2.0 | open-source MCP |
| Zathura | man page and tracker only; latest tag 2026.10.23 | 2026-11-06 | Zlib | GitHub API |

GPL warning: SumatraPDF and sioyek are GPL-3.0. "ADOPT" below means taking the idea and writing our own code. Only MIT,
Apache-2.0 and Zlib sources can be ported as code, with attribution.

Evidence grades: **[S]** source at the pin. **[T]** project issue tracker. **[D]** vendor docs. **[L]** forum or blog,
a lead only (`rust_app_development.md` §1.1). **[A]** my analysis of source, not measured.

## 3. Comparison

| App | Architecture | Memory and speed defaults | Sidebar and tab model | What users complain about |
|---|---|---|---|---|
| **Folio 1.16** [S] | Electron 45, React, pdf.js 6.5 viewer, PDFium wasm for edits, pdf-lib, tesseract.js | Whole file in worker RAM. Refuses over 300 MB or 10,000 pages. pdf.js defaults: 10-page view buffer, 2^25 px canvas cap. No published numbers | Left card: Pages, Outline, Notes. Horizontal tab strip appears at 2 or more tabs. Right: Ask AI panel | Form text lost, text boxes lost, silent save failure, main-process block on big files, Arabic editing broken (section 4.6). Project is 11 days old |
| **SumatraPDF** [S][T] | Native Win32 C++ on MuPDF. `ng` port in progress | Under 32 MiB read to RAM, else on demand. One global cache of 256 tiles. 256 MiB MuPDF store per document. Lazy session restore | Top tabs with tab groups. Bookmarks tree tracks the current page. Thumbnails and favorites panels | Annotation save corruption and loss (2021 to 2026), hangs on heavy vector pages, UI stalls on SMB, Arabic copy order |
| **sioyek** [S][T] | Qt, OpenGL, MuPDF, SQLite | 4 render threads. Keeps the 5 most recent pages times 5 slices; older ones freed after 1 s unused. Whole-document search string in RAM | Tabs are a command-driven list (`goto_tab`, cap 100), no strip found. TOC opens on command (`goto_toc`), not a fixed sidebar | Memory growth to 1.2 GB and 3 to 4 GB over long sessions (open since 2023), clock-step scroll lock, TOC does not follow position, crashes on UTF-8 |
| **pdf.js / Firefox** [S][T] | JS engine and web viewer | `DEFAULT_CACHE_SIZE` 10, canvas cap 2^25 px, idle cleanup after 30 s. Lazy page init past 5,000 pages, forced page mode past 10,000 | Sidebar with thumbnails, outline, attachments. Tabs belong to the browser | Out-of-memory on odd files, worker memory staying high, `/ActualText` ignored, no modified-after-signature indication |
| **Skim** [D][T] | Native Cocoa on PDFKit | Not documented. FAQ silent on memory | Left and right side panels for TOC, thumbnails and notes (not verified against docs). Tabs not verified | Notes in extended attributes are lost by email, Time Machine and some copies. Many display bugs closed as PDFKit, outside Skim's control. Repeated reload prompts [L] |
| **Preview / PDFKit** [T][L] | Apple's framework | Not published | Thumbnails or TOC sidebar. Native window tabs (not verified) | macOS 27 widget damage on save and pixelated widgets. Older reports of highlights vanishing [L] |
| **Zathura** [D][T] | GTK and girara with poppler or MuPDF plugins | `page-cache-size` 15 pages, LRU. Thumbnail cache for quick zoom previews. SQLite for bookmarks | Index overlay on Tab. No tabs; users embed it in `tabbed` and hit bugs | Search memory 2.8 and 4 GiB, position lost on reload, segfault on reload, ToC CPU and memory leak |
| **Okular** [D][T] | Qt, KDE, Poppler | Memory profiles: Low, Default, Aggressive, Greedy (preload up to 50% of memory) | Left navigation panel with contents and thumbnails (not verified against docs). Tabs not verified | Reload or Save As losing annotations, reload after self-save, holds a Windows file lock (2026) |
| **PDF Expert** [L] | Closed, native Mac | Not published | Not verified | Annotations lost around renames and DEVONthink integration. Weak evidence |
| **Foxit** [L] | Closed, native | One vendor-run test only (section 7) | Not verified | Searches found mostly CVEs (2025 and 2026 use-after-free on page changes), no usable tracker. Weak evidence |
| **Acrobat** [L] | Closed, native | One vendor-run test only (section 7) | Not verified. A redesigned UI drew complaints from 2023 | Holds exclusive write access on Windows (2017, "will not be changed"), new-UI freezes of 10 to 30 s, slow open on 64 GB machines |

**Sidebar model:** in everything we could verify, tabs and the TOC are separate surfaces. Open documents are a list in
one place and the outline is a panel in another. We did not find tabs in a sidebar with the TOC collapsed under the
active tab. Closed apps are unverified, so this is not a claim that it is unique. Sioyek #525 (2022, open) asks for a
TOC that is always visible, and #456 asks for one that follows the reading position.

## 4. Folio in depth

### 4.1 Tabs and tab memory

- A tab that has been visited never releases anything. Each gets its own iframe, on purpose: "pdf.js installs editing,
  selection and clipboard handlers on its window. Give each live reader its own document" (`src/documentTabs.ts:31`,
  frame created at `:79-80`).
- Switching only toggles `inert` and a CSS class (`documentTabs.ts:76`). Nothing is torn down until close
  (`:170`). When a tab goes inactive, the code touches the keyboard handler (`src/App.tsx:1090`), title bar and theme
  (`:253,1050`), OCR scheduling and the signature dialog (`:274`, `src/pageOcr.ts:274`). None of these releases memory.
- A tab dropped with others but never selected has no frame, but its `{kind: "bytes"}` source stays in JS memory until
  delivered (`documentTabs.ts:5,105,112-117`). [A]
- Cost per live tab [A]: one JS realm with React, one pdf.js worker with the entire file (`src/pdf.ts:18`, copied with
  `data.slice()`), up to 10 page views with canvases, an overlay canvas per page with images, cached AI text, and the
  find controller's arrays once searched. Not measured.
- The Pages sidebar requests every page in the background, 6 at a time (`src/thumbnails.ts:12,86-108`), so page proxies
  grow with page count while that view is active. Virtualised rows are good. [A]
- Identical bytes open twice collapse into one tab, because notes are keyed by content (`documentTabs.ts:134`). Worth
  keeping as a behaviour.

### 4.2 Virtualisation and canvases

Folio delegates all of this to pdf.js `PDFViewer` with defaults (`src/App.tsx:290-301`), and sets no canvas limits.
pdf.js at the pin:

- Page buffer: `DEFAULT_CACHE_SIZE = 10`, resized to `max(10, 2 * visible + 1)` (`web/pdf_viewer.js:75,1999`).
- Canvas cap `maxCanvasPixels = 2^25` (`web/app_options.js:599`), which is 134 MiB for one canvas.
- Priority: visible pages, then zoomed detail views, then the next page in scroll direction
  (`web/pdf_rendering_queue.js:112-135`).
- A DOM element and page view exist for every page, which is why Folio caps at 10,000 (`src/pdfInput.ts:2-3`), and
  pdf.js has its own limits at 250, 5,000 and 10,000 pages (`web/pdf_viewer.js:77-80`).

### 4.3 Text for search and AI

- Search is pdf.js `PDFFindController` (`App.tsx:288`). The first query extracts every page, and normalized text and
  diff maps stay in arrays for the life of the document (`web/pdf_find_controller.js:602,890-923`).
- Snippets call `getTextContent` again (`src/SearchResults.tsx:37`).
- AI text walks pages in order, writes `[Page N]` markers, stops before the page that would pass 400,000 characters and
  sets `truncated` (`src/AiPanel.tsx:22,33-53`). It is cached for the document's life.
- OCR text exists only as a selection layer; search and AI cannot see it (`README.md:249-251`). PR #21 is open for this.

### 4.4 Page citations

1. The prompt says "Cite specific information like (p. 12)" (`electron/ai-prompt.cjs:7`).
2. A remark plugin matches `/\((?:p|pp|page|pages)\.?\s*(\d+)(?:[–-]\d+)?\)/gi` in prose nodes and skips code and math
   (`src/AiMarkdown.tsx:25`).
3. The match becomes a button (`:48-53`) calling `goTo`, which clamps to the valid range (`src/App.tsx:618-622`).

Weak points: only physical pages. A range links its first page only. "(p. 2, p. 4)" is not linked, visible in Folio's
own `docs/screenshots/ask-ai.png` on the Save bullet. An invented page 900 silently goes to the last page, and nothing
checks that the passage is on that page.

### 4.5 PDF dark mode

CSS `filter: invert(0.88) hue-rotate(180deg) contrast(0.92)` on page canvases (`src/styles.css:604-607`). A second,
unfiltered canvas of the same pixel size holds only the image rectangles copied from the render, using coordinates
pdf.js records because `imagesRightClickMinSize` is set (`src/App.tsx:300`, `src/nightImages.ts:57-85`).

The idea is good and the cost is not. The overlay is painted on every `pagerendered` even with night mode off; CSS only
hides it (`nightImages.ts:87-93`, `src/reading-tools.css:635-646`). Any page with an image costs up to twice the canvas
memory all the time. [A] Vector figures and gradients are still inverted, since only images are recorded. Text selection
over a full-page scan needed a z-index fix (`reading-tools.css:652`).

### 4.6 Annotation persistence and what broke

Marks go to IndexedDB keyed by a full SHA-256 of the bytes plus length (`src/recent.ts:66`), debounced 400 ms
(`src/markup.ts:386`), and are recreated into pdf.js editors when each page renders (`markup.ts:226`). Marks on pages
not yet rendered are injected only for the export (`markup.ts:402`, `App.tsx:852-875`). The PDF on disk is never
written: Save is always a downloaded copy (`App.tsx:923`).

Consequences: other apps never see the notes unless exported, any byte change detaches them, and there is no save in
place or external-change reload at all (grep for `fs.watch`, `chokidar` and similar finds nothing). Password-protected
files are never stored, which is right. A `clearGenerations` store stops a suspended tab from writing cleared data back
(`recent.ts:219-232`), a neat trick worth taking.

Real bugs: [#36](https://github.com/Forward-Future/folio/issues/36) typed form text lost with no warning (2026-10-31).
[#11](https://github.com/Forward-Future/folio/issues/11) text boxes vanish on reopen (2026-10-30).
[#4](https://github.com/Forward-Future/folio/issues/4) save and print fail silently (2026-10-30).
[#3](https://github.com/Forward-Future/folio/issues/3) big file read blocks the main process and spikes both processes
(2026-10-30). [#5](https://github.com/Forward-Future/folio/issues/5) Recent keeps full bytes, up to about 2.4 GB
(2026-10-30). [#49](https://github.com/Forward-Future/folio/issues/49) Arabic editing broken (2026-11-04, open).
Changelog 1.10.5: printing cleared the unsaved flag, and highlights on a loading page were dropped. 1.10.3: exports
missed marks on unvisited pages, and Copy reported success without copying.

### 4.7 What users ask for (issues to 2026-11-06)

Page edits: rotate, delete, insert ([#69](https://github.com/Forward-Future/folio/issues/69)), extract
([#65](https://github.com/Forward-Future/folio/issues/65)). Searchable OCR
([#15](https://github.com/Forward-Future/folio/issues/15), PR #21). Shared AI memory across a folder of PDFs
([#70](https://github.com/Forward-Future/folio/issues/70)), which our agent CLI could serve well. Attachments
([#53](https://github.com/Forward-Future/folio/issues/53)). Image DPI
([#55](https://github.com/Forward-Future/folio/issues/55)). Default-reader registration
([#64](https://github.com/Forward-Future/folio/issues/64)). Homebrew and WinGet
([#72](https://github.com/Forward-Future/folio/issues/72)). Page-per-wheel-notch
([#74](https://github.com/Forward-Future/folio/issues/74)). Nobody has yet filed on tab memory or the size cap, which
says little about a project this young.

### 4.8 AGENTS.md rules worth stealing

- One release path, all platforms or none, with a daily check that opens an issue if a platform is missing.
- The smoke test runs off-screen and leaves the clipboard alone. Agents must not open a visible window unless asked.
  This matches `rust_app_development.md` §4.
- A short list of the few places where platform differences live, to check on any related change.
- PR title states what changes for the user. Body gives problem, change, approval, and how it was checked per platform,
  ending with model and harness. No version or changelog edits in PRs.
- The changelog is in user language with the old behaviour stated ("Before, it could drop the highlight").

### 4.9 Design, so we can differ on purpose

From `ask-ai.png`, `highlights-and-notes.png`, `search.png` and `back-to-page.png`: a web-app look. Warm off-white or
near-black chrome and one coral-red accent used for the logo, active thumbnail, citations and send box. Large radii and
pill segmented controls throughout. Top bar with logo, document name, a centred mode switch (Select, Draw, Text, Sign)
and right-hand icons. Left floating card with three equal tabs. Search as a floating bar plus a results card that covers
page content. A floating bottom pill for page, zoom and rotate that overlaps the text (legible overlap in
`highlights-and-notes.png`). AI answers as pink-tinted chat bubbles on the right.

Deliberate differences to propose (D8): native controls and system accent, no chrome floating over the page, one
sidebar where documents and their outline are a single tree, and less visual weight on modes.

## 5. SumatraPDF and sioyek questions

### 5.1 How SumatraPDF keeps memory low (`src/shared/`, pre-release HEAD)

1. **Native UI.** No webview in the core path. No toolkit runtime.
2. **Large files stay on disk.** Under 32 MiB is read to RAM so other programs can overwrite the file even without
   `FILE_SHARE_READ`; larger files are read on demand (`EngineMupdf.cpp:890-900`).
3. **One bitmap cache for all tabs.** `gRenderCache` with `kMaxBitmapsCached = 256` (`RenderCache.h:16`), so pixels do
   not scale with tab count. Eviction order: an off-screen page of this document, then stale zoom or rotation tiles,
   then off-screen tiles of a visible page, then other documents' pages (`RenderCache.cpp:378-436`).
4. **Failure degrades.** On render failure, `ReduceTileSize` halves tiles and retries (`RenderCache.cpp:803,1575`).
5. **Lazy restore.** `lazyLoading` delays each document until its tab is selected (`Settings.h:1183-1185`).
6. **Lazy threads.** Render threads spawn when work appears (`RenderCache.cpp:230-232`).
7. **Bounded prediction.** Up to 4 pages ahead, chained one at a time (`RenderCache.h:20`).

What it does not do [A]: the cap counts bitmaps, and its own TODO says so (`RenderCache.h:14-15`). At 1080p one full tile
is about 8.3 MB, so a full cache is about 2.1 GB. `FreeNotVisible` has no caller at this commit, so the cache fills up
before it evicts. `fz_new_context(..., FZ_STORE_DEFAULT)` gives each document a separate store of up to 256 MiB
(`EngineMupdf.cpp:3533`, `ext/mupdf/.../context.h:322`). A background tab is never unloaded. Its low numbers are a
fine floor to beat, not a design limit to copy.

### 5.2 Sioyek search and database

**Search.** With `SUPER_FAST_SEARCH = true` by default (`pdf_viewer/config.cpp:250`), a background thread opens its own
document and context and walks every page at every open, building one `std::wstring` plus page offsets
(`document.cpp:1391-1461`). That is 4 bytes per character on macOS and Linux, plus reference and equation indexes.
Nothing is saved, so every open pays again ([#1634](https://github.com/ahrm/sioyek/issues/1634)). The non-index path
searches page by page from the current page in a dedicated thread, restarting if a new query arrives
(`pdf_renderer.cpp:342-430`). Own dehyphenation: `-` at a line break is dropped, otherwise a space is added
(`utils.cpp:2638-2654`). It reorders glyphs itself, which caused ligature order bugs
([#724](https://github.com/ahrm/sioyek/issues/724)). The maintainer replaced MuPDF search because it did not handle RTL
([#858](https://github.com/ahrm/sioyek/issues/858)).

**Database.** SQLite sidecar. Tables `opened_books`, `marks`, `bookmarks`, `highlights`, `document_hash`, `links`
(`database.cpp:536-642`), plus a separate shared database option for syncing (`config.cpp:143`). Identity is an MD5 of
the whole file (`checksum.cpp:50-58`), computed off the UI thread, then cached per path, with annotations merged in once
known (`document.cpp:163-191`). So moved files keep their notes. Cost [A]: the hash is cached by path, so a rewritten
file can keep its old identity. SQL is often built by string concatenation (`database.cpp:1153`). The DB is never in the
PDF until an explicit export, which writes a new file with default options, a full rewrite (`document.cpp:2625-2626`).

**Reload.** Polling, waits `RELOAD_INTERVAL_MILISECONDS = 200` after the last edit, then runs `is_doc_valid` before
swapping, "because LaTeX software frequently puts PDF files in an invalid state" (`main_widget.cpp:1277-1284`). Good
pattern. Crashed anyway with the TOC open ([#1699](https://github.com/ahrm/sioyek/issues/1699)).

## 6. ADOPT, ADOPT-ADAPTED, KEEP OURS

Effort: S under 1 day, M up to 1 week, L more. Memory cost is for RectoPDF and is an estimate.

| # | Item and source | Label | Effort | Memory | Why |
|---|---|---|---|---|---|
| 1 | One render cache for all tabs, Sumatra `RenderCache` | ADOPT-ADAPTED | M | Reduces. Bound in bytes, not count | Sumatra's TODO names the flaw |
| 2 | Eviction order, Sumatra `FreeIfFull` | ADOPT | S | None | Tested order, including stale zoom tiles (#6366) |
| 3 | Halve tile size on allocation failure, Sumatra | ADOPT | S | Reduces under pressure | Degrade, do not crash |
| 4 | Lazy session restore, Sumatra | ADOPT | S | Large saving | Required for the per-tab budget |
| 5 | Free background tabs entirely | KEEP OURS | M | Largest saving | No one we read does it (D3) |
| 6 | Bounded predictive render, Sumatra, direction-aware priority, pdf.js | ADOPT-ADAPTED | S | Bounded by byte budget | Instant page turn within budget |
| 7 | Scaled thumbnail as zoom preview, Zathura `page-thumbnail-size` | ADOPT | S | Small, per visible page | Zoom never shows blank |
| 8 | Debounce 500 ms, max wait 5 s, Sumatra `SumatraPDF.h:49-51` | ADOPT | S | None | Tested on real LaTeX tools |
| 9 | Parse-validate before swap, sioyek `is_doc_valid` | ADOPT-ADAPTED | S | Brief second parse | Keep the old document until the new one parses |
| 10 | Skip self-saves, Sumatra `ignoreNextAutoReload` | KEEP OURS | S | None | A flag races; its own comment admits it. Compare fingerprint of what we wrote |
| 11 | SQLite sidecar and content identity, sioyek | ADOPT-ADAPTED | M | About 1 MB | Prepared statements, fast hash, re-hash on size or mtime change |
| 12 | Notes only in a private store, Folio, Skim, Okular | KEEP OURS | M | None | Sidecar plus explicit embed and export (D1) |
| 13 | Clear-generation token, Folio `recent.ts:219` | ADOPT | S | None | Stops late writers restoring cleared data |
| 14 | Search normalization with offset map, pdf.js `normalize()` | ADOPT-ADAPTED | M | Transient per page | Apache-2.0, can port. Add `/ActualText`, which pdf.js lacks |
| 15 | All text in RAM, pdf.js and sioyek | KEEP OURS | M | Saves tens of MB per large document | Stream pages, cache on disk (D4) |
| 16 | Search from current page, restart on new query, sioyek | ADOPT | S | None | Cancellation behaviour is right |
| 17 | Spawn hardening for Claude CLI, Folio `electron/ai.cjs:384-399` and PR #76 | ADOPT | S | None | Tools off, strict MCP, no settings, private empty cwd, no persistence |
| 18 | Whole-document text on stdin, Folio | KEEP OURS | M | Avoids a 400k character copy | Agent asks for pages through the CLI (D5) |
| 19 | Untrusted-document prompt with JSON framing, Folio `ai-prompt.cjs` | ADOPT | S | None | Sound prompt-injection hygiene |
| 20 | Regex citations, Folio | KEEP OURS | S | None | Use structured page references (Rule 13) |
| 21 | Dark mode keeps images in colour, Folio | ADOPT-ADAPTED | M | Near zero when done in the compositor | Apply a mask of engine image rects at composite time; no second bitmap; no cost when off |
| 22 | iframe or process per tab, Folio | KEEP OURS | n/a | Avoids its largest cost | Rust owns document state, so isolation is not needed |
| 23 | 300 MB and 10,000-page refusals, Folio | KEEP OURS | M | Forces O(1) per-page state | Rule 18 |
| 24 | Confirm before following any link, Folio 1.10.8 | ADOPT | S | None | Invisible full-page links are real |
| 25 | XFA off, no PDF JavaScript, Folio | ADOPT | S | Saves | Reduces attack surface. Sumatra now runs form JS; we need not |
| 26 | Page label sanity checks, Sumatra `EngineMupdf.cpp:3204-3233` | ADOPT | S | None | Real files have junk labels |
| 27 | Signature reporting: DocMDP, covered bytes, later signatures, Sumatra `:8442-8529` | ADOPT | M | Small | MuPDF's own check is too weak (Rule 15) |
| 28 | Memory profile choices, Okular | KEEP OURS | n/a | n/a | One budget, adaptive, config value only |
| 29 | Extended attribute notes, Skim | KEEP OURS | n/a | n/a | Rejected: lost by normal file handling |
| 30 | PDFKit as writer | KEEP OURS | n/a | n/a | Rejected: macOS 27 damage (Rule 16) |
| 31 | Release discipline and off-screen smoke, Folio AGENTS.md | ADOPT | S | None | Section 4.8 |
| 32 | Full-copy version history, Folio | KEEP OURS | M | Saves disk | One pre-save backup, bounded (D2) |

## 7. Benchmark targets

### 7.1 What exists

No rigorous independent benchmark was found. Everything here is single-reporter or vendor-run.

| Source | Measured | Grade |
|---|---|---|
| [PDNob vendor test](https://www.pdnob.com/read-pdf/fastest-pdf-reader-for-large-files.html), 2026-11-04, Windows 11, i7, 16 GB, 286 MB vector PDF. RAM tool unnamed; its intro contradicts its own table | Cold start and RAM: Sumatra 2.1 s, 168 MB. PDF-XChange 3.7 s, 312 MB. Foxit 4.6 s, 398 MB. Edge 5.5 s, 512 MB. Acrobat 9.8 s, 824 MB | L |
| [sioyek #12](https://github.com/ahrm/sioyek/issues/12), 2021 | 296 MB on a 98 KB PDF, Windows (since improved) | T |
| [sioyek #762](https://github.com/ahrm/sioyek/issues/762), 2023, open | 15-page PDF on a Mac: 200 MB to 1.2 GB in an hour, worse on Retina | T |
| [sioyek #693](https://github.com/ahrm/sioyek/issues/693), 2023, open | 3 to 4 GB after 24 h | T |
| [Zathura #197](https://github.com/pwmt/zathura/issues/197), 2020 | 200 MiB, 2,000 pages: first search hangs 50 s, 2.8 GiB | T |
| [Zathura #132](https://github.com/pwmt/zathura/issues/132), 2019, open | Search uses 4 GiB | T |
| [SumatraPDF #1494](https://github.com/sumatrapdfreader/sumatrapdf/issues/1494), 2020 | 80 MB file: search reaches 3 GB | T |
| [SumatraPDF #1393](https://github.com/sumatrapdfreader/sumatrapdf/issues/1393), 2020, fixed | 3.8 MB to 800 MB through repeated zoom | T |
| [KDE forum](https://forum.kde.org/viewtopic.php%3Ff=251&t=92378.html), undated in result | Contradicts the others: Acrobat lowest. Shows how unreliable this data is | L |

Folio has no published numbers.

### 7.2 Proposed budgets, for R10 to confirm

Reasoning: one A4 page on a 5K Retina window is about 2,000 by 2,800 px, or 22 MB. A 60 MB idle budget allows that page
and its neighbours plus a small parser state.

| Metric | Reference | Proposed target | How measured |
|---|---|---|---|
| Cold start to first page, 10 MB text PDF | none usable | < 300 ms | signpost timing |
| First page, 286 MB vector PDF | Sumatra 2.1 s [L] | < 1 s | same fixture class, generated |
| Open 1 GB, 50,000-page PDF | Folio refuses | Opens, first page < 1 s, memory unchanged by page count | fixture |
| `phys_footprint`, one 10 MB document, idle | Sumatra 168 MB on a much bigger file [L] | ≤ 60 MB | `footprint` |
| **One more background tab** | Folio: a full stack per tab [A] | ≤ 64 KB, 2 tabs vs 50 tabs | `footprint` diff |
| Peak with 50 tabs and heavy use | Sumatra about 2 GB theoretical [A] | ≤ 300 MB, configurable | `footprint` |
| Full search, 5,000 pages | 2.8 GiB at 2,000 pages [T] | ≤ 64 MB above idle, first hit < 100 ms when cached | `footprint`, timing |
| Search or render cancel | Sumatra #6362 blocked until done | < 50 ms | test harness |
| Input to frame, any time | n/a | < 16 ms; a placeholder if the page is not ready | Instruments |
| Soak, 30 days simulated, with sleep and clock steps | sioyek #693, #1441 | Flat ±5%, no crashes | soak rig |

### 7.3 Reproducible next step

A single script should generate the fixtures, launch each app, wait for quiet, and record `footprint --pid`,
`/usr/bin/time -l`, and first-page time, three cold runs each, median reported. Preview needs no install. Skim, sioyek
and Folio do, which I did not do. SumatraPDF needs Windows. See D10.

## 8. Failure-derived design rules

Each rule becomes a permanent test before the related code exists. Each test also needs a positive control: a
deliberately bad implementation that must fail it (`rust_app_development.md` §1.2, R4). Fixtures must be generated or public; never
user files.

**1. A save either completes or leaves the original byte-identical.** Write to a temp file in the same directory, fsync,
reopen and validate, then rename atomically. For an in-place append, verify the file on disk still matches what we
loaded first, as Sumatra does (`EngineMupdf.cpp:10096`).
- Evidence: [SumatraPDF #3551](https://github.com/sumatrapdfreader/sumatrapdf/issues/3551) saving annotations corrupts
  (2023-06-24, 35 comments). [#2173](https://github.com/sumatrapdfreader/sumatrapdf/issues/2173) and
  [#2227](https://github.com/sumatrapdfreader/sumatrapdf/issues/2227) crash and corruption in a Google Drive folder
  (2021). [Okular 258475](https://bugs.kde.org/show_bug.cgi?id=258475) hours of annotations lost, file unusable
  (2010-12-01). Code still says `TODO: if the file is locked, this might fail` (`EngineMupdf.cpp:10106`).
- Test: fault injection at every write and fsync (`ENOSPC`, `EIO`, kill). After each: original is byte-identical, or the
  new file validates with an independent parser. Sync-folder simulation replaces the file during the write.

**2. The UI reports what really happened.** No success message before confirmation. On failure the dirty state remains.
- Evidence: [Folio #4](https://github.com/Forward-Future/folio/issues/4) Save did nothing (2026-10-30).
  [Folio #12](https://github.com/Forward-Future/folio/issues/12) "Copied" with no clipboard change (2026-10-30).
  [SumatraPDF #1723](https://github.com/sumatrapdfreader/sumatrapdf/issues/1723) no error on failed save (2020-08-04).
- Test: inject failures into save, export, print, copy. Assert an error event, dirty still set, and no success event
  through the agent interface.

**3. Every change is tracked, and no exit path loses work silently.**
- Evidence: [Folio #36](https://github.com/Forward-Future/folio/issues/36) form text (2026-10-31). Folio 1.10.5 printing
  cleared the flag. [SumatraPDF #4740](https://github.com/sumatrapdfreader/sumatrapdf/issues/4740) drag tab to window
  loses annotations (2024-12-29), fixed only by disabling the drag.
  [#3954](https://github.com/sumatrapdfreader/sumatrapdf/issues/3954) lost on tab close (2023-12-12).
  [#5292](https://github.com/sumatrapdfreader/sumatrapdf/issues/5292) lost on save (2026-01-25).
- Test: a generated matrix of every edit type (highlight, note, ink, text box, form field, signature stamp, page edit)
  against every exit (close tab, close window, move tab, quit, crash restart, reload, open over, hibernate, print, export).
  Each cell must end saved, or prompted. Quit shows one sheet with a per-file choice
  ([#5647](https://github.com/sumatrapdfreader/sumatrapdf/issues/5647)).

**4. External changes never destroy unsaved work, and our own writes never trigger a reload.** Identify our writes by
the fingerprint we wrote, not a flag.
- Evidence: [SumatraPDF #4612](https://github.com/sumatrapdfreader/sumatrapdf/issues/4612) unsaved annotations vanish on
  refocus (2024-11-01). [Okular 269819](https://bugs.kde.org/show_bug.cgi?id=269819) reload destroys annotations (2011).
  [Okular 379296](https://bugs.kde.org/show_bug.cgi?id=379296) self-save triggers reload.
  [Okular 437774](https://mail.kde.org/pipermail/okular-devel/2021-May/039864.html) same, as a race (2021-05-28).
  Sumatra re-arms a flag to skip the watcher event (`SumatraPDF.cpp:4995-4998`).
- Test: while dirty, change the file externally: state becomes conflict, both versions preserved, user chooses.
  1,000 saves produce zero reloads, including with delayed watcher events.

**5. Reload is debounced, validated, position-preserving and crash-proof.** Keep the old document live until the new one
parses. Restore by page and offset. Handle a shrinking page count, and clear stale labels and outline.
- Evidence: [sioyek #1699](https://github.com/ahrm/sioyek/issues/1699) crash swapping with the TOC open (2026-10-05).
  [Zathura #603](https://github.com/pwmt/zathura/issues/603) position lost (2024-06-18).
  [#973](https://github.com/pwmt/zathura/issues/973) regression, jumps to page 1 (2026-07-08).
  [#729](https://github.com/pwmt/zathura/issues/729) segfault (2025-02-23).
  [SumatraPDF #2252](https://github.com/sumatrapdfreader/sumatrapdf/issues/2252) label stale after reload (2021-11-17).
  [Skim thread](https://email.esm.psu.edu/pipermail/macosx-tex/2022-June/058343.html) repeated prompts (2022) [L].
- Test: a writer rewrites the open file in many partial states, with TOC, search and thumbnails active. No crash. Final
  position is correct. Page count grows and shrinks. Labels and outline match the new file. Latency is under budget.

**6. Never block other programs from the file.** On Windows use all three share flags, and hold no directory handles.
On POSIX, do not access an mmap that a truncation can turn into `SIGBUS`; read through a handle and treat short reads as
"changed". [A]
- Evidence: [Adobe thread](https://community.adobe.com/t5/Acrobat-Reader/Disable-file-lock-on-Adobe-Reader-DC-Windows-10/m-p/9164368)
  exclusive write access, "will not be changed" (2017) [L].
  [pdfTeX list](https://tug.org/pipermail/pdftex/2003-January/003497.html) same since 2003 [L].
  [Okular request](https://discuss.kde.org/t/paid-request-okular-don-t-lock-file-for-overwrite/49519) (2026-08-19) [L].
  [SumatraPDF #556](https://github.com/sumatrapdfreader/sumatrapdf/issues/556) folder locked (2016).
  [#5052](https://github.com/sumatrapdfreader/sumatrapdf/issues/5052) folder rename fails after close (2025-08-02).
  [#1530](https://github.com/sumatrapdfreader/sumatrapdf/issues/1530) other formats (2020).
- Test: a second process overwrites, truncates, deletes, renames the file and renames its folder while open, on every OS.
  All succeed. The app shows "changed" or "missing" and does not crash. After closing, no handles remain.

**7. Bound render memory in bytes, globally, across all tabs.**
- Evidence: `RenderCache.h:14-15` TODO. [sioyek #762](https://github.com/ahrm/sioyek/issues/762) and
  [#693](https://github.com/ahrm/sioyek/issues/693) open since 2023. pdf.js allows 134 MiB per canvas.
- Test: 50 tabs, scroll, zoom, thumbnails, on page sizes up to A0. `phys_footprint` stays under budget. Internal cache
  byte counters must match allocator totals within a tolerance.

**8. Background tabs cost kilobytes.** Hold path, identity, position and zoom, then reopen on demand. Keep only the
active tab and a small recent set live.
- Evidence: Folio `documentTabs.ts:76,170`. Sioyek `cached_documents` (`document.cpp:1224-1229`). Sumatra has no unload.
  [PDF-XChange forum](https://forum.pdf-xchange.com/viewtopic.php?p=79936) memory errors with tabs open [L].
- Test: marginal cost of tab 50 vs tab 2 is at most 64 KB. Switching back restores page, offset, zoom and TOC state, within
  budget. Unsaved work is never hibernated away (Rule 3 matrix cell).

**9. Zoom cannot allocate without limit.** Cap the render area, tile beyond it, and on allocation failure reduce quality
instead of failing.
- Evidence: [sioyek #568](https://github.com/ahrm/sioyek/issues/568) crash from uncapped helper-window zoom (2022-12-27).
  [Zathura #449](https://github.com/pwmt/zathura/issues/449) (2024-02-27) and
  [#253](https://github.com/pwmt/zathura/issues/253). [SumatraPDF #1393](https://github.com/sumatrapdfreader/sumatrapdf/issues/1393).
- Test: zoom to the max on a huge page while a test allocator fails at random. No crash. Memory is under budget. Visible
  content stays usable.

**10. Search streams pages and never holds the whole document in RAM.** Bounded, cancellable, off the UI thread, with
results as they arrive.
- Evidence: [Zathura #197](https://github.com/pwmt/zathura/issues/197), [#132](https://github.com/pwmt/zathura/issues/132),
  [SumatraPDF #1494](https://github.com/sumatrapdfreader/sumatrapdf/issues/1494) (`fz_new_display_list_from_page` caches
  pixmaps, per its dev note). pdf.js `pdf_find_controller.js:602`. sioyek `document.cpp:1425`.
- Test: search a 5,000-page image-heavy file. Peak delta at most 64 MB. New query cancels the old within 50 ms. Input latency
  stays on budget. Encrypted documents leave no plaintext cache.

**11. Search finds what a reader sees, and highlights land on it.** Normalize NFKC and ligatures, honour `/ActualText`,
join line-end hyphens but keep compound hyphens, fold diacritics by option, handle RTL in logical order, and keep a
reversible offset map to glyphs.
- Evidence: [pdf.js #18662](https://github.com/mozilla/pdf.js/issues/18662) `ff` not found (2024-08-28).
  [#12237](https://github.com/mozilla/pdf.js/issues/12237) `/ActualText` ignored (2020, open).
  [#22410](https://github.com/mozilla/pdf.js/issues/22410) ligature glyph names (2026-10-31).
  [#11752](https://github.com/mozilla/pdf.js/issues/11752) hyphenated words (2020).
  [#19120](https://github.com/mozilla/pdf.js/issues/19120) wrong highlight after normalization (2024-11-28).
  [sioyek #724](https://github.com/ahrm/sioyek/issues/724) `field` read as `feild` (2023).
  [#858](https://github.com/ahrm/sioyek/issues/858) Arabic finds nothing (2023).
  [#1551](https://github.com/ahrm/sioyek/issues/1551) Hebrew matches across words (2026-02-06).
  [SumatraPDF #6499](https://github.com/sumatrapdfreader/sumatrapdf/issues/6499) reversed Arabic ligatures (2026-11-06, open).
  [#6485](https://github.com/sumatrapdfreader/sumatrapdf/issues/6485) (2026-11-05).
- Test: a golden corpus with queries, expected match counts and expected highlight rectangles, covering each case above.
  Copy text uses the same corpus. Include false-match checks: `well-known` across a line stays hyphenated.

**12. Physical page and label are separate types everywhere.** The UI shows both when they differ. Input accepts both
explicitly. Do not trust labels blindly.
- Evidence: [pdf.js #8225](https://github.com/mozilla/pdf.js/issues/8225) (2017),
  [#17162](https://github.com/mozilla/pdf.js/issues/17162) wrong page order reports (2023).
  [Zathura #245](https://github.com/pwmt/zathura/issues/245) (2021, open).
  [sioyek #1132](https://github.com/ahrm/sioyek/issues/1132) offset not persisted (2024, open).
  [SumatraPDF #1144](https://github.com/sumatrapdfreader/sumatrapdf/issues/1144) labels corrupt when encrypted (2019),
  [#2252](https://github.com/sumatrapdfreader/sumatrapdf/issues/2252). Junk labels: `EngineMupdf.cpp:3204-3233`.
  Folio has none (grep for `getPageLabels` finds nothing).
- Test: fixtures with roman front matter, restarts, duplicates, per-page prefixes, `.pdg` names and encrypted labels. Go to
  label and go to page each land correctly. Duplicates ask. Junk labels fall back to physical. Agent output has both.

**13. AI citations are structured and checked.** The agent cites through a defined page reference, resolved against the
document. Out-of-range shows as invalid, never clamped. A citation can carry a quote, which is checked against that page.
- Evidence: `AiMarkdown.tsx:25`, `App.tsx:621`, and `ask-ai.png`.
- Test: responses with multiple pages, ranges, labels, out-of-range and wrong quote. Each is linked or flagged correctly.
  Document text with a fake citation or instruction is inert.

**14. The TOC is lazy, follows position, and survives everything.** Expansion state per tab. Virtualised rows. All
destination types. Never rebuilt on scroll.
- Evidence: [sioyek #456](https://github.com/ahrm/sioyek/issues/456) does not follow reading (2022, open).
  [#1713](https://github.com/ahrm/sioyek/issues/1713) navigation broken (2026-11-02).
  [#1216](https://github.com/ahrm/sioyek/issues/1216) encrypted file (2024).
  [#1514](https://github.com/ahrm/sioyek/issues/1514) RAM crash in one package (2025).
  [Zathura #767](https://github.com/pwmt/zathura/issues/767) ToC CPU and memory leak (2025-07-17).
  [SumatraPDF #6190](https://github.com/sumatrapdfreader/sumatrapdf/issues/6190) wrong jumps, regression (2026-09-15).
  [#6134](https://github.com/sumatrapdfreader/sumatrapdf/issues/6134) no refresh after jump (2026-09-04).
  Folio holds `open` in per-node React state with no current-section link (`Sidebar.tsx:93-94`).
- Test: a 50,000-entry outline at depth 20 stays under frame budget. Named, explicit, `/XYZ` null and remote destinations
  land correctly. Tab switch keeps expansion. Collapse under the active tab does not change other tabs. Cycles and broken
  outlines do not hang.

**15. Saving never invalidates a signature unless the user chooses to, knowingly.** Detect signatures and DocMDP before
any edit. Allow only incremental appends on signed files. If incremental is impossible, refuse in-place save and offer a
copy with a clear warning. Do not rely on a file-length check.
- Evidence: Sumatra notes MuPDF's "changed since signing" only compares lengths (`EngineMupdf.cpp:8502-8504`) and falls
  back to a full rewrite when incremental is unavailable (`:10140`). Sioyek export is a full rewrite
  (`document.cpp:2625`). [pdf.js #21698](https://github.com/mozilla/pdf.js/issues/21698) no modified-after-signing
  indicator (2026-08-03, open). Folio warns only inside a collapsed details panel (`SignatureDialog.tsx:204`), though it
  does block its editor on signed files (`App.tsx:1421`).
  [WKU KB](https://td.wku.edu/TDClient/KB/ArticleDet?ID=2886) any change voids it [L].
- Test: signed, certified and multi-signed fixtures. After annotate and save: bytes up to the original end are identical,
  and an external validator accepts the original revision. DocMDP no-changes refuses. Positive control: a rewriting saver
  must fail.

**16. Round-trips change only what the user changed.** PDFKit does not write our files. Preview is not a test oracle.
- Evidence: [Apple 847316](https://developer.apple.com/forums/thread/847316) Preview and PDFKit rewrite `/AP`, leave `/AS`,
  widgets vanish, damage is permanent. Filed as FB24866826, still on 27.2. [Apple 847155](https://developer.apple.com/forums/thread/847155)
  pixelated widgets (September 2026). [pdf.js #22125](https://github.com/mozilla/pdf.js/issues/22125) `extractPages` drops
  catalog entries (2026-10-09, open). [pdf.js #15096](https://github.com/mozilla/pdf.js/issues/15096) radios on save (2022).
- Test: object-level diff of before and after. Only expected objects differ. Checkbox `/AS` matches `/V` and `/AP /N` keeps
  state keys. Metadata, layers and `/Lang` survive. Reopen in at least two independent engines.

**17. Notes survive normal file handling and are never trapped.** Identify by content and path. A changed or moved file
either rejoins its notes or reports them as orphaned, never dropped. Export and embed are always available.
- Evidence: [Skim manual](https://skim-app.sourceforge.io/manual/SkimHelp_37.html) email loses notes.
  [Skim FAQ](https://sourceforge.net/p/skim-app/wiki/FAQ). [Folio #11](https://github.com/Forward-Future/folio/issues/11).
  [sioyek #1129](https://github.com/ahrm/sioyek/issues/1129) moving annotations between similar files (2024).
  [DEVONthink thread](https://discourse.devontechnologies.com/t/pdf-annotations-disappeared-suddenly/58668) PDF Expert
  after rename [L].
- Test: rename, move, copy, replace, rebuild with changed bytes, and cloud-placeholder cases. Notes appear or are listed as
  orphaned. Database crash recovery. Reopen-restore for every mark type, including empty ones.

**18. Size limits are not product limits.** Per-page state is O(1), allocated when visible. No per-page widgets.
- Evidence: `pdfInput.ts:1-3`, `document-files.cjs:7`. pdf.js `PagesCountLimit`.
  [pdf.js #10565](https://github.com/mozilla/pdf.js/issues/10565) out-of-memory (2019, open).
- Test: 1 GB and 50,000-page fixtures open within budget. Memory does not scale with page count. Corrupt huge page trees
  fail cleanly with a timeout.

**19. The UI thread does no I/O, parsing, rendering or `stat`.**
- Evidence: [Folio #3](https://github.com/Forward-Future/folio/issues/3).
  [SumatraPDF #6369](https://github.com/sumatrapdfreader/sumatrapdf/issues/6369) per-button file checks stall over SMB
  (2026-10-20). [#6362](https://github.com/sumatrapdfreader/sumatrapdf/issues/6362) slow page blocks navigation (2026-10-18).
  [#5857](https://github.com/sumatrapdfreader/sumatrapdf/issues/5857) slow scroll regression (2026-07-31).
- Test: a slow virtual file system (2 s per read, or hanging) with a watchdog on the UI thread. Input stays on budget.
  Page keys during a slow render take effect at once.

**20. Heavy pages do not re-interpret for every tile.** Keep a bounded display list per page or a similar strategy.
- Evidence: [SumatraPDF #3842](https://github.com/sumatrapdfreader/sumatrapdf/issues/3842): 44 MB stream, 1.3 million
  segments, re-interpreted per tile (2023 to 2026-10-11). [#6402](https://github.com/sumatrapdfreader/sumatrapdf/issues/6402)
  an optimization then caused a colour bug (2026-10-22), so speedups need image comparison too.
- Test: generated vector-heavy fixture with zoom and pan latency budget. Pixel diff against a reference at each change.

**21. Long sessions, sleep and clock changes are normal.** Monotonic clocks only for durations. Flat memory over time.
- Evidence: [sioyek #593](https://github.com/ahrm/sioyek/issues/593) scroll locks when time-of-day steps back (2023; repro
  in the last comment). [#1441](https://github.com/ahrm/sioyek/issues/1441) hangs after sleep cycles (2025-07-24).
  [#693](https://github.com/ahrm/sioyek/issues/693).
- Test: injected clock with backward steps, midnight and sleep. A compressed soak with memory slope near zero. A lint bans
  wall-clock types in timing code.

**22. Paths and PDF strings are bytes until proven otherwise. Input is hostile.**
- Evidence: [sioyek #1690](https://github.com/ahrm/sioyek/issues/1690) SIGABRT in UTF-8 decode (2026-09-24).
  [#1411](https://github.com/ahrm/sioyek/issues/1411) (2025). [#1036](https://github.com/ahrm/sioyek/issues/1036) special
  characters (2024). Folio 1.10.8 invisible links, 1.10.2 reserved Windows names. Foxit
  [CVE-2025-55312](https://www.wiz.io/vulnerability-database/cve/cve-2025-55312) page-change memory corruption [L].
- Test: fuzz parser and paths in CI. Non-UTF-8 names round-trip. A full-page invisible link requires confirmation. PDF
  JavaScript never runs. Reserved names cannot be written.

**23. Actions work whatever the page's load state.** Commands queue against document state, not on-screen state.
- Evidence: Folio changelog 1.10.5 and 1.10.7, PRs [#30](https://github.com/Forward-Future/folio/pull/30) and
  [#34](https://github.com/Forward-Future/folio/pull/34). 1.10.3 export missed unvisited pages.
- Test: highlight, export, print and search on pages never rendered or mid-render. Output contains all marks.

**24. Caches are bounded and private.** Do not keep copies of user documents. Encrypted documents leave nothing on disk.
Storage is visible and clearable.
- Evidence: [Folio #5](https://github.com/Forward-Future/folio/issues/5) 2.4 GB. [Folio #37](https://github.com/Forward-Future/folio/issues/37).
- Test: open and close a password file, then scan the app's directories for its text and thumbnails. The cache size cap
  holds under churn. Clearing removes every entry.

## 9. [DECIDE] items

**D1. Where do highlights and notes live by default?**
- Options: A) in the PDF only, standard but rewrites the user's file. B) private sidecar only, as Folio does; safe but
  invisible to other apps. C) sidecar autosave, then written into the PDF on explicit Save.
- Recommendation: C. No work lost, file untouched until asked, standard on Save.
- Cost to change later: moderate. A migration is simple, but users learn the behaviour quickly.

**D2. What does Save do?**
- Options: A) always save a copy, as Folio does; safest, but files multiply. B) in place, atomic, incremental when
  possible, with one bounded backup of the original on first save. C) in place with no backup.
- Recommendation: B. Signed files default to a copy (Rule 15).
- Cost to change later: low in code, high in user trust if a save ever damages a file.

**D3. How many documents stay live?**
- Options: A) all, like everyone else. B) active plus a byte budget, least recent freed first. C) active only.
- Recommendation: B, with the active tab and at most two recent ones live. Dirty tabs stay live or are persisted first.
- Cost to change later: low, if the design assumes a tab can always be freed. High if it does not, so decide now.

**D4. Does search keep an index on disk?**
- Options: A) none, scan each time. Simple, slow on large files. B) ephemeral in memory. Fast, breaks the budget.
  C) per-document cache on disk, size-capped, deletable, none for encrypted files.
- Recommendation: C, built after streaming search works. Also serves the agent CLI.
- Cost to change later: low. It is a cache.

**D5. How does AI get document text?**
- Options: A) send the full text, as Folio does (400k character cap). B) the agent requests pages and search results
  through `recto` tools. C) both, where B is the default.
- Recommendation: B. Works on any size, keeps memory flat, and matches the agent CLI goal. Enabling tools for an
  untrusted document needs care: read-only tools and no write access unless the user asks.
- Cost to change later: moderate. The CLI contract is the expensive part.

**D6. What is in scope for v1 from what Folio users request?**
- Options: A) reading, notes and signing only. B) add page rotate, delete, extract. C) add OCR.
- Recommendation: A for v1, then B next. OCR later, likely outside the core process.
- Cost to change later: low. Non-PDF formats are excluded here too.

**D7. What does `recto --page N` mean?**
- Options: A) physical page, `--label` for labels. B) label first, with a fallback. C) require explicit choice.
- Recommendation: A. Scripts stay deterministic. Output always includes both.
- Cost to change later: high once scripts exist.

**D8. How far do we go from Folio's look?**
- Options: A) native chrome only, system accent, nothing floats over the page. B) native with a custom accent. C) a custom
  web-like style.
- Recommendation: A. Fits "Rust decides, the host draws" (§2.1) and costs least memory. Render concepts before deciding.
- Cost to change later: high, since it touches every surface.

**D9. What does "signing" mean in v1?**
- Options: A) visual stamp only, as Folio does. B) cryptographic (PAdES) only. C) both, labelled clearly in the UI.
- Recommendation: C, stamp first. A user must never mistake a stamp for a certificate signature. Certificate storage needs
  the platform track (R5).
- Cost to change later: moderate. Incremental-save rules must hold from day one regardless.

**D10. May we install competitors for a measured benchmark?**
- Options: A) Mac only: Skim, sioyek, Folio, plus built-in Preview. B) also a Windows VM for SumatraPDF, Foxit, Acrobat.
  C) no installs, use only the weak published numbers.
- Recommendation: A now, B when Windows work starts.
- Cost to change later: none. It does delay confidence in the budgets.

## 10. Gaps

- **Nothing was measured.** Costs marked [A] come from reading code. Folio's real per-tab memory is unknown.
- **Closed apps are weak.** PDF Expert, Foxit and Acrobat rest on forum posts and CVEs. Their tab models and memory
  behaviour are unverified. A Foxit search returned almost only security advisories.
- **Okular, Skim, Zathura source was not read** at a pin, only docs and trackers.
- **Issue fixes were not re-verified.** "Closed" reflects the tracker. Line numbers on SumatraPDF are pre-release and may
  move before 3.7.
- **Searches were not exhaustive.** Trackers were searched by theme, and GitHub's relevance sort may have missed cases.
- **The rule that mmap risks `SIGBUS` is my analysis.** It needs a spike with whichever engine the engine track selects.
