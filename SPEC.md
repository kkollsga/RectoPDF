# RectoPDF specification

Started 2026-11-06. Research and reasons are in `docs/research/`, concepts in `docs/concepts/`, the sign-off summary in
`docs/SIGNOFF.md`. **[DECIDE]** marks a question only the owner can answer. Items marked *default* follow a recommendation the owner
hasn't answered yet: "Lets start the build out" was taken as a go on those, and they can be overruled.

## 1. Product

A PDF reader for the Mac (Windows later) that stays fast and small however many large PDFs are open, offers the tools people
usually pay for, and lets an agent search a document instead of receiving all of it. App: **RectoPDF**. CLI: `recto`.

**Must be excellent:**
1. Memory and speed with many large PDFs open.
2. A calm screen with the maximum vertical reading space.
3. An agent that requests what it needs.
4. Saving that never damages a file or a signature.

**Never:** claim a signature is "valid" without checking it; leave content recoverable after claiming to remove it; send anything
off the machine unasked; change the user's file without an explicit save.

## 2. Performance budgets

A missed budget is a bug. The whole process tree counts, on release builds, on an M4-class Mac. Details in the platform report, §4.

| Metric | Target | Status |
|---|---|---|
| **One more idle tab** | **≤ 4 KiB, 0 file handles, 0 threads, 0 layers, 0 tiles**, whatever the PDF's size | Tested for heap and handles; layers and tiles when they exist |
| 1,000 tabs | ≤ 4 MiB over 2 tabs; sidebar opens < 150 ms | Partly tested |
| Live documents | Active plus at most 2 recent; a byte budget frees the oldest first | Tested for the count; byte budget when renderers exist |
| One document, text page | ≤ 60 MiB | Pending app |
| Colour full-bleed page | ≤ 100 MiB | Pending app |
| Fast scrolling, peak | ≤ 170 MiB | Pending app |
| Cold start to first sharp page | ≤ 300 ms p50, ≤ 500 ms p95 | Pending app |
| Open 2 GB, 10k pages | Page 1 sharp ≤ 250 ms warm, ≤ 600 ms cold; main never stalls > 16 ms | Pending app |
| Idle CPU | ~0%, ≤ 1 wake-up per second | Pending app |
| Scroll | 120 fps on ProMotion, ≥ 99% of frames on time, 0 blank frames on seen pages | Pending app |
| Sharp after scroll stops | ≤ 100 ms on a text page | Pending app |
| Background work (OCR, indexing, compress) | Helper process, background QoS, exits when done | Pending |
| Window closed | ≤ 20 MiB | Pending app |

## 3. Architecture

**Rust decides, the host draws.** A native AppKit host (no SwiftUI in the main window) over a Rust core through UniFFI. WinUI 3 later.
*Rejected:* Rust GPU toolkits (a Metal swapchain adds 105–184 MiB and loses native accessibility and IME); Tauri and Electron (a blank
webview is 56.7 MiB across four processes); PDFKit as the viewer (3–5 s main-thread stall at 10k pages, and it damages forms on save).

**Three tab tiers:** *Cold* is a record with no handle. *Warm* adds its contents and page table. *Hot* is the visible tab. Promotion is
fast because everything is rebuilt from disk.

**Two engines, one writer.**
- **PDFium without V8 or XFA** draws pages and supplies selection, search text and highlight positions. It runs in a sandboxed,
  killable worker behind a `RenderBackend` seam. Measured in `docs/research/2026-11-06-renderer-spike.md`: text and vector pages at 2x
  are p50 2.06 ms, p95 8.64 ms. Cancelling isn't always prompt and one hostile file reached 4 GiB, so the worker gets a **hard memory
  limit, a per-page deadline with kill, and retirement at 64 MiB** (*default*, amends the benchmark's rule).
  *Rejected:* MuPDF (AGPL), poppler (GPL). hayro stays the challenger: as fast, pure Rust, but no cancel call and it reached 843 MiB where PDFium peaks at 113.
- **Search text needs a normaliser.** PDFium returns line-end hyphens as U+FFFE, plus NBSP and CRLF, which defeat plain `rg`.
- **PDFium returns hidden text** (14 of 17 techniques, and real files contain it), so hidden text is flagged for the agent as untrusted, never dropped silently, because an OCR layer is invisible too.
- **The lopdf fork's `IndexedReader`** owns file structure with no renderer running. Its append-only writer is the only code that
  writes files. *Rejected:* upstream lopdf, which has no indexed reader; PDFium's writer, which would be a second writer.
- **Agreement gate:** when the two parsers disagree about a file, it stays readable but saving and signing turn off.

**distillPDF is parked** (owner, 2026-11-06). Its Tesseract and invisible-text approach is reused for OCR without depending on it.

**Pages reach the screen** as `CALayer`s holding IOSurface tiles, 512 px, zero copy, off-screen tiles purgeable. See the platform report, §2.

## 4. Screen design

Reference: `docs/concepts/`. Rules from the owner:

- **No top bar.** Maximum vertical reading space; nothing sits above or over a page.
- **Tabs live in the sidebar.** Tapping the active PDF folds or opens its contents.
- **At rest the sidebar is only names and contents.** The rail fills as you read. Chevron and page numbers appear on hover.
- **The macOS menu bar is the home of every command** (owner, 2026-11-06: "Not all options need to be in the sidebar. A lot can be in the macos top bar instead"). Menus: File, Edit, View, Go, Pages, Mark Up, Tools, Window, Help. Map: `docs/concepts/menumap-light.png`. Rust decides which items exist and which are enabled. ⌘K finds any command.
- **The `⋯` panel holds only what you touch while reading** (owner: "let the … menu toggle open a compact menu in lower part of left sidebar with tabs"). It opens in the lower sidebar with two tabs, View (zoom, paper, layout) and Mark up, plus an **Edit PDF** button. A tool started from the menu bar that needs choices, such as Compress, opens here, in place of the tabs.
- **Editing tools show in the sidebar only in edit mode** (owner, 2026-11-06: "These are tools in the sidebar only visible when in edit mode"). The panel changes to the edit tools when you enter with ⌘E, the Pages menu or Edit PDF, and Done returns to reading. In the Pages menu, Rust enables those items only in edit mode.
- **Saving looks and works like a native Mac app** (owner, 2026-11-06: "I want something that looks more like a native save / save as feature. Like word etc"). With unsaved changes, the red window button shows its dot and a Save icon appears beside the window controls. `⌘S` saves, `⇧⌘S` and File > Save As open the system's save sheet, and closing asks Don't Save, Cancel or Save. Tools change the open document (Compress has Apply) and never have their own save buttons.
- **Agent icon at the top right**, in the empty margin. It opens the agent panel and stays in the same spot.
- **Dark mode offers dark or white paper.** Signing always shows the true page.
- **Focus view:** sidebar hidden, page number only, agent icon kept.
- A neutral signature mark, never a green tick in v1.

## 5. Agent

- The app starts the user's **existing `claude` install** if there is one, and never handles credentials. If it's missing, the panel shows
  setup steps and the reader works as normal.
- **The agent requests what it needs.** The first prompt holds facts about the PDF (size, pages, text status, outline) plus the question,
  the current page number and any selection. Page text is never included.
- Tools limited to `Read` and `Grep` over a per-document folder, in a fully locked-down run. Never `Bash(rg ...)`: `rg --pre` executes text
  that comes from the PDF. Exact arguments: agent report, §1.
- `recto` CLI on a local socket with levels Off, Read-only, Control and Full (*default:* Off). Signing needs a human gesture and is never
  reachable by an agent.
- [DECIDE] Whether a public release waits for Anthropic's written answer on subscription use. *Default:* yes, with an API-key route too.

## 6. Scope: everything in one build

"We build everything in one go, but we test as we build." Build order follows dependencies only.

| Group | Features |
|---|---|
| Reading | Lazy open, tabs, contents, Find, paper modes, focus view, session restore |
| Agent | Panel, corpus, `recto`, generated skill |
| Annotations | Highlight, underline, strikeout, note, ink. Notes autosave beside the file, and go into the PDF on Save (*default*) |
| Forms | Filling, owned by the Rust core and lopdf (*default*). No JavaScript; detect and explain. Dynamic XFA detected only (*default*) |
| Signing | **Placeholder for now** (owner, 2026-11-06): the labelled image option, a neutral "signature present, not checked" report, and the final seams in `recto-sign`. No file is signed cryptographically until it's built properly to the official standards (ISO 32000-2, ETSI EN 319 142, RFC 5652, RFC 3161) |
| Edit PDF | **An edit mode** (owner, 2026-11-06: "pdf edit mode that lets you edit text, drag and drop pdfs into the app to add more pages to an existing pdf, rearrange pages, turn/rotate individual pages"). **Pages:** the page area becomes a grid; drag to rearrange, drop PDFs to add their pages, rotate, delete, extract, split. Edits are a small list applied on Save: instant, about 8 B a page, and undoable. **Text:** click text and type. Lines rewrap inside their text block, as in Foxit's Edit Text; other blocks don't move, and nothing flows across pages. It keeps the PDF's font; a character the font lacks uses an embedded fallback font. See `docs/research/2026-11-06-foxit-editing.md` |
| Security | Password protection |
| Saving | **Save** and **Save As**, atomic, never partly written. Adding things appends to the file. **Removing things (delete a page, redact, compress) rewrites it, so the old data is really gone.** If a change would break a signature, Save is off and Save As explains why. One backup of the original on first save (*default*) |
| Compare | Text first, read-only |
| Compress | **Low, Medium, High to a fixed image DPI, then save.** Start: 220, 150, 96 DPI. Measured at printed size; never enlarges, leaves text, vectors and 1-bit scans alone; cleans the file first. Apply changes the open document, then it saves normally |
| OCR | Tesseract, with **Save as searchable PDF**: an invisible text layer with an embedded Unicode font, in a helper process |
| Extraction | **Click an image to copy it losslessly.** **Snapshot tool:** drag a box for text, PNG, SVG and JPG, all on the clipboard at once |

**SVG is hand-rolled** (owner, 2026-11-06): a small streaming generator in `recto-svg` that writes as it walks the content stream, with no
document held in memory. Paths, images and text come first, then gradients, masks and blend modes. Anything SVG can't express is
embedded as a raster of just that element and reported, and content outside the snapshot box is left out rather than hidden.
*Rejected:* `hayro-svg` as a dependency, and PDFium, which has no SVG output as far as we know.

**JPG honesty:** a JPG is lossless only when it's the original bytes of one image. Anything else is re-encoded and says so.

| Added by the owner, 2026-11-06 | Redaction, remove hidden data, watermarks and Bates numbers, create form fields, measure tools |

Signature trust through EU lists comes later, with the signing work.

[DECIDE] Image copy and snapshot on PDFs whose permissions forbid copying (*default:* a notice with "Copy anyway"). Open-source readers differ: pdf.js and Evince ignore the flag by default, Okular obeys it with a setting to turn that off, and SumatraPDF blocks text copy but still copies as an image.

**Left out:** paragraph reflow and image replacement (for now), conversion to Word or Excel, creating PDFs from Office files, sending documents for signature,
print production, portfolios, 3D and media, V8.

## 7. Decision log

Numbered so they can be cited. Newest last.

1. **Name RectoPDF, CLI `recto`.** The name must say it's a PDF reader. Free on crates.io, PyPI, npm, Homebrew and GitHub on 2026-11-06; trademarks unchecked.
2. **Native AppKit over Rust.** Lowest measured memory.
3. **Developer ID, not the Mac App Store.** A sandboxed child can't see `rg` or `claude`.
4. **Use the existing `claude` install.** Owner: "Use the existing claude install if it exists."
5. **The agent pulls, the app never pushes text.** Owner: "That hands off the decisions to the agent."
6. **Your lopdf fork, plus PDFium.** Neither can do the other's job; upstream lacks the lazy reader.
7. **Park distillPDF.** Owner, 2026-11-06.
8. **No top bar.** Owner: "We need to maximize vertical reading space."
9. **One build.** Owner: "We build everything in one go, but we test as we build."
10. **Hand-rolled SVG.** Owner: "we can probably handroll an efficient svg generator". Streaming suits the memory goal.
11. **Mac first, Windows later** (*default*). The core is cross-checked for Windows from the first commit.
12. **Permissive or closed licence** (*default*). Rules out MuPDF and poppler.
13. **Signatures are a placeholder for now.** Owner: "lets do a signature place holder for now. We can build it out properly later using the official standards. So structure the code accordingly." A placeholder never produces anything that looks like a signature.
14. **All five extra tools are in this build.** Owner answered yes on 2026-11-06.
15. **PDFium confirmed for now** (*default*). Fast on 5,689 real pages; two lines of the benchmark's rule fail, so the rule is amended as in §3.
16. **Normal Save and Save As.** Owner, 2026-11-06. Replaces the earlier save-to-a-copy default.
18. **Edit mode is in, including text editing.** Owner, 2026-11-06. Reverses the earlier call to leave text editing out. Text edits work on a whole block, matching Foxit's baseline.
17. **Commands live in the macOS menu bar;** the sidebar panel keeps only what you use while reading. Owner, 2026-11-06.
