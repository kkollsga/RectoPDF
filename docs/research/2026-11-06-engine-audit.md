# Engine audit: lopdf fork, distillpdf, ripgrep search corpus

**Date:** 2026-11-06. **Track:** engine audit for RectoPDF (`recto`). **Status:** research; no app code exists.

**Audited at:**
- `lopdf-lazy-engine`, branch `work/lopdf-045-integration`, HEAD `909771a`. `origin/distillpdf/indexed-reader-v7` is the same commit (0 commits apart both ways). The fork is 115 commits ahead of `origin/main`.
- `distillPDF`, branch `main`, HEAD `b8d6d69`, version 0.1.6, pinned to the fork at `909771af` (`distillpdf/Cargo.toml`).

**Machine:** Mac mini M4 (4P + 6E cores), 16 GB, macOS 27.0.1, rustc 1.99.1, external SSD, ripgrep 15.1.0.

## 1. Summary

1. **Lazy open works and is the main win.** On a 330 MiB file the fork opens in 1.2 ms, reads 0.34 MB and sits at 2.3 MiB `phys_footprint`. The eager loader takes 179 ms and 655 MiB. On a 16 MiB S-1 with 116,576 objects the figures are 11.5 MiB versus 298 MiB.
2. **Lazy open does not make page work small.** Stream decoding and the operator AST dominate peak memory. One page with 382,527 operators peaks at 230 MiB from only 4.7 MiB of decoded content. distillpdf's text path peaks at 301 MiB on that file and 406 MiB on the 383-page NASA report.
3. **The fork is not a viewer engine and cannot save cheaply.**
   - It has no rendering path: it stops at a tokenised operator list.
   - The indexed reader has no writer.
   - The only incremental save is eager. It holds the whole file in a `Vec` plus a full `Document`, about 300 MiB to append 527 bytes to the 17 MB S-1. The output bytes are correct: the prefix is identical.
   - It has no `/ByteRange` or signature support and no typed AcroForm API.
4. **distillpdf's per-page path suits background indexing.** It extracts one page of plain text at a time (1–3 ms/page typical, 97 ms on the worst page), and that gives a natural cancellation point. Everything structured (`render`, `distill`, blocks with bboxes) is whole-document only, has no cancellation and peaks at 200 MiB–1 GiB on big files.
5. **Search with ripgrep is cheap.**
   - A query over one document takes 7–12 ms; over 5,872 page files from 141 documents, 58–60 ms.
   - The corpus is 1.9% of the PDF bytes (3.3% on disk) across the library.
   - A selective query costs 59–342 tokens. A frequent term capped at 40 lines costs about 1,400. Folio's 400,000-character dump costs 100,000 tokens and still cuts off 72% of the S-1.
6. **Hit to highlight is possible today, but only through an API labelled diagnostic.** `dbg_spans_xy` located 94% of 456 sampled words within ±3 pt of poppler's box, at about 1× word area. Block bboxes contained 91% of words but cover 59–132× the word's area.

## 2. lopdf fork

### 2.1 Public API of the indexed reader

| Area | API | Source |
|---|---|---|
| Open | `IndexedReader::open`, `open_with_options`, `open_shared`, `open_cached`, `open_shared_cached` | `src/indexed_reader/open.rs:877-923` |
| Sources | `RandomAccessSource` trait (`len`, `read_at`, `validate_unchanged`, `read_exact_at`, `read_range`); `FileSource`, `BytesSource` | `src/source.rs:94`, `:267`, `:203` |
| Objects | `resolve_object`, `resolve_object_shared`, `resolve_many_shared` | `src/indexed_reader/resolve.rs:129`, `:1173`, `:1198` |
| Budgeted | `resolve_scalar_with_permit`, `resolve_object_with_permit`, `resolve_stream_with_permit`, `prepare_object_stream_with_permit`; `ScalarResolutionPermit::cancel` | `resolve.rs:137-214`, `src/scalar_budget.rs:295` |
| Streams | `resolve_stream_descriptor`, `IndexedStreamDescriptor::open_plain_encoded`, `EncodedStreamReader::read_chunk` | `streams.rs:579`, `:129`, `:192` |
| Pages | `page_map`, `page_map_with_stats`, `page_count`; `PageMapEntry::{id, inherited}` | `page_map.rs:289-297`, `:54-58`, `resolve.rs:1325` |
| Metadata | `trailer_entry_owned`, `trailer_entry_raw_owned`, `object_ids`, `object_location`, `version`, `source_len`, `is_encrypted`, `is_authenticated` | `resolve.rs:1253-1320` |
| Stats | `index_stats`, `cache_stats`, `object_cache_stats`, `object_stream_cache_stats` | `stats.rs:173-269` |

Not public: `xref_start` is `pub(crate)` (`open.rs:25`) and `object_location` deliberately hides offsets (`resolve.rs:1288`). There is no cache-trim call. Dropping the reader is the only way to release its caches.

### 2.2 What is lazy and what is not

| Thing | Behaviour | Source |
|---|---|---|
| Header, tail | Eager: a 1 KiB and a 64 KiB window | `mod.rs:17-21`, `:230-232` |
| Xref chain, all revisions | **Eager at open**. Parsed in full and retained as a `BTreeMap`, O(objects). Measured 70–85 B per object. Limits: 1,000,000 entries, 1,024 revisions, 16 MiB window, 32 MiB decompressed | `mod.rs:22-33`, `:234-237` |
| Encryption dictionary, password check | Eager when `/Encrypt` is present | `open.rs:972-1012` |
| Object bodies | Lazy: 4 KiB window growing in 64 KiB steps, 4 MiB per-object cap | `mod.rs:39-41`, `:240-242` |
| Object streams | Lazy. The whole container is decoded on first member touch, cached only by a cached reader | `mod.rs:42-47`, `:75-81` |
| Page tree | Untouched until `page_map()`, then a **full walk of every node**. `/Count` is not trusted and no descent to page N exists. The reader does not retain the map, so `page_count()` walks again each call | `page_map.rs:206-264`, `resolve.rs:1324-1327` |
| Stream bytes | `resolve_object` materialises all encoded bytes (cap 64 MiB). Descriptors read none | `resolve.rs:1863`, `mod.rs:243`, `streams.rs:579` |
| Stream decode | Never done by the reader. The caller decodes the whole buffer. The chunked reader exposes encoded plaintext only: "Decoding PDF filter chains is intentionally outside" its contract | `object.rs:983-1028`, `streams.rs:126-129` |
| Filters | Flate, Brotli, LZW, ASCII85, ASCIIHex, RunLength. DCT, JPX, CCITT and JBIG2 are not decoded; DCT measured as a "missing feature" error | `object.rs:1062-1067` |

**I/O model.** The reader uses `pread` through `FileExt::read_at`, with an `fstat` after every physical read to detect file changes (`source.rs:348-361`). It does not use mmap: `#![forbid(unsafe_code)]` rules that out inside the crate (`lib.rs:2`), though an app could supply its own mmap `RandomAccessSource`. The eager `Document` and `IncrementalDocument` read the whole file into a `Vec` (`reader.rs:80`, `:386`).

**File change is a hard stop.** After any external or in-app append, every read fails with a sticky `SourceChanged` (`source.rs:318-328`; measured below). A save means reopening the reader.

**Threads.** `IndexedReader` is `Send + Sync`, asserted in `src/indexed_reader/tests/resolve.rs:233`. Same-object calls are single-flighted (`mod.rs:42-44`). A reader creates no threads or timers of its own: `thread::spawn` in `source_cache.rs` appears only after `#[cfg(test)]` at line 721. `resolve_many_shared` uses the global rayon pool when that feature is on (`resolve.rs:1219`). Idle cost is therefore memory plus one file descriptor.

### 2.3 Encryption, writing, forms, annotations, signatures

| Capability | State | Evidence |
|---|---|---|
| Decrypt with empty password | Works on the indexed route: RC4-40, RC4-128, AES-128, AES-256, and inline `/Encrypt` RC4/AES | Measured on `tests/fixtures_pdf/encrypted/*` |
| User password | `IndexedReaderOptions.password` works; `PasswordRequired` and `InvalidPassword` are typed | Measured on `userpw.pdf`, `inline_encrypt_userpw.pdf` |
| Chunked read of encrypted stream | Refused as `Protected`; must use the materialising path | Measured; `streams.rs:12-24` |
| Public-key security handler | No code found (0 hits for `PubSec`) | `rg` over `src` |
| Incremental save | **Eager only.** `IncrementalDocument` holds `bytes_documents: Vec<u8>` plus a full `Document`, and `save_to` writes the old bytes again | `incremental_document.rs:4-14`, `writer.rs:357` |
| Save correctness | Prefix byte-identical; keeps the xref type (table or stream); `/Prev` set; re-encrypts appended objects when the previous revision was decrypted | Measured; `writer.rs:361-364`, `:396`, `document.rs:84-92` |
| Password on incremental load | None: `password: None` in `load_internal`. Refuses a still-encrypted document | `reader.rs:386-400`, `writer.rs:319` |
| Writing from `IndexedReader` | **None** | API list above |
| `/ByteRange`, signature, CMS | **None** (0 hits in `src`, tests, examples, README, changelog) | `rg` |
| AcroForm | **No typed API** (0 hits). Generic object access reaches it: `/AcroForm`, `/Fields`, `/SigFlags`, `/XFA` all read fine | Measured on IRS 8949, 990 |
| Annotations | `Document::get_page_annotations` is eager only. Generic resolve works on the indexed route: 1,075 widgets across 12 pages in 31 ms. No appearance generation | `document.rs:741`; measured |

### 2.4 Content-stream path: exactly what exists

**In the fork:**
- `Content::decode` (`parser_aux.rs:20`) turns decoded bytes into a `Vec<Operation>`, where `Operation { operator: String, operands: Vec<Object> }` (`content.rs:7-10`, `parser/mod.rs:841`). It always builds the whole list; no streaming iterator exists.
- `Document::extract_text` (`parser_aux.rs:51`) is eager-only and has no positions.
- Encodings and CMaps exist for text decoding (`src/encodings/`). `font_embedding` is for writing fonts.
- **There is no graphics state, CTM, clipping, paths, colour, glyph outlines, compositing or rasteriser.**

**In distillpdf, not the fork,** four separate operator walkers exist for extraction, not painting (`walker.rs:3-6`):
- `text::decode_spans` (`text.rs:1393`): CTM, text state, fonts, ToUnicode, form descent.
- `vector::walk` (`vector.rs:1441`): `q/Q/cm/gs`, paths, colour, transcoded to SVG for figure clusters. "Shadings / patterns / soft masks are out of scope" (`vector.rs:10`).
- `img::walk`: raster placement.
- `extract::walk_drawn`: which images a page paints.

`raster.rs` handles colour spaces and DCT, and `img.rs:174-182` handles CCITT. JPX and JBIG2 are not decoded (`img.rs:180`). Font programs are read only for encoding data (`text.rs:141`). **Neither library produces a pixel.** These walkers could seed an interpreter but are not one. See [DECIDE] 6.

## 3. distillpdf

### 3.1 Rust API

`PdfDocument` (`doc.rs:106`) offers:
- **Open:** `open`, `open_with_engine`, `from_bytes`, `from_bytes_with_engine` (`:969-1025`), and `engine` for the route used (`:1051`).
- **Text:** `page_count`, `extract_text`, `extract_page_text` (`:1071-1107`).
- **Whole-document analysis:** `extract_images`, `extract_fonts`, `extract_tables`, `analyze_tables`, `extract_links`, `render`, `toc`, `outline`, `section`, `front_matter`, `ocr_plan`, `distill` (`:1207-1338`).
- **Diagnostics:** `dbg_spans`, `dbg_spans_xy` (`:1130`, `:1143`).
- **Model types:** `load_dpdf` and `DocModel`, with `Block { page, bbox, kind, text, cells, … }` (`model/mod.rs:93`, `:300-309`).
- **Markdown:** `markdown::html_to_markdown` (`markdown.rs:38`).

The default engine is `Eager` (`doc.rs:704-706`). RectoPDF must call `open_with_engine(path, Engine::Lazy)`. `Lazy` falls back to eager when the indexed open refuses, and it counts that (`doc.rs:984-994`, `:1051`). Of 429 corpus and fixture PDFs, 425 opened lazily. 2 adversarial page trees fell back, and 2 user-password files failed with `Error::Encrypted`, the expected refusal given gap D4 (my probe's `expect` turned that error into a panic; distillpdf did not panic).

### 3.2 The questions asked

| Question | Answer | Source |
|---|---|---|
| One page at a time? | **Plain text only**: `extract_page_text`. Page-range structured output does not exist. `render` and `distill` take no page argument | `doc.rs:1107`, `:1236`, `:1338` |
| Why whole-document? | The pipeline computes body font and cross-page transforms across all pages (chrome, headings, captions) | `html.rs:1307-1340`, `model/build.rs:42` |
| Page numbers kept? | Yes: `Block.page`, `Page.n`, PDF page labels, `<section data-page="N">` | `model/mod.rs:200-205`, `:305`, `html.rs:2741` |
| Positions kept? | Block-level `bbox` in user space, on 97–100% of blocks measured except `attention` (93%) and `rfc9110` (93%). Only reachable through `distill` to a `.dpdf` file and `load_dpdf`; `build_model` is `pub(crate)` | `model/mod.rs:84`, `:309`, `build.rs:42` |
| Word-level positions? | Only `dbg_spans_xy`: `(text, x, y, width, size)`, labelled "Diagnostic". Spans can be single glyphs. `extract_page_text` output carries no link back to spans | `doc.rs:1141-1152` |
| Interruptible? | **No.** No cancel or progress API in `src`. A per-page loop gives page-sized checkpoints. Killing a helper process is the only way to stop `render` or `distill` | `rg` for cancel/progress |
| OCR optional? | **Yes.** The `tesseract` feature is off by default; it builds vendored C/C++ through CMake. Vendored sources 35 MiB, models 9.3 MiB. `ServerOcrEngine` (plain TCP client) is compiled into the default build but runs only if called | `distillpdf/Cargo.toml:22`, `ocr/engine.rs:60-68` |
| Password? | **No public parameter.** `open_with_engine` passes `None` and eager retries only the empty password | `doc.rs:985`, `:673` |
| Threads | Global rayon pool for `extract_text`, `render` and others. It cannot be shut down, so its threads stay idle for the process lifetime, which conflicts with `rust_app_development.md` §3.2. `par_iter` honours a caller's `ThreadPool::install` | `doc.rs:1086`, `html.rs:1329` |

### 3.3 Dependencies and binary size

Release profile as shipped: LTO, `codegen-units=1`, `opt-level=3`, strip.

| Binary | Bytes | Over `hello` |
|---|---|---|
| `hello` (std only) | 302,736 | n/a |
| `lopdf_min`: fork, `default-features = false`, indexed path | 865,520 | +0.54 MiB |
| `lp`: fork with default features, all probe modes | 1,445,760 | +1.09 MiB |
| `dp`: distillpdf + fork, no Tesseract, all probe modes | 4,669,120 | **+4.16 MiB** |

Dependency tree (`cargo tree -e normal`, including the root and proc-macros): fork without defaults **57** packages; distillpdf **92** normal, 95 with build edges. `otool -L` on `dp` lists only system libraries (CoreFoundation, libSystem, libiconv). That rules out third-party dynamic libraries, not statically linked C.

## 4. Measurements

**Method.**
- Throwaway probes in the scratchpad, release build as above, each mode in a fresh process.
- Memory is `phys_footprint` from `proc_pid_rusage(RUSAGE_INFO_V4)`. "Peak" is `ri_lifetime_max_phys_footprint`. Cross-check: `/usr/bin/time -l` reported 19,694,048 B peak where the probe reported 18.77 MiB, a match.
- Baseline process footprint is about 1.75 MiB and is included in the figures.
- Timed runs held the shared heavy lock, so no other heavy job ran, but light activity may have. Sub-millisecond differences are noise.
- **All runs are warm-cache.** See Not tested.

### 4.1 Open: eager versus indexed (median of 3 unless noted, cache 0)

| Document | Pages | Objects | Size MiB | Eager open ms | Eager footprint MiB | Indexed open ms | `page_map` ms | Indexed footprint after map MiB | Read at open MB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `forms_irs_f8949` | 2 | 1,609 | 0.12 | 1.9 | 8.1 | 0.28 | 0.06 | 2.1 | 0.08 |
| `attention_1706.03762` | 15 | 5,957 | 2.1 | 7.2 | 27.5 | 1.44 | 0.12 | 2.3 | 0.31 |
| `standards_ietf_rfc9110` | 194 | 7,952 | 2.7 | 16.9 | 49.0 | 3.04 | 1.30 | 2.9 | 0.58 |
| `sec_moderna_10k` | 244 | 19,722 | 4.2 | 16.6 | 71.4 | 5.16 | 1.41 | 3.3 | 0.98 |
| `sec_spacex_s1_ipo` | 308 | 116,576 | 16.5 | 77.3 | 297.5 | 38.6 | 2.40 | 11.5 | 6.59 |
| `gov_nasa_ntrs20240002628` (most real pages) | 383 | 34,890 | 97.6 | 34.9 | 304.8 | 4.07 | 3.89 | 4.6 | 0.20 |
| `gov_usgs_usgs70277338` (largest file) | 1 | 3,476 | 330.4 | 179.1 | 654.8 (peak 887) | 1.17 | 0.09 | 2.3 | 0.34 |
| synthetic 20k, classic xref | 20,000 | 40,648 | 5.2 | 16.1 | 57.0 | 11.0 | 48.5 | 5.0 | 1.92 |
| synthetic 20k, object streams | 20,000 | 40,856 | 3.3 | 14.6 | 56.8 | 4.07 | 58.3 | 9.2 | 0.87 |
| synthetic 100k, classic xref | 100,000 | 203,230 | 26.2 | 74.7 | 268.7 | 49.2 | 239 | 18.4 | 8.32 |
| synthetic 100k, object streams | 100,000 | 204,264 | 16.7 | 75.7 | 258.2 | 21.5 | 300 | 34.8 | 3.59 |

Notes:
- Eager open is parallel (rayon, 10 cores) and indexed `page_map` is serial, so at 100k pages eager is **faster** end to end (75 ms versus 288–321 ms), at 7–15× the memory. The real corpus has at most 383 pages.
- The corpus has no real PDF above 383 pages. The larger cases are synthetic (`gen_big.py`) and flagged as such.
- `index_stats` estimates are conservative: 32 MB estimated versus about 10 MiB measured on the S-1.
- Inside distillpdf the eager route is slower (S-1: 246 ms) because it loads on a private one-thread pool (`doc.rs:542`).

### 4.2 Page access through the fork: every page once, random order (page dict, streams, Flate, `Content::decode`)

| Document | Cache MiB | Wall ms | p50 ms | p95 ms | Max ms | Peak MiB | Physical reads MiB |
|---|---:|---:|---:|---:|---:|---:|---:|
| rfc9110 | 0 / 8 / 32 | 72 / 70 / 72 | 0.38 | 0.47 | 0.54 | 7.7 / 11.0 / 12.4 | 5.0 / 2.6 / 2.6 |
| moderna | 0 / 8 / 32 | 155 / 154 / 156 | 0.71 | 1.35 | 2.1 | 14.3 / 16.7 / 18.7 | 7.3 / 2.3 / 2.3 |
| spacex | 0 / 8 / 32 | 390 / 402 / 378 | 1.37 | 1.87–2.17 | 2.6–3.8 | 24.9 / 29.3 / 33.3 | 16.6 / 14.4 / 9.1 |
| nasa | 0 / 8 / 32 | 832 / 809 / 838 | 0.25–0.35 | 10.5–11.6 | 50–52 | 247 / 254 / 262 | 45.5 / 59.0 / 51.4 |
| usgs (1 page, 382,527 ops, 4.7 MiB decoded) | 0 / 8 / 32 | 64 / 64 / 67 | 64 | 64 | 64 | 230 / 231 / 231 | 2.3 / 2.8 / 2.8 |
| synthetic 20k | 0 / 8 / 32 | 265 / 215 / 175 | 0.009–0.013 | 0.009–0.014 | 0.08–0.09 | 5.7 / 12.7 / 13.8 | 342 / **689** / 6.2 |

Findings:
- With a warm OS cache, the app-level cache barely changes latency and costs footprint. That matches the §3.2 "let the OS cache" guidance. Cold-disk behaviour is not measured.
- A source cache smaller than the working set can read **more** than no cache (689 versus 342 MiB, synthetic 20k at 8 MiB).
- The AST is the memory problem: about 600 B per operator at peak on the USGS page.

### 4.3 Largest stream: chunked versus materialised

USGS file, largest stream 29.7 MiB encoded (object 95, Flate image):

| Step | Time ms | Footprint after, MiB |
|---|---:|---:|
| Descriptors for all 2,801 streams, no payload | 575 | 3.3 |
| `read_chunk` over the 29.7 MiB | 37.8 | 3.3 (flat) |
| `resolve_object`, materialise | 4.3 | 33.0 |
| `decompressed_content_with_limit` to 87.7 MiB | 230 | 120.8 |

The streaming read costs nothing, but there is no streaming decode to feed it into.

### 4.4 Threads reading different pages at once (one shared reader, 32 MiB cache, random pages)

| Document | 1 thread | 2 | 4 | 8 | Peak MiB at 1 / 8 threads |
|---|---:|---:|---:|---:|---|
| spacex, pages/s | 780 | 1,256 | 1,882 | 2,284 | 34 / 69 |
| nasa, pages/s | 465 | 719 | 1,064 | 1,343 | 705 / 859 |
| synthetic 20k with object streams, pages/s | 110,773 | 116,993 | 135,286 | 178,034 | 16 / 20 |

Concurrent reads work and scale about 2.9× at 8 threads on real documents, on 4P + 6E cores. NASA peaks at 705 MiB here versus 247 MiB in the single pass of 4.2. I did not establish the cause; allocator retention over repeated passes is one candidate.

### 4.5 distillpdf, `Engine::Lazy`: per-page and whole-document

Peak is whole-process, one mode per process.

| Document | Open ms | One page text ms | All pages sequential ms | p50 / p95 / max ms | Peak MiB | `extract_text` (parallel) ms / peak | `render` Page mode ms / peak | `distill` text-only ms / peak | `.dpdf` MB |
|---|---:|---:|---:|---|---:|---|---|---|---:|
| f8949 | 0.35 | 2.2 | 3.5 | 2.1 / 2.1 / 2.1 | 7.8 | 3.7 / 10.8 | 12.6 / 13.6 | 16.8 / 13.1 | 0.03 |
| attention | 1.5 | 1.5 | 36.7 | 1.6 / 6.4 / 8.2 | 11.6 | 19 / 20 | 95 / 25 | 132 / 26 | 0.22 |
| rfc9110 | 4.1 | 1.2 | 187 | 0.97 / 1.3 / 1.5 | 18.3 | 56 / 35 | 245 / 60 | 441 / 62 | 2.6 |
| moderna | 6.3 | 4.1 | 353 | 1.3 / 3.1 / 3.5 | 22.8 | 94 / 59 | 613 / 110 | 849 / 114 | 2.1 |
| spacex | 38 | 4.1 | 821 | 2.9 / 4.2 / 7.0 | 36.3 | 214 / 89 | 1,525 / 197 | 2,070 / 213 | 4.0 |
| nasa | 8.0 | 0.7 | 1,405 | 1.1 / 16.7 / 77.9 | **406** | 451 / 607 | 1,690 / 894 | 3,927 / **981** | 73.8 |
| usgs | 1.3 | 105 | 97 | 97 | **301** | 97 / 301 | 429 / 301 | 2,378 / 312 | 7.0 |
| synthetic 20k | 36 | 0.11 | 582 | 0.029 / 0.032 / 0.13 | 13.5 | 133 / 18 | 596 / 74 | 922 / 114 | 9.0 |

Eager comparison:

| Document, mode | Eager | Lazy |
|---|---|---|
| spacex `render`, peak MiB | 460 | 197 |
| nasa `render`, peak MiB | 1,051 | 894 |
| usgs `onepage`, peak MiB | 1,234 | 304 |
| spacex open, ms | 246 | 38 |
| attention `render`, ms | 39 | 95 |
| rfc9110 `render`, ms | 160 | 245 |
| spacex `render`, ms | 1,356 | 1,525 |
| **synthetic 20k, `extract_page_text` loop** | **38,516 ms** (`get_pages` per call, O(n²)) | **582 ms** |

Output sizes match between engines on every measured document (HTML bytes, text character counts). That is a length check, not a byte diff.

`text_only` `.dpdf` on NASA is 73.8 MB, 76% of the PDF, despite dropping assets. HTML is 69.7 MB there because vector figures render as inline SVG. Use plain text for the corpus, not the model.

### 4.6 Incremental save: measured

| File | Load ms | Save ms | Appended bytes | Prefix identical | Peak footprint MiB | Afterwards |
|---|---:|---:|---:|---|---:|---|
| `forms_irs_f8949` (xref stream) | 2.1 | 0.3 | 686 | yes | 8.6 | poppler parses; indexed re-read sees the change |
| `policy_worldbank_wbD34442284` (linearized, `/DSS`, signature field) | 3.5 | 0.2 | 481 | yes | 15.9 | same |
| `sec_spacex_s1_ipo` | 80.6 | 2.4 | 527 | yes | **340** (see note) | same |

External append while a reader is open: the next resolve returns `indexed source error`, as designed. Peak figures are inflated by the probe: it reads the original and the output whole for the prefix comparison (about 33 MiB on the S-1) and reopens the result. On the S-1, footprint right after `IncrementalDocument::load` was 314 MiB, about 297 MiB without the probe's copy. Use **about 300 MiB** as the cost. `save_to` also writes the whole prefix again.

### 4.7 Search corpus prototype

**Layout.** `corpus/<doc>/pNNNNN.txt`, one file per page from `extract_page_text`, plus `manifest.jsonl` with `{p, f, chars}`. Variant B, `model/<doc>/pNNNNN.md`, has one line per block with `line_bbox` in the manifest. Variant C, `single/<doc>/doc.txt`, is one file with `[page N]` markers.

**Size:**

| Corpus | PDF MB | Pages | Text chars | Apparent MB | On disk MB | Disk / PDF |
|---|---:|---:|---:|---:|---:|---:|
| attention | 2.2 | 15 | 39,435 | 0.04 | 0.07 | 3.1% |
| rfc9110 | 2.9 | 194 | 462,156 | 0.47 | 0.80 | 28% |
| moderna | 4.4 | 244 | 606,165 | 0.62 | 1.35 | 31% |
| spacex | 17.3 | 308 | 1,448,871 | 1.47 | 2.06 | 12% |
| nasa | 102.4 | 383 | 630,353 | 0.65 | 1.46 | 1.4% |
| **bench100 folder, 141 PDFs** | 881.7 | 5,872 | 16,378,691 | 16.8 | 28.8 | **3.3%** |
| spacex, variant B (blocks + bbox manifest) | 17.3 | 308 | 1,515,501 | 1.86 (manifest 0.33) | 2.43 | 14% |
| spacex, variant C (single file) | 17.3 | 308 | 1,452,767 | 1.46 | 1.46 | 8.5% |
| synthetic 20k, per page | 5.4 | 20,000 | 897,788 | 1.69 | **82.7** | 1,530% |
| synthetic 20k, variant C | 5.4 | 20,000 | 1,186,682 | 1.19 | 2.1 | 39% |

Build time, sequential per-page, one process per document: S-1 987 ms, NASA 1,887 ms. All 141 documents took 18.7 s wall on one core at a time, including about 0.06 s per document of harness overhead.

**ripgrep latency** (`/opt/homebrew/bin/rg`, median of 15, p95 within 1–5 ms, spawned from Python so about 6 ms is process start and harness):

| Corpus | Files | Median ms |
|---|---:|---:|
| attention | 16 | 5.9–7.1 |
| rfc9110, moderna, spacex, nasa | 195–384 | 8–12 |
| spacex, single file | 1 | 6.7–7.1 |
| 141-document library | 6,013 | 57–60 |
| synthetic 20k per page, rare term | 20,001 | 200–207 |
| synthetic 20k per page, term on every page | 20,001 | 315 |
| synthetic 20k single file | 1 | 5.4–9.7 |

rg's own cost on the library query: 5.0 MB peak footprint, 7.8 MB RSS (`/usr/bin/time -l`). It runs only during the query, so nothing is resident when idle.

**Token cost (chars / 4):**

| Query | Output | Tokens |
|---|---|---:|
| spacex `Regulation S-K` | locate recipe | 75 |
| spacex `dual.class` | locate | 69 |
| spacex `indemnif` | locate | 708 |
| spacex `Starlink` (346 lines, 100 pages) | raw `rg -n -i` | 11,605 |
| spacex `Starlink` | locate | 6,854 |
| spacex `Starlink` | locate, first 40 lines | 1,373 |
| spacex `Starlink` | `rg -c` | 677 |
| spacex `Starlink` | locate 40 lines + read 3 pages (3 × 1,176) | about 4,900 |
| moderna `mRNA-1273` | locate | 139 |
| rfc9110 `If-None-Match` | locate | 342 |
| library `risk factors` | locate | 59 |
| library `transformer` | locate, first 40 lines | 1,093 |
| spacex `Starlink`, variant B (paragraph per line) | raw | **40,566** |

Locate recipe: `rg -n -i -m 3 --max-columns 200 --max-columns-preview TERM DIR`.

**Whole-document dump for comparison.** The 400,000-character cap is Folio's figure from the brief; I did not verify it.

| Document | Text chars | Tokens uncapped | Tokens at Folio cap | Text lost to cap |
|---|---:|---:|---:|---:|
| attention | 39,435 | 9,858 | 9,858 | 0% |
| rfc9110 | 462,156 | 115,539 | 100,000 | 13.4% |
| moderna | 606,165 | 151,541 | 100,000 | 34.0% |
| nasa | 630,353 | 157,588 | 100,000 | 36.5% |
| spacex | 1,448,871 | 362,217 | 100,000 | **72.4%** |

On the S-1, a selective query is about 1,300× cheaper than the capped dump, a frequent term capped at 40 lines about 73×, and locate plus three pages about 20×. The search also reaches the 72% of the document that a capped dump never sends.

Variant B's one-line-per-block format is costly for agents: paragraph-long lines triple raw output. Plain per-page text has short layout lines. Keep the `--max-columns` flags in the skill.

### 4.8 Hit to highlight region, against poppler word boxes

456 words unique on their page, 12 random pages per document. Documents: `spacex`, `moderna` and three two-column arXiv papers (`cs_CL_2607.28528`, `cs_CV_2607.28589`, `eess_SP_2607.28115`).

| Method | Text found | Region correct | Region size versus word | Cost |
|---|---:|---:|---:|---|
| Block bbox: rg line in variant B to `line_bbox` (±2 pt) | 443/456 | 417/456 = **91.4%** | median 59–132× | whole-document `distill` |
| Spans: `dbg_spans_xy` run per page (±3 pt) | 456/456 | 428/456 = **93.9%** | median 0.9–1.1× | 0.03–90 ms, on demand per page |

The span figure is a lower bound: matching takes the first occurrence in concatenated spans, which can land inside a longer word. The span box height uses `size` above the baseline, so descenders fall outside.

**Failure:** poppler 26.04's `pdftotext -bbox` crashes (`std::out_of_range`) on `attention` and `rfc9110`, so those two are not in this check. Poppler's crash was not investigated.

## 5. Confirmed and not tested

### Confirmed by running it

| Claim | How |
|---|---|
| Indexed open reads a small fraction of a large file, without scanning | Counting source: 0.34 MB of 330 MiB |
| Footprint after open and page map: 2–12 MiB on real files and up to 35 MiB on synthetic ones, against 8–655 MiB eager | 4.1 |
| Encryption on the indexed route: 6 empty-password variants, 2 with user password, wrong password refused | Fixtures |
| Concurrent page reads through one shared reader | 4.4 |
| Chunked encoded read holds footprint flat | 4.3 |
| File change makes reads fail | 4.6 |
| Incremental save preserves the prefix byte for byte, for xref-table, xref-stream and linearized files | 4.6, poppler parse, fork re-read |
| Form, widget and signature-field dictionaries readable through generic resolve | IRS 8949, 990, World Bank file |
| distillpdf lazy route serves 425/429 PDFs | Route check over `benchmarking` and `tests` |
| Eager per-page loop is O(n²) in pages | 38.5 s versus 0.58 s on synthetic 20k |
| Corpus size, rg latency, token cost, highlight accuracy | 4.7, 4.8 |

### Not tested

| Item | Why it matters |
|---|---|
| **Cold disk.** All numbers are warm; `purge` needs sudo | First-open latency, cache sizing and per-page corpus reads could all differ |
| Signature validity after append, by a real validator | Prefix identity implies it for approval signatures, but `/DSS`, DocMDP and `/Perms` handling were not checked by Acrobat or any validator |
| Opening incremental output in Acrobat or Preview | Only poppler and the fork re-read it |
| Incremental save on encrypted files | Repo tests exist in `tests/incremental_document.rs`; not run |
| Fork or distillpdf test suites | Not run; read-only audit |
| Real PDFs over 383 pages or over 330 MiB | None in the repos; scaling is synthetic. Multi-GB files untested |
| Stream over the 64 MiB `resolve_object` cap | Largest real stream is 29.7 MiB. The refusal is read from code, not exercised |
| Public-key encryption | No fixture; no code found |
| Tesseract feature: build, size, runtime | Not built |
| Memory returned after dropping a reader | Idle-tab budget assumes it; `malloc` retention seen in 4.4 |
| Highlight on rotated, RTL or vertical text; `attention` and `rfc9110` | Poppler crash; rotation not handled in the probe |
| Malformed PDFs beyond the repo's adversarial fixtures | No fuzzing |
| Windows or Linux | Mac only. `FileSource` has both code paths |
| 30-day soak, energy, idle wake-ups | Not in this time-box |

## 6. Gaps to close

Effort is rough agent-days, excluding review.

### In the lopdf fork (upstream to the fork unless noted)

| # | Gap | Evidence | Effort | Where |
|---|---|---|---:|---|
| 1 | **Append-only incremental writer over `IndexedReader`.** No `Document`, no whole-file `Vec`, O_APPEND, returns written offsets, preserves xref type, re-encrypts, supports encrypted input. Expose `xref_start` and max object number | About 300 MiB to append 527 B; `open.rs:25` | 4–6 | Fork |
| 2 | **Signature plumbing:** `/ByteRange` and `/Contents` placeholder writing, offset patching, byte-range digest reads | 0 hits | 2–3 | Fork. CMS and PAdES go in a RectoPDF crate: 5–10 more, and a separate track should confirm |
| 3 | **Streaming operator iterator** without an AST, borrowed operands | 230 MiB peak for one page; 859 MiB at 8 threads | 3–5 | Fork; then migrate distillpdf walkers (2–4). Needed regardless of renderer, because it lowers indexing peak |
| 4 | **Streaming filter decode** over chunked reads, including decryption | `streams.rs:126-129`; 121 MiB for one 30 MiB image | 3–5 | Fork |
| 5 | **Reopen after append** without rebuilding: parse only the new xref section | Sticky `SourceChanged`, `source.rs:318-328` | 1–2 | Fork |
| 6 | **Compact xref index**: packed array, not `BTreeMap` | 70–85 B per object; 10 MiB idle on the S-1 | 1–2 | Fork |
| 7 | **Lazy page lookup**: descend by `/Count` with validation, retain the map, typed inherited box, rotate and resources | `page_map` 300 ms at 100k pages; `resolve.rs:1325` walks on every call | 2–3 | Fork. Low urgency: under 4 ms for real files here |
| 8 | **Source-cache sizing fix** or readahead policy | 689 versus 342 MiB read | 1 | Fork, after cold-disk numbers |
| 9 | **Cancellation** on ordinary resolve calls | Permit-only today: `scalar_budget.rs:295` | 1 | Fork |
| 10 | **Typed forms and annotations**: field tree, inheritance, appearance generation, XFA detection, widget edit | 0 AcroForm hits; `document.rs:741` eager only | 8–15 | **RectoPDF crate** on generic resolve, not the fork |
| 11 | Page rendering | Section 2.4 | see [DECIDE] 6 | Not the fork |

### In distillpdf (the owner's library)

| # | Gap | Evidence | Effort |
|---|---|---|---:|
| D1 | **Public per-page spans**: promote `dbg_spans_xy` to a stable API with rotation and page box | `doc.rs:1141` | 1 |
| D2 | **Text-to-position map**: `page_text_with_map(page)` returning text plus byte range to rect | `doc.rs:1107` returns only a `String` | 2–3 |
| D3 | **Per-page or page-range structured output** with degraded cross-page passes, or a two-phase API: document statistics first, then pages | `html.rs:1307`, `build.rs:42` | 5–8 |
| D4 | **Password parameter** on public open | `doc.rs:985` | 0.5 |
| D5 | **Cancellation and progress** on `render` and `distill` | none | 1–2 |
| D6 | **Reuse the page list per call** (no `Vec` clone per page; eager `get_pages` per call is the O(n²) case) | `doc.rs:1108` | 0.5 |
| D7 | **Smaller text-only `.dpdf`**: exclude fidelity SVG | 73.8 MB on NASA | 1–2 |

## 7. [DECIDE] items

### [DECIDE] 1: Corpus file layout
- **Question:** one file per page, or one per document?
- **Options:**
  - **A, per page.** `p00154.txt:12:…` tells the agent the page with no parsing. 8–12 ms per document, 4 KiB minimum per file (20k pages: 83 MB, 205 ms).
  - **B, per document** with page markers. 7 ms at any size, smallest on disk, but hit-to-page needs a manifest or marker lookup, and token cost rises when agents must find markers.
  - **C, per page with a threshold:** B above about 5,000 pages.
- **Recommendation:** A, adding C only when a real long document needs it. The real corpus tops out at 383 pages, where A costs only 3–5 ms more per query.
- **Cost to change later:** low. The corpus is a regenerable cache; skills and the manifest change with it.

### [DECIDE] 2: Highlight mapping
- **Question:** how does a search hit become a highlight rectangle?
- **Options:**
  - **A, block bboxes persisted** from `distill`. Available now, but whole-document, up to 981 MiB, and 59–132× the word's area.
  - **B, spans on demand** for the visible page only, after D1 and D2. 0.03–90 ms, about 1× word area, nothing persisted.
  - **C, persist word boxes** in the index. Instant, but a larger corpus and an index format to maintain.
- **Recommendation:** B. The corpus gives the page, and geometry is computed only for pages on screen, so idle cost stays zero.
- **Cost to change later:** low. No stored format is involved.

### [DECIDE] 3: What feeds the search corpus
- **Question:** plain text or the structured model?
- **Options:**
  - **A, `extract_page_text`.** Per page, interruptible, 13–406 MiB peak, 1.9% of PDF bytes.
  - **B, Markdown or model** as the primary corpus. Headings and tables are kept, but whole-document, 1–6× the peak on big files (S-1 36 versus 213 MiB, USGS 301 versus 312) and worse rg output.
  - **C, A now, and generate B on request** ("give me the tables in this section") in a helper process.
- **Recommendation:** C.
- **Cost to change later:** low; the corpus can be rebuilt.

### [DECIDE] 4: Where distillpdf runs
- **Question:** inside the app process or in a helper?
- **Options:**
  - **A, helper process** (`recto index`, same binary) at background QoS. Kill to cancel, all memory returns on exit, a crash stays contained, and the rayon pool dies with it. Costs a spawn per job plus IPC.
  - **B, in-process thread pool.** Simpler, but no cancellation for `render`, permanent idle pool threads, and a 200 MiB–1 GiB peak inside the viewer that the allocator may not return.
- **Recommendation:** A. It is the only option that meets "an idle tab costs close to nothing" without changes to distillpdf.
- **Cost to change later:** medium. It shapes the job and progress protocol.

### [DECIDE] 5: Save and sign path for the first version
- **Question:** what does form saving and signing use?
- **Options:**
  - **A, build the appending writer in the fork first** (gap 1, 4–6 days).
  - **B, ship `IncrementalDocument` behind a seam,** replacing it later. Output is correct today, but costs about 300 MiB on the S-1 and writes the whole file again on each save.
  - **C, writer inside RectoPDF,** outside the fork. Faster to iterate but needs internals that only the fork exposes.
- **Recommendation:** A if forms or signing are in the first milestone, otherwise B for a spike. Whichever is chosen, validate output with a real signature validator first, since that is not tested here.
- **Cost to change later:** low if saving sits behind one seam; bytes on disk are standard PDF either way.

### [DECIDE] 6: Rendering, which neither library provides
- **Question:** how much of the viewer's engine can rest on this code?
- **Options:**
  - **A, third-party renderer with its own parser.** The fork then handles structure, forms, signing and search only. The file is parsed twice, and gaps 3 and 4 matter less for display but still help indexing.
  - **B, own interpreter on the fork,** seeded by distillpdf's walkers. It needs fonts, a rasteriser, compositing, JPX and JBIG2, and gaps 3 and 4 become mandatory. Large effort.
- **Recommendation:** leave open until the rendering track reports. This audit only establishes that nothing here produces pixels. The "lazy engine" claim holds for structure and objects, not for display.
- **Cost to change later:** high. It sets the core architecture and memory budget.

### [DECIDE] 7: Idle tab policy
- **Question:** does a background tab keep its reader?
- **Options:**
  - **A, drop the reader after a quiet period,** keeping path, page and scroll position. Reopen takes 1–50 ms in measured files plus 0–4 ms for a real page map (up to 300 ms at 100k synthetic pages).
  - **B, keep it.** 2–12 MiB per real document after open and page map, more once caches and pages are touched, plus one file descriptor.
- **Recommendation:** A. A per-tab cache of 8 MiB is a starting point: warm-cache latency does not improve above zero, so the number should be set by the cold-disk measurement.
- **Cost to change later:** low. Whether a dropped reader actually returns memory is still untested.

## 8. Commands

All paths relative to the scratch folder unless absolute. `H=/Volumes/EksternalHome/Koding/Rust/SeedWell/tools/sysguard/heavy.sh`. The machine has no `timeout` binary, so `run.py` enforced a wall limit and a 6 GB RSS ceiling on every probe, and the Bash tool enforced the rest. Probe sources and logs were throwaway scratch; the probe's public API calls are named in sections 2–4.

```sh
# Build (fork patched in by path; CARGO_TARGET_DIR under scratch)
$H cargo build --release -p probe --offline
$H cargo build --release -p lopdf_min --offline
cargo tree --offline -p lopdf_min -e normal --prefix none
cargo tree --offline -p probe -e normal --prefix none

# Synthetic many-page PDFs
/usr/bin/python3 -I gen_big.py 20000 synth/s20k.pdf
/usr/bin/python3 -I gen_big.py 100000 synth/s100k.pdf
target/release/lp modern synth/s20k.pdf synth/s20k_modern.pdf

# lopdf (3 reps of eager and indexed per document)
$H /usr/bin/python3 -I run.py lp
#   runs: lp eager PDF | lp indexed PDF CACHE_MIB | lp scroll PDF CACHE_MIB | lp threads PDF 32 N
target/release/lp enc PDF [PASSWORD]
target/release/lp forms PDF
target/release/lp incr PDF OUT.pdf
target/release/lp changed PDF TMP.pdf
target/release/lp streams PDF

# distillpdf
$H /usr/bin/python3 -I run.py dp
#   runs: dp open|onepage|pagetext|alltext|render PDF eager|lazy [PAGE]; dp model PDF lazy OUT

# Corpus and search
$H /usr/bin/python3 -I run.py corpus     # dp corpus PDF lazy OUT, for every bench100 PDF
$H /usr/bin/python3 -I rgbench.py
/usr/bin/time -l /opt/homebrew/bin/rg -c -i 'risk factors' corpus/lib

# Highlight check
/usr/bin/python3 -I hl.py    # dp spansdump + pdftotext -bbox -f N -l N PDF -
```

## 9. Sources

Primary sources are local code at the commits above; citations are `file:line`.
- Fork: `src/indexed_reader/*`, `src/source.rs`, `src/source_cache.rs`, `src/scalar_budget.rs`, `src/incremental_document.rs`, `src/writer.rs`, `src/reader.rs`, `src/document.rs`, `src/object.rs`, `src/content.rs`, `src/parser_aux.rs`, `docs/indexed-reader-contract.md`.
- distillPDF: `distillpdf/src/{doc,access,html,text,vector,img,raster,walker}.rs`, `distillpdf/src/model/{mod,build}.rs`, `distillpdf/Cargo.toml`, `AGENTS.md`, `ROADMAP.md`, `benchmarking/REPORT.md`.
- Test PDFs: `distillPDF/benchmarking/bench100`, `benchmarking/secondopinion/corpus`, `benchmarking/corpus_tests/corpus`, `tests/fixtures_pdf`.
- Process: `rust_app_development.md` §1, §3.
- ISO 32000-1 is cited only as quoted by the fork's own comments; I did not check the specification itself.
