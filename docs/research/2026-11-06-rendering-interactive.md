# Rendering and interactive PDF: what draws the pixels, and how it lives with lopdf

**Track:** R7 (libraries) and R11 (spike specification), for RectoPDF (`recto`).
**Date:** 2026-11-06. **Status:** research finished, spike specified but not run.

This report contains no performance measurement of any renderer. Nothing was built, and no third-party binary was
downloaded or run. Numbers from other people are labelled as theirs.

---

## 1. The answer

**lopdf does not draw. Something else must, and whichever renderer we choose brings its own parser.** That cannot be
avoided today, so the design has to make two parsers safe.

**Recommended design:**

1. **Renderer: PDFium without V8 or XFA, in a separate, sandboxed, killable worker process.** It is the only candidate
   that has no licence problem, is run at Chrome's scale, and has a real pause callback, a custom read callback and
   matrix-plus-clip rendering. This is provisional until the spike has run (§9).
2. **hayro is the challenger,** behind the same `RenderBackend` seam. It is pure Rust, permissively licensed and moving
   fast. Today it has no cancellation, no overprint, no form behaviour, and several open reports of tiny files
   allocating gigabytes. Its use in development as a second opinion costs little.
3. **lopdf (the fork's `IndexedReader`) owns structure and is the only writer.** Outlines, links, page map, form model,
   signature inventory, agent tasks and all saves live in the Rust core.
4. **Form logic lives in Rust, not in PDFium's form engine.** The core owns field values and generates appearance
   streams. It appends a real incremental update, and the renderer reopens those exact bytes. What the user sees is then
   what would be saved, and the renderer can be swapped without rewriting forms.
5. **An agreement gate** compares what the two parsers report. If they disagree, the document stays viewable but saving
   and signing are off, and the UI says why.
6. **v1 makes no claim that a signature is valid.** It reports what is there and never says "valid". Claiming validity
   is what puts a viewer inside the shadow-attack class (§6.4).

**Key findings that shaped this:**

| # | Finding | Evidence |
|---|---|---|
| 1 | PDFium's public API is not thread-safe at all: one call at a time, across the whole library | `public/fpdfview.h:11-13` at chromium/8138 |
| 2 | PDFium's custom read callback works, but it is less lazy than it looks: every parsed stream's raw bytes are copied to the heap, and parsed objects stay cached | `cpdf_syntax_parser.cpp:854-866`, `cpdf_indirect_object_holder.cpp`. Read in source, **not measured** |
| 3 | hayro needs the file as one contiguous `&[u8]`, so `mmap` is the route. Its maintainer suggested exactly that. It resolves every page at open and keeps decoded object streams for the document's life | `hayro-syntax/src/data.rs`, `page.rs:49-56`, [#1345](https://github.com/LaurenzV/hayro/issues/1345), [#1659](https://github.com/LaurenzV/hayro/issues/1659) |
| 4 | Pure Rust does not remove the need for a worker. hayro has open reports of a 97-byte stream asking for ~95 GB and a 198-byte stream for 2 GB (reporters' figures, not ours) | [#1549](https://github.com/LaurenzV/hayro/issues/1549), [#1695](https://github.com/LaurenzV/hayro/issues/1695) |
| 5 | hayro has no form behaviour. It draws an existing `/AP` and nothing more | `hayro-interpret/src/interpret/mod.rs:165-240` |
| 6 | The lopdf fork has no form or signature code, and its incremental save is eager | No `AcroForm`, `ByteRange` or `NeedAppearances` in `src/`; `incremental_document.rs:4-14` holds all bytes and a full `Document` |
| 7 | 11 of the 141 files in the owner's own benchmark set are IRS hybrid forms: XFA packets plus usage rights (`/UR3`). They are not simple forms, and saving them is a real risk | Our byte scan (§9.3) |
| 8 | PDFium has no public API for toggling layers. An in-memory appended revision does the job | No OCG hits in the 19 public headers scanned at 8138 |

---

## 2. What was compared, at which version

| Candidate | Pinned at | Date | How it was read |
|---|---|---|---|
| PDFium | branch `chromium/8138`, commit `5f092b52`, release "PDFium 159.0.8138.0" | commit 2026-10-30, binary 2026-11-04 | Public headers and sparse source from pdfium.googlesource.com |
| bblanchon/pdfium-binaries | release `chromium/8138`; also `chromium/8125` | 2026-11-04; 2026-10-26 | GitHub API, `steps/05-configure.sh` |
| pdfium-render | crates.io 0.9.5; source read at master `f14b224` (declares 0.9.6, unpublished) | 2026-11-01 | Sparse source. **No git tags exist, so 0.9.5 itself was not read** |
| MuPDF | tag 1.28.6 (`da5f5eb8`); headers and `CHANGES` read at master `e6613569` (1.29.0-dev) | 2026-10-29; 2026-11-05 | Sparse source. **Master, not the tag** |
| `mupdf` crate | 0.8.0 | 2026-06-22 | crates.io metadata only |
| hayro | `hayro-v0.9.0`, commit `3f3825c3` | 2026-11-02 | Sparse source at the tag, plus issues read at today's main |
| PDFluent (`pdf_render`) | 1.3.0 | 2026-11-03 | README, NOTICE, one `Cargo.toml` |
| poppler | 26.11.0 | 2026-11-05 | README only, because the licence rejects it |
| pdf.js | v6.5.438 | 2026-11-01 | `api.js`, `app_options.js`, `writer.js`, advisories |
| PDFKit, CoreGraphics | current Apple docs | read 2026-11-06 | Apple documentation JSON, partly |
| Windows.Data.Pdf | current Microsoft docs | docs updated 2025-11-21 | Class pages |
| lopdf fork | `909771af` (lopdf 0.45.0 plus the indexed reader) | 2026-09-09 | Local, read-only. Upstream is at 0.47.0 (2026-11-04) |

**Evidence marks in the tables:** **[S]** read in pinned source. **[D]** official docs or headers. **[C]** the project's
own claim, not tested. **[I]** third-party report in an issue tracker. **[?]** not verified.

---

## 3. Comparison

"PDFium" is the non-V8 build unless the row says V8.

### 3.1 Fidelity

| Feature | PDFium | MuPDF | hayro 0.9.0 | poppler | pdf.js | PDFKit / CGPDF | Windows.Data.Pdf |
|---|---|---|---|---|---|---|---|
| PDF 2.0 (ISO 32000-2) | R6 encryption path [S]. No conformance statement found. BPC and page-level output intents [?] | [?] | Parses version 2.0 [S]. Keys for `OutputIntents` exist, but no use seen in the interpreter [S] | [?] | [?] | [?] | [?] |
| Transparency | Groups, isolation, soft masks [S]. `/K` knockout groups [?] | [?] | Yes, but no knockout or CMYK blending [C] | [?] | Knockout test files exist in its suite [S]; support itself [?] | [?] | [?] |
| Overprint | One image-only special case found (`cpdf_imagerenderer.cpp:155`) [S] | Overprint simulation, `mutool draw -M` [D] | **None in interpreter or renderer** (only an `OPM` key) [S] | [?] | [?] | [?] | [?] |
| Patterns, shadings | `cpdf_rendertiling.cpp`, `cpdf_rendershading.cpp` [S] | [?] | Yes, recent fixes [S]. Mask cost scales with page area [I #1753] | [?] | [?] | [?] | [?] |
| JBIG2 | Yes, has its own fuzzer [S] | `jbig2dec` [S] | Own decoder. **Open allocation bombs** [I #1550, #1695, #1701] | [?] | Yes, `external/jbig2` [S] | [?] | [?] |
| JPX | Yes, has its own fuzzer [S] | OpenJPEG [S] | Own decoder. **Open allocation bomb** [I #1549]; no HTJ2K answer [I #924] | [?] | Yes, `external/openjpeg` [S] | [?] | [?] |
| CJK, CMap | Has a CMap fuzzer [S]. Fallback needs fonts inside the sandbox | [?] | 61 predefined CMaps, ~250 KB [S]. Non-embedded CJK can vanish [I #1693] | [?] | `external/bcmaps` [S] | [?] | [?] |
| Colour management | lcms for ICCBased, roll to 2.19.1 [S] | lcms2 [S] | moxcms; DeviceCMYK via a built-in micro profile; RGBA output only [S, I #1725] | [?] | `external/qcms` [S] | System [?] | [?] |
| Project's own claim | Chrome's viewer | n/a | "1,600+" regression PDFs, "almost any PDF" renders [C] | n/a | n/a | n/a | n/a |

Poppler's cells are `[?]` because its licence rejects it for a non-GPL product and it was not read further.

### 3.2 Interactive behaviour

| Feature | PDFium | MuPDF | hayro 0.9.0 | poppler | pdf.js | PDFKit | Windows.Data.Pdf |
|---|---|---|---|---|---|---|---|
| Annotation appearance | `FPDF_ANNOT` draws all except widget and popup [D]. Generates missing AP for 10 subtypes (`cpdf_generateap.cpp:1601-1619`) [S] | `pdf_update_annot` synthesises AP [S] | Draws an existing `/AP` only; hidden and no-view flags honoured [S] | [?] | Yes; many `*-without-appearance` test files [S] | `PDFAnnotation.draw` [D] | **None in the API** [D] |
| AcroForm fill | Full form engine: `FORM_On*`, `FPDF_FFLDraw`, timer and invalidate callbacks [D] | `pdf_set_field_value`, field events [S] | **None** [S]. `/Annots` iteration stops at the first bad entry [I #1575] | `NeedAppearances` handled [S]; rest [?] | Yes, plus XFA dataset update on save (`writer.js:275`) [S] | Widget value properties [D] | **None** [D] |
| XFA | **V8 build only** [D]. `pdf_enable_xfa` is tied to `pdf_enable_v8` in the binaries [S] | No mention in `CHANGES` or headers [?] | None | Not examined | Yes. Viewer default on (`app_options.js:830`), API default off (`api.js:276`) [S] | [?] | None listed |
| JavaScript | **V8 build only**; otherwise a stub runtime [S]. Can list actions without running them (`fpdf_javascript.h`) [D] | MuJS, `pdf_enable_js`, field events [S] | None | Not examined | QuickJS sandbox. Viewer default on [S]. **Two "arbitrary JS" advisories** (2024, 2026) [D] | Not mentioned in pages read [?] | None listed |
| Signatures, read | Count, byte range, contents, subfilter (`fpdf_signature.h`) [D] | [?] | Not exposed | [?] | [?] | [?] | None |
| Incremental save | `FPDF_INCREMENTAL` [D] | Yes [?] | No: read-only by design [C] | Not examined | Yes [S] | Unsafe writer (sibling report, finding 6) | None |
| Layers | Renders usage state. **No public toggle in 19 headers** [S] | [?] | Base state, ON, OFF, OCMD; **no `/VE` or annotation `/OC`** [S, I #1690] | [?] | [?] | [?] | [?] |
| Tags | `fpdf_structtree.h`, `FPDFCatalog_IsTagged` [D] | [?] | Marked content passed to devices [S]; no tag tree | [?] | [?] | [?] | None |

### 3.3 Runtime and project

| Property | PDFium | MuPDF | hayro 0.9.0 | poppler | pdf.js | PDFKit / CGPDF | Windows.Data.Pdf |
|---|---|---|---|---|---|---|---|
| Custom file access | `FPDF_LoadCustomDocument`, `m_GetBlock` [D]. `m_FileLen` is `unsigned long`, so 4 GB on Windows LLP64 [D, our reading] | `fz_new_stream` with seek callback [S] | **No.** One `AsRef<[u8]>`, so mmap or a `Vec` [S]. Lazy-read PR draft in review [I #1345] | [?] | Range loading, chunks, `disableAutoFetch` [S] | `CGDataProviderCreateDirect` [D]; byte-position callback named, not spelled out | `LoadFromStreamAsync(IRandomAccessStream)` [D] |
| Open without reading the file | Yes: xref chain, then pages on demand (`TraversePDFPages`), unless repair is needed (whole-file scan) [S] | Repair path exists [S]; lazy behaviour [?] | Xref plus **every page dictionary at open** [S]. Reporter: ~95% of open time is this [I #1659, their number]. Fix PR open [#1741] | [?] | Yes, in a JS engine [S] | [?] | [?] |
| Memory after pages are visited | **Grows:** stream bytes copied and objects cached until the document closes [S, not measured]. Image cache has a limit (`CacheOptimization`) [S] | Store with a size limit (`FZ_STORE_DEFAULT`) [S]; sibling report: 256 MiB per document | Zero-parse objects [S]. Decoded object streams kept for the document's life [S]. Caller drops `RenderCache`; "TODO: allow clearing" [S] | [?] | Sibling report: whole file and all text in worker RAM | [?] | [?] |
| Thread safety | **None**: one call at a time, whole library [D]. `pdfium-render` wraps a global mutex, "no performance benefit" [D] | Per-thread cloned context; one thread per document; display lists render anywhere [D]. Shared lock set [I mupdf-rs #260] | `Pdf` is `Send + Sync`; races fixed in [#1343](https://github.com/LaurenzV/hayro/issues/1343) (closed 2026-10-03). `RenderCache` uses `Rc`, so one per thread [S]. Read-order bug open [I #1746] | [?] | Worker | [?] | Types marked agile [D]; [?] |
| Cancel a slow page | `IFSDK_PAUSE.NeedToPauseNow`, checked per 100 objects (`kStepLimit`) [S]. **Long single image decodes are not guaranteed to pause** [our reading]. `FPDF_RenderPage_Close` to stop [D] | `fz_cookie.abort`, "no upper bound" on when it stops [S] | **None.** Issue [#1052](https://github.com/LaurenzV/hayro/issues/1052) open; PRs #1264, #1266 open; maintainer plans his own design [I] | [?] | [?] | [?] | IAsyncAction [?] |
| Tile rendering | `FPDF_RenderPageBitmapWithMatrix` with clip [D]; progressive start takes offset and size [D]. Draws into a caller buffer (`FPDFBitmap_CreateEx`) [D] | Display list plus pixmap [?] | `render_into` with any transform and context size [S]; u16 dimensions [S]; region decode for JPX still open [I #1559] | [?] | [?] | CGContext [?] | `SourceRect` [D] |
| Download size (compressed, from GitHub API, not measured) | mac-arm64 **3.63 MB**; V8 **13.1 MB**. Win-x64 4.03 MB; V8 13.2 MB | `mupdf` crate source 8.97 MB; built size [?] | Crates are small, built size [?]. Built-in fonts ~240 KB, CMaps ~250 KB [C] | [?] | Needs a JS runtime | In the OS | In the OS |
| Licence | BSD-style [S] plus third-party notices | **AGPL-3.0 or commercial** (§4.2) | Apache-2.0 OR MIT | **GPL** ("programs which call Poppler must be licensed under the GPL") [D] | Apache-2.0 | OS | OS |
| Maintenance | Google. Binaries by one unaffiliated person, weekly since 2017. `pdfium-render` mostly one person | Artifex; commits this week, 1.28.6 on 2026-10-29 | **Mostly one person** (1,923 of the listed commits, "in my free time"); breaking API each 0.x | Monthly tags | Mozilla, monthly | Apple | Microsoft; class page dated 2017 |
| NVD keyword count: 2024 / 2025 / 2026 to date (our query; keyword match, rough) | 3 / 2 / **28** | 1 / 2 / 8 | 0 total | 2 / 9 / 11 | Mostly third-party wrappers, plus 2 project advisories | Not queried | Not queried |
| Fuzzing | 51 files in `testing/fuzzers`, including XFA and JS-related targets [S]. Not under OSS-Fuzz's `projects/` folder | OSS-Fuzz project [S] | **Three codec targets only** in-tree (`ccitt`, `jbig2`, `jpeg2000`) [S]. Others report issues | OSS-Fuzz project [S] | OSS-Fuzz has a `pdf-js` project [S] | [?] | [?] |
| Sandbox | None built in. `FPDF_SetSandBoxPolicy` is only a machine-time policy [D]. Ours to provide | Ours | Ours. `forbid(unsafe)` in `hayro` and `hayro-interpret`, but `hayro-interpret` enables `hayro-syntax`'s `unsafe` feature, and SIMD and flate use unsafe underneath [S] | Ours | Browser's | OS | OS |

**PDFium security note:** three CVEs published on 2026-11-03 read "execute arbitrary code inside the sandbox" in Chrome
before 157.0.8103.38 (NVD). Chrome relies on its sandbox here. A viewer without one has no second line of defence.

**Other Rust options checked as of today:**

- **PDFluent `pdf_render`** is not an independent renderer. Its NOTICE calls `pdf-render` a "substantially extended fork
  of hayro", pinned to `vello_cpu` 0.0.6 where hayro uses 0.3. The forms, XFA and engine crates are AGPL or commercial.
  Created 2026-07-15, one committer. Note also that the crates.io name `pdf_render` now points here, not at the older
  pdf-rs project.
- **`pdf_oxide`, `oxidize-pdf`, `printpdf`, `pdf` (pdf-rs):** described as extraction or manipulation toolkits. They were
  not evaluated as renderers.
- **`pdfium` crate:** GPL-3.0, a different wrapper. **`poppler-rs`:** MIT bindings over GPL poppler.

---

## 4. Licensing that decides things

### 4.1 Permissive, no effect on Recto's licence

PDFium, hayro, pdf.js, lopdf. Recto still has to ship their notices. `steps/08-licenses.sh` in pdfium-binaries collects
them, but the shipped contents were not checked.

### 4.2 MuPDF: the AGPL impact, stated precisely

This is my reading, not legal advice.

- **Linking MuPDF** (static or dynamic; the `mupdf` crate links it) makes Recto and MuPDF one combined work. The FSF FAQ
  says the GPL family covers "the whole combination" for both static and dynamic linking (`#GPLStaticVsDynamic`).
- **AGPLv3 §5(c):** "You must license the entire work, as a whole, under this License to anyone who comes into possession
  of a copy." **§6:** binaries must come with the machine-readable Corresponding Source. So every user of a distributed
  Recto would be entitled to all of Recto's source under AGPL, including the core and any later paid features.
- **§13 (network use)** starts "if you modify the Program". A desktop app with no server does not trigger it. A hosted
  Recto service would, if MuPDF were modified. Artifex's page goes further and asks for full application source for any
  server-based deployment.
- **A helper process is not a clean way out.** The FAQ says pipes and sockets normally mean separate programs, but that
  "intimate" exchange of complex internal data can make one program, and calls the line "a legal question". A renderer
  worker exchanging page data is close to that line. Artifex's own table says "Full source code disclosure (including
  your app code)".
- **Commercial licence:** Artifex describes per-copy fees with a quarterly minimum or a subscription, and no disclosure
  obligation. **No public price: contact sales.**
- **The cost is zero only if Recto is itself AGPL.** Then MuPDF becomes a serious option; SumatraPDF and sioyek use it
  under GPL (sibling report). That is [DECIDE] RD2.

### 4.3 PDFluent's "AGPL or commercial" and poppler's GPL

Same reasoning. I did not look for a commercial route for poppler.

---

## 5. Interactive scope

"Interactive PDF" is defined by these three lists. They answer the owner's "all modern PDF formats, including
interactive PDF".

### v1

| Feature | Exact scope | Why |
|---|---|---|
| Links, outlines | Read via lopdf. GoTo, named destinations and URI. URI asks first. Page labels with sanity checks | Cheap, and 115 of 141 benchmark files have links, 98 have outlines (our scan, heuristic) |
| AcroForm widgets | Show all. Fill text, comb, multiline, checkbox, radio, list and combo. Save as an incremental update. Generate appearance streams | The core of "interactive", and no renderer's writer is needed |
| Annotations, read | Show everything with an appearance. Missing ones drawn by PDFium where it can | Cheap with PDFium |
| Annotations, create | Highlight, underline, strikeout, sticky note, ink. Free text if the form text layout is ready. Stored per sibling report D1 | Most asked-for, and all simple vector appearances (see RD6) |
| JS: detect only | Count and list script actions without running them, and say so plainly: "This form has scripts that were not run" | Safe, and honest about values that may not recalculate |
| XFA hybrid (AcroForm plus XFA) | Show and fill the AcroForm layer. Handle the XFA packet at save (see RD5) | 11 of 141 benchmark files are this |
| XFA dynamic: detect | `/NeedsRendering true` gets a clear message instead of the "please wait" placeholder page | Honest and cheap |
| Optional content | Default state on open. Layer panel toggling via an in-memory appended revision | Works with any backend. 5 of 141 files have layers |
| Tagged PDF | Reading-order text to VoiceOver and Narrator from the struct tree when present | 43 of 141 files are tagged. Accessibility should not wait, but this is only the first step |
| PDF/A | Display. Detect the claim and show it. Warn that saving may break conformance | Display costs nothing. 24 of 141 hit the scan |
| Encryption, open | Standard handler R2 to R6 with user password; AES-256 and legacy RC4. Honour permission flags | PDFium reads it [S], and there are fixtures for it |
| Signatures: report | Inventory, covered bytes, later revisions, DocMDP, "changed after signing" (sibling Rule 15). **Never "valid"** | Avoids the shadow-attack class (§6.4) |

### Later

| Feature | Trigger or plan | Why not v1 |
|---|---|---|
| Field format and calculate, no JS engine | Parse the common `AFNumber_Format`, `AFDate_FormatEx`, `AFSimple_Calculate` and similar calls, and run them in Rust | Likely to cover many real forms without an engine. **Share not measured**: the spike counts it |
| JS with an engine | Only behind an explicit per-document choice, in its own worker, with no file or network reach | Largest attack surface in the list; see RD4 |
| Saving into encrypted files | After lopdf's encrypted write path is tested | Upstream changelog shows encrypted incremental save rejected, then added (0.44). Issues #603 and #608 are open upstream |
| Signature verification | Own design: signed-revision view, update classification, trust store | Needs §6.4 fully done |
| Creating signatures | Append-only through the lopdf writer | Sibling report D6 recommends signing in v1. If "signing" there means cryptographic signing, see RD7 |
| Portfolios, attachments | List and save out, never launch | Rare; no portfolio in the benchmark set |
| Full tag semantics | Role maps, alt text, table structure | After reading order works |
| PDF/A validation | Only if asked for | A separate product area |
| Public-key encryption | On request | Rare |

### Never

| Feature | Reason |
|---|---|
| Running document-level JS automatically on open | Runs attacker's code with no user intent. pdf.js shipped two arbitrary-JS advisories in this area, and the shadow paper shows privileged JS abuse in Acrobat |
| XFA dynamic in the default build | Deprecated direction (§5.1), adds about 9.5 MB of compressed download and V8's attack surface, and only PDFium offers it among permissive options. Offered as an optional add-on only if RD5 says so |
| Multimedia and 3D (RichMedia, Screen, Movie, Sound, 3D) | Show the poster or AP only. hayro's README says 3D is "basically only supported in Acrobat" |
| `Launch` actions, automatic form submission, SOAP | Exfiltration and code execution paths, with little value for a reader |
| Claiming PDF/A or PDF/UA conformance of files we save | We would be making a promise we cannot check |
| Stripping permissions or encryption beyond what the password allows | Legal and trust risk; also out of scope for a reader |

### 5.1 XFA status

The Library of Congress format description says deprecated features "should not be written into a PDF 2.0 document, and
should be ignored by a reader", and lists XFA among dropped or deprecated features, saying "The PDF 2.0 appears to limit
the use of XFA". **The ISO 32000-2 text itself was not read** because it is not free at the source I used, so the
exact wording stays in the Not verified table.

### 5.2 What the owner's own files mean for this

Every IRS file in the benchmark set has an XFA packet, `/UR3` usage rights and three or four `%%EOF` markers, and none
has `/NeedsRendering true` (our scan). So these are real AcroForm hybrids. Two consequences are not yet settled:

- **The XFA packet.** If AcroForm values change and the XFA data does not, an XFA-aware viewer may show the old values.
  pdf.js updates the datasets on save (`writer.js:275`).
- **Usage rights.** `/UR3` is a signature over the document's rights. What Acrobat shows after our save is **unknown and
  needs a test in Acrobat**. That is a task the owner, not an agent, may need to run.

---

## 6. How lopdf and the renderer coexist

### 6.1 Can the renderer parse lazily by itself?

**Yes, and this is the right split.** lopdf cannot feed either renderer: PDFium takes only bytes, and hayro's interpreter
takes `hayro-syntax` types. A hayro draft to resolve objects from a caller-supplied source
([PR #1662](https://github.com/LaurenzV/hayro/pull/1662)) was closed by its author on 2026-11-03 to shorten the queue,
not rejected. It is the one route to a single-parser product, and is listed under §8.

Both can read lazily, with these limits:

| | PDFium | hayro |
|---|---|---|
| How it gets bytes | `pread` on a passed file descriptor via `m_GetBlock` | mmap, or a `Vec` |
| Worst case at open | Whole-file scan on repair | Same, plus all page dictionaries always |
| What grows | Raw stream bytes and parsed objects, until close | Decoded object streams, render cache |
| How we bound it | **Retire the worker** after N pages or M MB, then reopen | Drop `RenderCache`; retire the worker |
| File changes underneath | Read error, which we catch | **SIGBUS.** Worker dies, core survives |

### 6.2 Is there a risk of the two parsers disagreeing?

**Yes. It is real, and already visible in today's sources:**

- The fork's own contract pins differences between its **eager and indexed** routes on malformed files
  (`docs/indexed-reader-contract.md`). So "lopdf" is two parsers. **Recto must use only the indexed route for reads.**
- PDFium rebuilds the xref by scanning when loading fails (`cpdf_parser.cpp:255,263,280`). The fork uses "conservative
  bounded framing and typed recovery/refusal". The same damaged file can therefore produce different object graphs.
- hayro [#1746](https://github.com/LaurenzV/hayro/issues/1746): a page renders differently depending on what was read
  before it. hayro [#1575](https://github.com/LaurenzV/hayro/issues/1575): one bad entry ends `/Annots` iteration, so
  annotation counts disagree with a stricter parser.
- Known soft spots: hybrid xref priority, freed and reused objects, stream length mismatches, junk before the header.
  The owner's fixtures already cover several (`hybrid_xref_revision`, `freed_object_revision`, `freed_compressed_link`,
  `stream-short-length`), and `pdf-association/pdf-differences` exists for this purpose.

### 6.3 The recommended design

```
                   Host app (Swift / C#): draws, native text field overlay
                         │  view models                 ▲ shared-memory bitmaps
                         ▼                              │
 ┌──────────────── recto-core (Rust, no C++) ──────┐    │
 │ lopdf IndexedReader   structure, links, outline │    │
 │ Form + annotation model, appearance generator   │    │
 │ Append-only writer (the ONLY writer)            │    │
 │ Agreement gate, signature inventory, agent API  │    │
 └──────┬──────────────────────────────────▲───────┘    │
        │ fd of file + pending append      │ answers    │
        ▼ page, matrix, clip, deadline     │            │
 ┌──────────────── recto-render worker(s) ─┴────────────┴┐
 │ RenderBackend: PDFium non-V8 (default) | hayro (test) │
 │ Read-only. One thread (PDFium). Sandboxed: no network,│
 │ no file reach except the passed fd and fonts.         │
 │ Killed on overrun. Retired after N pages or M MB.     │
 └───────────────────────────────────────────────────────┘
```

**Rules:**

1. **One writer.** Only the core writes, and only by appending. A worker never writes. PDFium's save functions are not
   called.
2. **What you see is what is saved.** A pending edit is real appended bytes over the original (a virtual file: base file
   plus an append buffer). The worker reopens that. After a field commit, the new appearance is drawn by the renderer
   from those bytes, not by a separate preview path. Layer toggles use the same mechanism and are simply never saved.
3. **Same bytes for both parsers.** Open once, then pass the descriptor. The fork's `FileSource` already detects a change
   (`SourceError::SourceChanged`). A detected change closes the session with a message.
4. **No form engine in the worker.** Typing happens in a native host field over the widget. This matches "Rust decides,
   the host draws". It also respects §0.8 of the plan: using `FORM_OnLButtonDown` for agents would be synthetic mouse
   input in all but name. Agents fill fields through the same model methods as the UI. `FPDF_FFLDraw` is still used to
   draw widgets, because `FPDF_ANNOT` skips them.
5. **Selection and find geometry come from the renderer,** because highlights must line up with its pixels. Text for
   agents may use the lopdf path, and it is covered by the gate below.

**The agreement gate** runs cheap checks at open and more per page:

| Check | lopdf side | PDFium side |
|---|---|---|
| Page count | `page_count()` | `FPDF_GetPageCount` |
| Repaired? | `index_stats().recovered()` | `FPDF_DocumentHasValidCrossReferenceTable` |
| Revisions | **New:** expose the chain (a small fork addition) | `FPDF_GetTrailerEnds` |
| Page boxes, rotation (visible pages) | `page_map()` | `FPDF_GetPageSizeByIndex`, box getters |
| Annotation count, widget count and rectangles | `/Annots` | `FPDFPage_GetAnnotCount`, `FPDFAnnot_GetRect` |
| Signatures: count and byte ranges | `/ByteRange` walk | `FPDFSignatureObj_GetByteRange` |
| After every save | Reopen; field values match what was written | `FPDFAnnot_GetFormFieldValue` |

**On a mismatch:** the document stays viewable with what the renderer draws. Saving, signing and "changed after signing"
reporting are disabled, and a plain notice appears. `recto` reports the mismatch to agents, including text-extraction
results. PDFium offers no lookup of an object by number, so this gate cannot prove full graph equality. **This is a
reduction of the risk, not a proof.** A hayro or lopdf comparison in development can compare objects by id.

### 6.4 Shadow attacks: what they are and what we do

**What the paper says** (Mainka, Mladenov, Rohlmann, NDSS 2021):

- Attackers prepare a document with hidden content, get it signed, and then reveal the content with **well-formed,
  standard-compliant incremental updates**. There are three variants: hide, replace, hide-and-replace.
- 16 of 29 viewers were vulnerable (their count, 2020 versions). "The Hide and Hide-and-Replace variants also bypass
  even perfectly implemented denylists or allowlists."
- **Two parsers are not the core of this attack.** It is the viewer saying "valid" while showing a revision the signer
  never saw. The earlier 2019 work (incremental saving attacks) did use malformed updates and parser leniency. Two
  parsers add that second surface.
- The fix vendors reported: "we compare the parsed document with and without this Incremental Update" (Adobe, PDF
  Architect, per the paper). PDF-Detector compares after removing updates following the first signature.
- Viewers without signature validation were **excluded** from the study (Preview, Sumatra, Evince, Skim). A viewer that
  makes no claim falls outside this class.

**What Recto does:**

| Level | Behaviour |
|---|---|
| **v1** | Reports signature presence, byte ranges, revisions after the signature, and DocMDP. **Never "valid".** Offers "View as signed": truncates the file at the signature's byte-range end and renders those bytes |
| **v1 saving** | Append only (sibling Rule 15). Gate must pass. DocMDP "no changes" refuses |
| **Later, verifying** | "Valid" requires all of these: crypto check; gate passes; each later update is classified against DocMDP rules; **pixel comparison of signed versus final revision on every page**, any non-annotation change is a warning. Reported in the same panel, never as a green tick alone |
| **Before signing (later)** | Render the exact bytes being signed and warn on PDF-Detector's prevention signals |

### 6.5 What this asks of the lopdf fork

| Need | State today | Size (my estimate) |
|---|---|---|
| Append-only writer over `IndexedReader` | `IncrementalDocument` is eager and holds everything; upstream [#575](https://github.com/J-F-Liu/lopdf/issues/575) is an open `/Prev` corruption report | Small to medium |
| Revision list | Not exposed; `recovered()` is | Small |
| AcroForm model and appearance generation | None, and this belongs in Recto, not the fork | Medium to large; the largest single item here |
| Merge upstream 0.46 and 0.47 | Fork is at 0.45. RUSTSEC-2026-0187 is fixed from 0.42, so the fork has that fix | Small |
| A real fuzz target for the indexed route | Upstream [#615](https://github.com/J-F-Liu/lopdf/issues/615) reports fuzz findings. Not checked against the fork | Small |

---

## 7. Rejected options

| Option | Reason |
|---|---|
| **MuPDF** | AGPL forces Recto's whole source open on every copy, or an unpriced commercial deal. Technically strong: overprint, JS, forms, cancel. Returns if RD2 is answered "AGPL is fine" |
| **Poppler** | GPL: its README says "programs which call Poppler must be licensed under the GPL". Features were not examined further |
| **pdf.js** | Needs a JS engine or webview. Plan §3.2 rejects webviews on memory. Sibling report: whole file in worker RAM. Kept as source of test files and as reference for XFA dataset saving |
| **Platform renderers as the main engine** (PDFKit/CGPDF, Windows.Data.Pdf) | Two engines with different bugs breaks "Windows reproduces it exactly". Windows has no forms or annotations API. Sibling report: PDFKit is an unsafe writer. Kept as the free visual baseline in the spike |
| **hayro as v1 default** | No cancellation, no overprint, resource bombs open, one person, no stable API, no full-document fuzz target in-tree. Not rejected for good; see §8 |
| **PDFluent** | Fork of hayro on an older vello, one committer in four months. Forms and XFA are AGPL or commercial |
| **Writing our own renderer on lopdf** | Years of work: fonts, colour, transparency and codecs. No |
| **Forking hayro onto lopdf for one parser** | hayro-syntax lifetimes run through everything, so we would carry the fork forever. Revisit only if upstream accepts an object source |
| **PDFium in-process** | No sandbox for a parser that had 28 NVD hits this year, one global lock, no kill. A crash would take the app |
| **PDFium V8+XFA as the default** | About 9.5 MB more compressed. Runs attacker scripts, needs `allow-jit` under Hardened Runtime or a jitless mode. XFA is a deprecated direction |
| **PDFium form engine for fill and save** | See RD3: two writers, state hidden in a killable worker, agent control through synthetic events |
| **Using `pdfium-render` in the product** | Fine for the spike. High-level progressive rendering and form-event wrappers were not seen, so we would use raw bindings anyway. Its global mutex is pointless in a one-thread worker. 0.9.4 fixed two double-frees. A thin FFI over about 40 functions fits R7's "write the small, critical pieces yourself" |

---

## 8. What would make us switch

| Change | Switch to | Evidence needed |
|---|---|---|
| Spike: hayro's suspect pages are no worse than PDFium's (or within 1 percentage point), **and** cancellation ships, **and** the bomb issues close, **and** footprint is clearly lower | hayro as default, with PDFium dropped | Spike rerun on a later hayro |
| hayro accepts a caller-supplied object source | Consider one parser: hayro-interpret over lopdf | Upstream merge, plus a spike |
| PDFium spike shows footprint growth cannot be held by retiring workers | hayro, or a patched PDFium | Measured growth on `gov_nasa_ntrs20240002628` |
| pdfium-binaries stops, or provenance fails | Build PDFium ourselves | Attestation failure, or a gap in weekly releases |
| Google removes or changes AGG, or the non-V8 build path | Reassess | Release notes |
| Owner chooses AGPL for Recto | MuPDF becomes a real option for fidelity and forms | RD2 answer plus spike C |
| Real users need dynamic XFA in numbers | Optional V8 worker download | Support data |

---

## 9. Spike specification (not run; needs approval)

**Question:** Which renderer, in a worker, meets Recto's memory and speed goals with acceptable fidelity, and do lopdf and
the renderer agree?

**Time-box:** one agent-day, plus one human or agent review pass over suspect pages. **Machine:** the owner's Mac (16 GB,
10 cores). One heavy build at a time (§7.1 of the plan). **Location:**
`/Volumes/EksternalHome/Koding/Rust/PDF-reader/spikes/render-compare/`, removed after the report (R11). No binaries or
result images in git. Reads fixtures and the lopdf fork; writes neither.

### 9.1 What to build

One small Rust workspace. **Each measured run is one process for one file**, so memory numbers stay clean (§3.6).

| Binary | Backend | Notes |
|---|---|---|
| `spike-lopdf` | lopdf fork at `909771af` by git rev | `IndexedReader::open`, `page_map`, `index_stats`. Dumps JSON of pages, boxes, annots, recovered flag |
| `spike-pdfium` | `pdfium-render =0.9.5`, `default-features = false`, features `pdfium_8125` only; binds the 8125 dylib | Uses `load_pdf_from_reader` with `pread`. Raw bindings for `FPDF_RenderPageBitmap_Start` and `FPDF_FFLDraw` |
| `spike-hayro` | `hayro =0.9.0`, `memmap2 =0.9.12` | `Pdf::new(Arc<Mmap>)`, `render` at scale 2 |
| `spike-cgpdf` | Small Swift CLI using CoreGraphics. Nothing downloaded | Visual baseline only |
| `spike-mupdf` (optional, C) | `mupdf =0.8.0` | **Visual reference only, never shipped.** AGPL has no effect on an undistributed spike. Builds C sources, so run it last |
| `spike-pdfium-v8` (optional, D) | 8125 V8 tarball | Size and one JS and XFA file only. Not a candidate |
| `harness` | Python stdlib only | Runs binaries interleaved A, B, A, B; three repeats; 120 s timeout; kills at 4 GiB footprint; keeps raw samples |

### 9.2 What to measure

| Metric | Definition | Tool |
|---|---|---|
| Open time | Process start excluded; open call to page count available; median of 3. Warm cache stated | `Instant` |
| First page | Open plus first page at 2x | `Instant` |
| ms per page at 2x | Scale 2.0 (144 dpi), white background. p50, p95, max. First 200 pages | `Instant` |
| Peak `phys_footprint` | `proc_pid_rusage(RUSAGE_INFO_V4).ri_lifetime_max_phys_footprint`, cross-checked by `footprint` on one run per backend | syscall |
| Footprint curve | After open; after 1 page; after each 25 pages; after dropping caches and document | syscall |
| Binary size | Stripped release binary plus dylib. `du` and `size -m` | shell |
| Visual failures | §9.4 | own script |
| Cancel latency | Stop at 50 ms into the slowest pages: time until the call returns, and whether kill was needed | `Instant` |
| Tile cost | 512 x 512 tile at 4x versus full page at 4x; time and peak | `Instant` |
| Worker cycle | Kill, respawn, reopen, first tile; baseline footprint of an idle worker | syscall |
| Parallel scale | hayro threads 1, 2, 4; PDFium processes 1, 2, 4 | `Instant` |
| Survival | Crash, hang or over-cap per file | harness |
| Agreement | Every §6.3 gate row, lopdf against each backend, per file | JSON diff |

### 9.3 Test files

Feature columns come from **our byte scan on 2026-11-06**: regex on raw bytes plus inflated Flate streams, using Python's
standard library. It may contain false positives. A recheck of 2 of the "JS" hits found no `/JavaScript` key. The 346 MB
file was skipped by the scan.

**From `/Volumes/EksternalHome/Koding/Rust/distillPDF/tests/fixtures_pdf` (121 PDFs): all, one pass, for crash and
agreement.** Named for review:

| Purpose | Files |
|---|---|
| Transparency | `alpha_groups`, `smask_group_raster`, `smask_panel` |
| Colour | `separation`, `colorspace_images`, `cmyk_jpeg` |
| Codecs | `decode_jpeg`, `codec_basemap`, `undecodable_codec`, `inline_image`, `oversized_raster` |
| Annotations, widgets | `annot_render`, `annot_appearance`, `links` |
| Geometry | `rotated_pages`, `rotated_image`, `indirect_mediabox`, `inherited_mediabox`, `pagelabels` |
| **Parser differentials** | `hybrid_xref_revision`, `freed_object_revision`, `freed_compressed_link`, `objstm_pages`, `objstm_filter_forms`, `damaged_streams` |
| Encrypted | `encrypted/` (8 files), `objstm/objstm-r4-rc4.pdf` |
| **Adversarial** | `adversarial/` (all 8): `form_bomb`, `form_repeat`, `deep_page_tree`, `deep_broad_page_tree`, `damaged_startxref`, `damaged_startxref_intact`, `stream-missing-length`, `stream-short-length` |

**From `benchmarking/bench100` (141 PDFs, 841 MB): all, first 200 pages each,** plus these named tiers:

| Tier | Files | Why |
|---|---|---|
| Very large | `gov_usgs_usgs70277338` (346 MB, 1 page), `gov_nasa_ntrs20240002628` (102 MB, 383 pages; JBIG2, CCITT, layers, FreeText, Popup) | Lazy reading and footprint growth |
| Image-heavy | `cs_CV_2607.28611` (30 MB), `gov_usgs_usgs70278165` (29 MB; JPX, DeviceN, overprint on) | Decode cost |
| Long, text | `standards_ietf_rfc9110` (194 pages), `eu_law_eu_aiact` (144 pages) | Per-page cost, scrolling |
| **Hybrid XFA forms** | all 11 `forms_irs_*` | Form draw, XFA and UR3 behaviour |
| **Signed** | `policy_worldbank_wbD34442276`, `…277`, `…284`, `…285` (`/FT /Sig`, 2 EOFs) | Signature inventory, signed-revision view |
| Overprint | `gov_usgs_usgs70275236`, `gov_nasa_ntrs20180005693`, `un_report_un_sdg2024` | hayro has none |
| CJK | `physics_optics_2607.28558` | Predefined CMap |
| Encrypted | `un_report_un_sdg2023` | Real-world |

**Gaps in the owner's files, filled from outside:** no PDF 2.0 file, no dynamic XFA, no confirmed JS form, no knockout
test, no shadow sample.

### 9.4 Visual failures

There is no ground truth, so use majority agreement:

1. Render at 2x with PDFium, hayro and CGPDF (MuPDF if run).
2. Page pair score: share of pixels with max channel difference above 32, after 1 px dilation to tolerate anti-aliasing.
3. A page is **suspect for a renderer** when its score against the other two exceeds 1% and they agree with each other.
4. An agent or the owner reviews all suspects up to 100 per renderer, and labels each one real or noise.
5. Report real failures per renderer, by feature. These thresholds are starting points: report the raw scores too.

### 9.5 Downloads, exactly

**Nothing below is downloaded until the owner approves.**

**PDFium binaries** from `https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8125/`. Using 8125
because `pdfium-render` 0.9.5's `pdfium_latest` binds 8125. Sizes and digests are from the GitHub API on 2026-11-06:

| File | Bytes | SHA-256 | Use |
|---|---|---|---|
| `pdfium-mac-arm64.tgz` | 3,540,999 | `4939bd935ac83ddc09a3a4f038bc43a19cea1660316a3667f26cb07a070e139d` | Required |
| `pdfium-v8-mac-arm64.tgz` | 13,015,665 | `0792e15ce0a899fd9ee56b3ff83ff77fc29abf06616b48ae6df49a7efbf9bb21` | Optional, D |

**Crates** from crates.io through a committed `Cargo.lock`, using `cargo fetch --locked`. Checksums from the crates.io API:

| Crate | Checksum | Note |
|---|---|---|
| `hayro` 0.9.0 | `34d3977ae664ab6eea015be3024379c83c6a3580f67d51be6884f1992bf683ba` | |
| `pdfium-render` 0.9.5 | `296b1146652b486ea6eea0d316e85ab6837ff69e8d2b1568e6fa7bc38bd4c00e` | |
| `memmap2` 0.9.12 | `6b0ca1155a9ecc25aa12e8313a119f32edf0255fea2f44839c4ac0a12b2ac157` | |
| `mupdf` 0.8.0 | `185fb74927a40c569b7152c01c73f4c0206059b8b3d37c7eb007cc3f63453b64` | Optional, C. Its build script compiles vendored C |

**Outside test files,** from GitHub at pinned commits:

| Source | Pin | Files | Licence |
|---|---|---|---|
| `pdf-association/pdf20examples` | `c20f2c17bfcc4baab7cfe62e70fae64caf14d5fa` | All 7 PDFs, including BPC, page-level output intent, offset start, incremental save | CC BY-SA 4.0 |
| `pdf-association/pdf-differences` | `907fe96e52b73e491489eee545c47b119bf9989b` | All | Apache-2.0 |
| `mozilla/pdf.js` `test/pdfs` | tag v6.5.438 (`0a415334…`) | `knockout_*.pdf` (8), `annotation-*-without-appearance*.pdf`, `annotation-text-widget.pdf`, `annotation-choice-widget.pdf`, `annotation-button-widget.pdf`, `js-*.pdf` (3), `bug1947248_forms.pdf`. `xfa_*.pdf.link` files point elsewhere: fetch two, record hashes | Apache-2.0 for the repo; linked files vary, check each |
| PDFium `testing/resources` | branch 8138 | `annot_javascript.pdf`, `annots.pdf`, `annots_action_handling.pdf`, `annotation_markup_multiline_no_ap.pdf` | BSD-style |
| PDF-Attacker samples | Check https://pdf-insecurity.org first | One each of hide, replace, hide-and-replace, if samples are published. Otherwise generate them | Check |

**Integrity:**

1. Download each file into its own new, empty directory. The harness and scripts live elsewhere, and paths are passed as
   arguments (Python with `-I`).
2. `shasum -a 256 -c` against the table. **Stop on mismatch.**
3. `gh attestation verify <file> --repo bblanchon/pdfium-binaries`. The GitHub API shows 2 attestations for the 8125
   tarball digest, and for 8138 they are a SLSA v1 provenance and a release attestation. Confirms the build came from
   that repo's workflow, not that its source is trustworthy.
4. Unpack with `tar -tzf` listed and checked first (no absolute paths, no `..`). Only `libpdfium.dylib`, headers and
   licences are used.
5. Cargo verifies crate checksums from `Cargo.lock`. Record `cargo tree` and the dependency count. Build scripts run
   during the build: do it as the normal user, with no credentials in the environment, and no network after `fetch`.
6. Run harness binaries with no credentials, against copies of nothing private. All test PDFs are public or synthetic.

### 9.6 Proposed decision rule

These are **proposals; replace them with R10's budgets when they exist.** PDFium non-V8 is confirmed if all hold:

- No crash in bench100 outside the adversarial set. Adversarial files may kill the worker but not the harness.
- Idle worker baseline and footprint curve leave room for the app budget; the curve can be held flat by retiring.
- p95 page at 2x under 150 ms on text and vector pages.
- Cancel returns under 100 ms on 95% of slow pages, with kill covering the rest.
- Real visual failures on no more than 1% of pages, reviewed.

Otherwise §8 applies.

### 9.7 Deliverable

`docs/research/YYYY-MM-DD-render-spike.md` with Confirmed and Not tested tables, the machine and OS, every version, raw
JSON kept outside git with its hash noted, and the suspect-page review. Then delete the spike.

---

## 10. [DECIDE] items

Prefixed RD so they do not collide with D1 to D7 in the sibling report.

**RD1. Which renderer does v1 ship?**
- A) **PDFium non-V8 in a worker.** Battle-tested and fast to reach full fidelity. Costs: C++ in the product, a sandbox to
  build, a steady security update duty, memory growth to manage, no real parallelism without more processes.
- B) **hayro in a worker.** Pure Rust and permissive, threads work, small. Costs: no cancellation yet, no overprint, open
  resource bombs, one maintainer, breaking API.
- C) **Both shipped,** switched per document. Worst of both for size and testing.
- **Recommendation: A, confirmed by the spike,** with hayro as a dev-only second opinion.
- **Cost to change later: low to moderate.** The seam, worker and form design are shared. Text geometry and the agreement
  gate have per-backend parts.

**RD2. Is Recto closed source, or could it be AGPL?**
- A) **Permissive or closed.** MuPDF and poppler stay out.
- B) **AGPL for all of Recto.** MuPDF is free to use; everyone gets all source, and any paid or closed feature is out.
- C) **Buy Artifex's commercial licence.** Closed and strong, at an unknown recurring cost; needs a sales call.
- **Recommendation: A,** which keeps all options on the renderer seam open. Only you can decide this.
- **Cost to change later:** A to B is cheap at any time. B to A is very expensive once outside contributions exist.

**RD3. Who owns form filling and saving?**
- A) **Rust core plus lopdf** (recommended design). One writer, testable without a UI, agents use the same methods,
  survives a renderer swap, native text input. Costs: we write the appearance generator and the append-only writer, and
  edge-case forms will expose gaps.
- B) **PDFium's form engine and writer.** Much less code at first. Costs: form state lives in a killable worker, a second
  writer, every timer and invalidate callback crosses a process boundary, agents drive it through mouse-style calls, and
  it ties forms to PDFium.
- **Recommendation: A.** Effort is the real trade-off: this is the largest item in this report.
- **Cost to change later: high** in either direction. Decide before form work starts.

**RD4. How much JavaScript runs?**
- A) **None in v1; detect and tell.** Forms keep stored values, but calculated fields do not update.
- B) **A) plus common `AF*` functions in Rust,** soon after v1. No engine. Covers an unmeasured share of forms.
- C) **Real engine in an isolated worker, opt-in per document.** Most complete. V8 needs `allow-jit` or jitless mode, more
  size and a large attack surface.
- **Recommendation: A for v1, then B.** Hold C until users ask.
- **Cost to change later: low.** Everything is additive.

**RD5. XFA.** Two questions.
- **Saving hybrids:**
  - A) Update the XFA datasets, as pdf.js does. Most correct; more work.
  - B) Remove `/XFA` from the AcroForm in the appended revision. Simple, but changes the form's nature.
  - C) Leave it stale. Risks wrong values in XFA-aware viewers. Rejected.
  - **Recommendation: A if it fits in the v1 form work, otherwise B with a notice.** Test against Acrobat first.
- **Dynamic XFA:**
  - A) Detect and explain only. Small and safe; those forms do not work here.
  - B) Optional downloadable PDFium-V8 worker. They work, at about 9.5 MB and V8's attack surface.
  - **Recommendation: A.** "Modern" PDF does not need a format the standard is moving away from.
- **Cost to change later: low** for both.

**RD6. How much annotation creation is in v1?**
- A) **Read only.** Smallest.
- B) **Highlight, underline, strikeout, note, ink.** Covers what readers want; all simple appearances.
- C) **B plus free text and stamps.** Needs the form text layout; more surface.
- **Recommendation: B.** It also fits sibling D1 and D6.
- **Cost to change later: low.**

**RD7. Does v1 verify or create signatures?**
- A) **Report only, never "valid".** Outside the shadow attack class. Users needing verification use another tool.
- B) **Verify in v1.** Useful, but needs the full §6.4 process, a trust store and revision classification.
- C) **Create in v1 too.** Sibling D6 lists "signing". If that means applying a certificate signature, it adds key handling
  and a pre-sign check. If it means placing a signature image, it is just an annotation, and much smaller.
- **Recommendation: A,** plus creating only if you confirm which meaning you intend.
- **Cost to change later: low** to add. **High** if a wrong "valid" ever ships.

**RD8. Where do PDFium binaries come from in releases?**
- A) **pdfium-binaries,** pinned, digest and attestation checked. No build infrastructure, weekly. One unaffiliated person
  in the chain; shared library only (no static tarball seen in this release).
- B) **Build ourselves** with the same GN args. Full control and could strip more. Large checkout, a heavy build for a
  16 GB Mac, and we own the weekly duty.
- **Recommendation: A for v1,** with an update path that can ship a new PDFium within days. Recheck B before 1.0.
- **Cost to change later: low.** The build output is the same library.

**RD9. Approve the spike?**
- A) **Run §9 as written,** including the downloads listed in §9.5.
- B) **Run required parts only,** without MuPDF and V8.
- C) **Skip it** and go with RD1 on reading alone. Not advised: memory is the main question, and nothing here measures it.
- **Recommendation: B first.** Add MuPDF only if RD2 opens it.
- **Cost:** about one agent-day, about 5 MB of required downloads plus test files.

---

## 11. Confirmed and Not verified

### Confirmed from a primary source

| Claim | Source | Date read |
|---|---|---|
| PDFium: "None of the PDFium APIs are thread-safe" | `public/fpdfview.h:11-13`, 8138 | 2026-11-06 |
| Custom file access via `m_GetBlock` | `public/fpdfview.h:475-492,595` | 2026-11-06 |
| Stream bytes copied on parse | `core/fpdfapi/parser/cpdf_syntax_parser.cpp:854-866` | 2026-11-06 |
| Parsed objects cached in `indirect_objs_` | `core/fpdfapi/parser/cpdf_indirect_object_holder.cpp` | 2026-11-06 |
| Pause checks every 100 objects | `cpdf_progressiverenderer.h:47`, `.cpp:113,129` | 2026-11-06 |
| `FPDF_ANNOT` skips widgets and popups | `public/fpdfview.h:947-951` | 2026-11-06 |
| Missing-AP generation by subtype | `core/fpdfdoc/cpdf_generateap.cpp:1601-1619` | 2026-11-06 |
| Xref validity is exposed; can change as the parser evolves | `public/fpdfview.h:636-648` | 2026-11-06 |
| Incremental save flag | `public/fpdf_save.h:45-47` | 2026-11-06 |
| V8 and XFA are one switch in the binaries; AGG is default (no Skia arg) | `steps/05-configure.sh`, pdfium-binaries | 2026-11-06 |
| Tarball sizes, digests, 2 attestations | GitHub API | 2026-11-06 |
| hayro takes `Arc<AsRef<[u8]>>` or `Vec` | `hayro-syntax/src/data.rs` | 2026-11-06 |
| hayro resolves all pages at open | `hayro-syntax/src/page.rs:49-56,118-` | 2026-11-06 |
| hayro's decoded object streams retained | `hayro-syntax/src/data.rs` (`SegmentList`) | 2026-11-06 |
| hayro annotations are AP-only | `hayro-interpret/src/interpret/mod.rs:165-240` | 2026-11-06 |
| hayro has no overprint code | grep of `hayro-interpret`, `hayro`; only key `OPM` in `dict.rs:785` | 2026-11-06 |
| hayro has crypto despite stale crate docs that say otherwise | `hayro-syntax/src/crypto/`, `pdf.rs:new_with_password` | 2026-11-06 |
| hayro fuzz targets: 3 codecs | `hayro-fuzz/fuzz_targets/` | 2026-11-06 |
| MuPDF licence text and commercial route | `README`, `COPYING`, master | 2026-11-06 |
| MuPDF cookie abort has "no upper bound" | `include/mupdf/fitz/device.h:456-473` | 2026-11-06 |
| MuPDF forms, JS and overprint simulation | `form.h:178,347-350`, `javascript.h`, `CHANGES` 1.28 | 2026-11-06 |
| Poppler: "must be licensed under the GPL" | `README.md`, master | 2026-11-06 |
| pdf.js defaults; advisories | `app_options.js:454,830`, `api.js:276`, GHSA-hq66-cqwq-w95j, GHSA-wgrm-67xf-hhpq | 2026-11-06 |
| Shadow attack facts | NDSS 2021 paper, pp. 1-5, 11-13 | 2026-11-06 |
| lopdf advisory, fixed in 0.42 | OSV GHSA-6fj4-rcr3-r7rr, RUSTSEC-2026-0187 | 2026-11-06 |
| Fork: indexed/eager differences pinned; `recovered()`; `SourceChanged` | Local fork at `909771af` | 2026-11-06 |
| Windows API surface | Microsoft Learn class pages | 2026-11-06 |
| `allow-jit` meaning | Apple documentation | 2026-11-06 |
| AppContainer isolation | Microsoft Learn, dated 2025-07-08 | 2026-11-06 |

### Not verified

| Item | Why it matters | How to settle |
|---|---|---|
| **All performance and memory numbers for every renderer** | The main question | Spike |
| PDFium memory growth in practice | Source suggests it; a free path may exist that I did not find | Spike footprint curve |
| Uncompressed sizes of any binary | Budget | Spike |
| **hayro 0.9.0 fidelity on our files** | Its "almost any PDF" is its own claim | Spike |
| PDF 2.0 features in any renderer (BPC, page-level intents) | "All modern formats" | `pdf20examples` in spike |
| ISO 32000-2 wording on XFA and `NeedAppearances` | Spec text not read; LoC is a secondary source. I recall `NeedAppearances` is deprecated there, but did not confirm it | Read the spec |
| What Acrobat shows after our save of an IRS form (`/UR3`, XFA) | Could mislead users | Test in Acrobat; may need the owner |
| JS in IRS forms: what it actually does | Sets RD4's value. My scan found `/JavaScript` keys, no plain `AF*` text | Enumerate with `FPDFDoc_GetJavaScriptAction` in spike |
| PDFium cancellation inside a single large image decode | Sets how often we kill | Spike |
| Worker launch and sandbox on Mac (XPC or `sandbox_init`) and Windows (AppContainer) with font access | The design depends on it. The XPC page I read did not cover crash isolation or entitlements | Platform track or a small spike |
| Passing a file descriptor to a sandboxed worker plus system fonts in PDFium | Font fallback must work | Same |
| PDFKit's JS and XFA behaviour; CGPDF's fidelity | Baseline only | Spike for fidelity |
| Windows.Data.Pdf fidelity, threading, cancellation | Baseline only | Windows spike if ever needed |
| MuPDF threading rules at the current version | Read through search excerpts from the 1.26 and 1.27 docs; the threads page URL I tried returned 404 | Read MuPDF docs at 1.28 |
| Which MuPDF version `mupdf` 0.8.0 vendors | Spike accuracy | Check `mupdf-sys` |
| `pdfium-render` 0.9.5 source | I read master at 0.9.6-dev; the crate has no tags | Read the `.crate` before use |
| NVD counts as a measure of security | Keyword match, and it misses Chromium-internal fixes. Not a trend claim | Treat as a rough signal |
| Whether lopdf's open issues #575, #603, #608 and #615 apply to the fork | Writer design | Test the fork |
| Whether `FPDF_DocumentHasValidCrossReferenceTable` and `FPDF_GetTrailerEnds` work as a gate in practice | Gate design | Spike agreement matrix |
| Licence of each third-party component inside PDFium tarballs | Notices to ship | Inspect after download |

---

## 12. Sources

All read 2026-11-06.

**PDFium:** [public headers at 8138](https://pdfium.googlesource.com/pdfium/+/refs/heads/chromium/8138/public/);
[parser](https://pdfium.googlesource.com/pdfium/+/refs/heads/chromium/8138/core/fpdfapi/parser/);
[render](https://pdfium.googlesource.com/pdfium/+/refs/heads/chromium/8138/core/fpdfapi/render/);
[fuzzers](https://pdfium.googlesource.com/pdfium/+/refs/heads/chromium/8138/testing/fuzzers/);
[pdfium-binaries release 8138](https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium/8138) (2026-11-04);
[release 8125](https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium/8125) (2026-10-26);
[pdfium-render](https://github.com/ajrcarey/pdfium-render) README and source (2026-11-01).

**hayro:** [repo at hayro-v0.9.0](https://github.com/LaurenzV/hayro/tree/hayro-v0.9.0) (2026-11-02). Issues and PRs:
[#1052](https://github.com/LaurenzV/hayro/issues/1052), [#1264](https://github.com/LaurenzV/hayro/pull/1264),
[#1266](https://github.com/LaurenzV/hayro/pull/1266), [#1343](https://github.com/LaurenzV/hayro/issues/1343),
[#1345](https://github.com/LaurenzV/hayro/issues/1345), [#1549](https://github.com/LaurenzV/hayro/issues/1549),
[#1550](https://github.com/LaurenzV/hayro/issues/1550), [#1559](https://github.com/LaurenzV/hayro/issues/1559),
[#1575](https://github.com/LaurenzV/hayro/issues/1575), [#1659](https://github.com/LaurenzV/hayro/issues/1659),
[#1662](https://github.com/LaurenzV/hayro/pull/1662), [#1690](https://github.com/LaurenzV/hayro/issues/1690),
[#1693](https://github.com/LaurenzV/hayro/issues/1693), [#1695](https://github.com/LaurenzV/hayro/issues/1695),
[#1701](https://github.com/LaurenzV/hayro/issues/1701), [#1725](https://github.com/LaurenzV/hayro/issues/1725),
[#1737](https://github.com/LaurenzV/hayro/pull/1737), [#1741](https://github.com/LaurenzV/hayro/pull/1741),
[#1746](https://github.com/LaurenzV/hayro/issues/1746), [#1753](https://github.com/LaurenzV/hayro/issues/1753).
Timings in these issues are the reporters', not measured by us.

**MuPDF:** [repo](https://github.com/ArtifexSoftware/mupdf); [Artifex licensing](https://artifex.com/licensing);
[Using MuPDF with C, 1.26.1](https://mupdf.readthedocs.io/en/1.26.1/using-mupdf.html) (search excerpt);
[overview, 1.27.0](https://mupdf.readthedocs.io/en/1.27.0/reference/c/overview.html) (search excerpt);
[mupdf-rs #260](https://github.com/messense/mupdf-rs/issues/260).

**Licences:** [AGPLv3](https://www.gnu.org/licenses/agpl-3.0.en.html);
[GPL FAQ](https://www.gnu.org/licenses/gpl-faq.html).

**Others:** [poppler README](https://gitlab.freedesktop.org/poppler/poppler/-/blob/master/README.md);
[pdf.js v6.5.438](https://github.com/mozilla/pdf.js/tree/v6.5.438);
[pdf.js advisory 2026](https://github.com/mozilla/pdf.js/security/advisories/GHSA-hq66-cqwq-w95j);
[PDFluent](https://github.com/pdfluent/pdfluent-sdk) README and NOTICE.

**Platforms:** [PDFAnnotation](https://developer.apple.com/documentation/pdfkit/pdfannotation);
[CGDataProvider direct](https://developer.apple.com/documentation/coregraphics/cgdataprovider/init(directinfo:size:callbacks:));
[allow-jit](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.cs.allow-jit);
[XPC services](https://developer.apple.com/documentation/xpc/creating-xpc-services);
[PdfDocument](https://learn.microsoft.com/en-us/uwp/api/windows.data.pdf.pdfdocument);
[PdfPage](https://learn.microsoft.com/en-us/uwp/api/windows.data.pdf.pdfpage);
[PdfPageRenderOptions](https://learn.microsoft.com/en-us/uwp/api/windows.data.pdf.pdfpagerenderoptions);
[AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation).

**Attacks and standards:** [Shadow Attacks, NDSS 2021](https://www.ndss-symposium.org/ndss-paper/shadow-attacks-hiding-and-replacing-content-in-signed-pdfs/);
[pdf-insecurity.org](https://pdf-insecurity.org);
[Library of Congress, PDF 2.0](https://loc.gov/preservation/digital/formats/fdd/fdd000474.shtml) (updated 2023-12-07);
[pdf20examples](https://github.com/pdf-association/pdf20examples);
[pdf-differences](https://github.com/pdf-association/pdf-differences).

**Advisories and counts:** [NVD API](https://services.nvd.nist.gov/rest/json/cves/2.0) (keyword search, our query);
[OSV](https://osv.dev) (GHSA-6fj4-rcr3-r7rr, RUSTSEC-2026-0187);
[OSS-Fuzz projects](https://github.com/google/oss-fuzz/tree/master/projects).

**Local:** `/Volumes/EksternalHome/Koding/Rust/lopdf-lazy-engine` at `909771af`;
`/Volumes/EksternalHome/Koding/Rust/distillPDF/tests/fixtures_pdf`;
`/Volumes/EksternalHome/Koding/Rust/distillPDF/benchmarking/bench100`;
sibling report `docs/research/2026-11-06-prior-art-failures.md`.
