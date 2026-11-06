# Renderer benchmark spike: PDFium, with hayro as comparison

2026-11-06. Runs decision RD9 option B from `2026-11-06-rendering-interactive.md` §9: required parts only, no MuPDF, no
V8 or XFA builds. Throwaway code (R11). Nothing here is shipped.

## 1. Answer

**PDFium is fast and has a small floor, but two lines of the §9.6 rule fail as written.** On this Mac, text and vector
pages at 2x have a p95 of 8.64 ms against a 150 ms target, and an idle worker costs 2.27 MiB. Opening the 330 MiB file
takes 0.94 ms and 3.8 MiB, close to the lopdf fork's 1.15 ms and 2.4 MiB. What fails:

- **Cancel:** 83.8% of slow pages return within 100 ms; the rule wants 95%. Nothing needed a kill in 297 attempts, and
  the slowest return was 551 ms. A kill takes 0.85 ms and a new worker draws its first tile 13 to 27 ms after that.
- **Memory on heavy image pages:** retiring holds the curve flat, but it can't help one page that needs too much by
  itself. 28 of 5,689 pages exceed the 170 MiB hard budget in a fresh worker, and the worst reaches 493 MiB.

**PDFium's text API returns hidden text.** 14 of 17 hidden-text techniques in our fixture came through, and real PDFs in
the corpus contain such text. The text must not reach an agent unfiltered. A pixel check caught all 14 for about 1 ms per
page after the render.

**Recommendation on RD1: stay with A, PDFium non-V8 in a worker,** with the rule amended as in §12. You decide that: as
written, §9.6 says "not confirmed". hayro is as fast or faster here, which the earlier report did not know, but it has no
cancel call and reached 843 MiB on a file where PDFium peaks at 113 MiB.

## 2. Machine, versions, downloads

| Item | Value |
|---|---|
| Machine | Mac mini, Apple M4, 10 cores, 16 GB (`hw.memsize` 17179869184), macOS 27.0.1 (26A434) |
| Rust | rustc 1.99.1, cargo 1.99.1, release: `opt-level = 3`, thin LTO, `codegen-units = 4`, stripped |
| Swift | `/usr/bin/swiftc -O`, CoreGraphics only |
| Python | `/usr/bin/python3 -I`, standard library plus `ctypes` for `proc_pid_rusage` |
| PDFium | chromium/8125, VERSION 158.0.8125.0; `pdf_enable_v8 = false`, `pdf_enable_xfa = false`, `pdf_use_partition_alloc = false` |
| Crates | `pdfium-render` 0.9.5 (features `pdfium_8125` only), `hayro` 0.9.0 (default features, which embed fonts and CMaps), `memmap2` 0.9.12, `png` 0.18.1, `serde_json`, `libc` |
| lopdf fork | `909771af05c4210db4c138dc2393e5fb6f77b7c7`, exported with `git archive` into scratch |
| Reference for text | `pdftotext` from the Homebrew poppler already on this machine |
| Corpus location | `/Volumes/EksternalHome`, an external volume. All timed runs warm-cache: files read first, and repeats follow |

**Downloads, each in its own new empty folder, code kept elsewhere:**

| File or source | Check | Result |
|---|---|---|
| `pdfium-mac-arm64.tgz`, 3,540,999 B | SHA-256 `4939bd935ac83ddc09a3a4f038bc43a19cea1660316a3667f26cb07a070e139d` | **Matches §9.5** |
| Same | `gh attestation verify --repo bblanchon/pdfium-binaries` | **Passes.** SLSA v1, workflow `build-all.yml@refs/heads/master`, commit `4bdb7acd45b39ef6a7fc2a5b4c83f0398e45c9a6`, run 40312837199. This confirms the build came from that workflow, not that its source is trustworthy |
| Same | `tar -tzf`, 52 entries | No absolute paths or `..` |
| `libpdfium.dylib` | SHA-256 `a45c44645523968525c710462691dd974f4efb1013ce1c249919a0939cfb373a` | Ad hoc signed, arm64 |
| `hayro`, `pdfium-render`, `memmap2` | `Cargo.lock` checksums | **All match §9.5** |
| `pdf-association/pdf20examples` | `c20f2c17…` by `git fetch --depth 1` | 7 PDFs |
| `pdf-association/pdf-differences` | `907fe96e…` | 37 PDFs |
| `mozilla/pdf.js` v6.5.438 | `0a415334…`, sparse | 24 PDFs |
| PDFium `testing/resources`, branch chromium/8138 | `5f092b52…`, sparse | 4 PDFs |

Those 72 outside files are hashed in `outside_files.sha256` in the log folder. The tag and branch pins came from the spec; the
files themselves have no digest in the spec, so their check is the commit id git verified.

**`libpdfium.dylib` links AppKit, CoreGraphics, CoreFoundation and Foundation** (`otool -L`). That is not in the earlier
reports. It did not show up in idle footprint, and the runs never started an application, so nothing appeared in the
Dock. Whether it loads inside the XPC sandbox is not tested.

| Size | Value |
|---|---|
| `libpdfium.dylib` | 7,388,864 B; `__TEXT` 5.8 MB |
| `spike-pdfium` stripped, with `pdfium-render` and JSON | 1.3 MB |
| `spike-hayro` stripped | 5.2 MB |
| Crates in the normal dependency tree | `pdfium-render` 24, `hayro` 76, `lopdf` 66 |

## 3. How it was measured

- **One process for one file or mode.** Memory is `phys_footprint` from `proc_pid_rusage(RUSAGE_INFO_V4)`, read inside the
  process; peak is `ri_lifetime_max_phys_footprint`. Same offsets as the engine audit's probe.
- **Cross-check with `footprint`:** 2385 KB against 2.33 MiB at idle, 3633 KB against 3.55 MiB after open, 19 MB against
  19.25 MiB after 50 pages. They agree.
- **Limits:** the harness polls each child every 20 ms and kills at 120 s (some long runs allowed more; see the code) or at 4 GiB of footprint or RSS.
- **Pages:** 2x, white background, `FPDF_RenderPageBitmap_Start` with the pause callback, then `FPDF_FFLDraw`. Time covers
  bitmap creation, `FPDF_LoadPage`, drawing and `FPDF_FFLDraw`. No file writes in timed runs.
- **Repeats:** PDFium over all of bench100, 3 repeats; hayro over all of bench100, 1 repeat; named tiers interleaved A, B, 3
  repeats. Open 7 repeats (5 on synthetic files). Medians are per page across repeats, then p50 and p95 across pages.
- **Lock:** every build and benchmark ran through `heavy.sh`, one heavy job at a time, shared with another agent.

**Deviations from §9, with reasons:**

| Spec | Done | Why |
|---|---|---|
| `spikes/render-compare/` in the repo | Scratch folder | Your instruction |
| `load_pdf_from_reader` | `FPDF_LoadCustomDocument` with a `pread` callback through the same crate's bindings | `PdfDocument::handle()` is `pub(crate)`, so that loader can't be combined with raw calls. Same PDFium path, and closer to the planned worker |
| Three repeats for all | hayro 1 repeat on the full corpus | Time-box. 3 on named tiers |
| Footprint peak from the harness | Internal lifetime peak | The 20 ms poll misses short processes. Early tables built from it were replaced |
| Review of all suspects | See §9 | Time-box and a noisy threshold |
| Committed `Cargo.lock`, `cargo fetch --locked` | `Cargo.lock` created here; the three named crates checked against §9.5 | There was no earlier lockfile. Other dependencies have only their crates.io checksums |
| No credentials in the build environment, no network after fetch | Build scripts ran in my normal shell, not scrubbed or offline. Benchmarks ran with a minimal environment (`PATH`, `HOME=/nonexistent`, `TMPDIR`) | Not done for builds. A deviation from §9.5 step 5 |
| Fetch two `xfa_*.pdf.link`, PDF-Attacker samples | Not fetched | Out of scope (no XFA build) and time-box |

**Things that could have touched timings.** During the first timed PDFium pass I also ran three `git fetch` commands and small
fixture commands outside the lock. Other agents' jobs were kept out only by the lock, and normal background processes ran.
Repeat-level spread is shown below. Medians are stable; single slow pages vary more: the USGS page read 746 to 939 ms in one
batch and 676 ms in another.

## 4. Speed at 2x

Rows use medians per page across 3 repeats over the first 200 pages of each of 141 files. "Text and vector" means no image and no shading object on
the page, including inside form XObjects, from `FPDFPage_GetObject` and `FPDFPageObj_GetType`.

| pool | pages | p50 ms | p95 ms | p99 ms | max ms | total s |
|---|---:|---:|---:|---:|---:|---:|
| all bench100 pages (≤200 each) | 5689 | 2.41 | 30.39 | 109.59 | 922.22 | 55.5 |
| **text and vector: no image, no shading** | 4401 | 2.06 | 8.64 | 26.63 | 161.50 | 14.8 |
| no image (shadings allowed) | 4418 | 2.06 | 8.94 | 26.61 | 161.50 | 15.0 |
| text-dominant: no image, no shading, <500 paths | 4307 | 2.02 | 6.98 | 23.85 | 40.48 | 12.5 |
| with images | 1271 | 12.45 | 96.11 | 441.60 | 922.22 | 40.5 |
| images under 10% of page area | 403 | 6.12 | 30.50 | 119.58 | 224.63 | 4.9 |

**Repeat-level spread:**

| repeat | pages | p50 ms | p95 ms | p99 ms | max ms | total s |
|---|---:|---:|---:|---:|---:|---:|
| r1 | 5689 | 2.38 | 28.97 | 107.68 | 745.71 | 53.9 |
| r2 | 5689 | 2.48 | 30.98 | 113.41 | 922.22 | 57.0 |
| r3 | 5689 | 2.69 | 33.33 | 109.85 | 939.39 | 61.7 |

**Slowest text and vector pages:**

| ms | doc | page | objects |
|---:|---|---:|---|
| 161.5 | un_report_un_sdg2023 | 26 | text 120 path 1874 |
| 116.3 | math_NA_2607.28104 | 32 | text 92 path 115366 |
| 102.7 | gov_nasa_ntrs20240002628 | 111 | text 165 path 11013 |
| 100.8 | gov_nasa_ntrs20240002628 | 30 | text 25 path 16244 |
| 92.8 | un_report_un_sdg2024 | 22 | text 129 path 1389 |
| 85.5 | cs_DS_2607.28298 | 28 | text 89 path 655 |
| 72.3 | gov_usgs_usgs70275236 | 18 | text 43 path 123592 |
| 70.7 | cs_CL_2607.28607 | 3 | text 397 path 3097 |
| 58.5 | gov_nasa_ntrs20240002628 | 80 | text 12 path 33326 |
| 44.2 | gov_nasa_ntrs20240002628 | 35 | text 19 path 6167 |
| 40.5 | cs_SE_2607.28587 | 0 | text 173 path 12 |
| 40.1 | gov_usgs_usgs70275236 | 13 | text 156 path 2923 |
| 39.6 | gov_nasa_ntrs20240002628 | 141 | text 0 path 10333 |
| 38.1 | cs_SE_2607.28424 | 0 | text 137 path 12 |
| 35.1 | math_NA_2607.28456 | 0 | text 148 path 1 |

**Slowest of all:** the USGS map and `gov_nasa_ntrs20220005969`'s page backgrounds, which combine one image with 72 shadings.

**PDFium against hayro, same pages:**

| pool | backend | pages | p50 | p95 | p99 | max | total s |
|---|---|---:|---:|---:|---:|---:|---:|
| all common pages | pdfium | 5689 | 2.41 | 30.39 | 109.59 | 922.22 | 55.5 |
| all common pages | hayro | 5689 | 2.20 | 24.55 | 87.40 | 776.16 | 44.3 |
| text and vector | pdfium | 4401 | 2.06 | 8.64 | 26.63 | 161.50 | 14.8 |
| text and vector | hayro | 4401 | 1.98 | 4.64 | 18.27 | 74.28 | 11.0 |
| with images | pdfium | 1271 | 12.45 | 96.11 | 441.60 | 922.22 | 40.5 |
| with images | hayro | 1271 | 11.39 | 77.19 | 378.28 | 776.16 | 33.2 |

**Named tiers, interleaved, 3 repeats:**

| doc | backend | pages | p50 | p95 | max | peak fp MiB |
|---|---|---:|---:|---:|---:|---:|
| usgs | pdfium | 1 | 675.92 | 675.92 | 675.92 | 493.1 |
| usgs | hayro | 1 | 430.40 | 430.40 | 430.40 | 258.2 |
| nasa | pdfium | 200 | 4.84 | 49.22 | 137.71 | 260.9 |
| nasa | hayro | 200 | 2.85 | 43.38 | 217.33 | 277.8 |
| rfc9110 | pdfium | 194 | 1.35 | 1.72 | 8.03 | 30.0 |
| rfc9110 | hayro | 194 | 1.84 | 2.21 | 3.42 | 26.8 |
| spacex | pdfium | 200 | 5.43 | 7.53 | 13.23 | 37.6 |
| spacex | hayro | 200 | 3.18 | 4.39 | 7.34 | 70.2 |
| moderna | pdfium | 200 | 3.08 | 6.32 | 86.07 | 26.9 |
| moderna | hayro | 200 | 1.86 | 3.66 | 5.73 | 45.5 |
| cs_CV_2607.28611 | pdfium | 40 | 5.74 | 110.18 | 188.67 | 394.4 |
| cs_CV_2607.28611 | hayro | 40 | 3.91 | 145.01 | 592.10 | 569.8 |
| gov_usgs_usgs70278165 | pdfium | 98 | 3.82 | 52.36 | 260.43 | 112.8 |
| gov_usgs_usgs70278165 | hayro | 98 | 3.32 | 37.53 | 775.53 | 843.0 |
| eu_law_eu_aiact | pdfium | 144 | 2.36 | 3.47 | 18.86 | 22.5 |
| eu_law_eu_aiact | hayro | 144 | 2.28 | 3.32 | 16.89 | 32.6 |

**Scaling.** PDFium in processes, hayro in threads with one `RenderCache` per thread, first 200 pages:

| kind | doc | n | wall s p50 | throughput vs n=1 |
|---|---|---:|---:|---:|
| hayro threads | cv | 1 | 1.70 | 1.00× |
| hayro threads | cv | 2 | 0.99 | 1.72× |
| hayro threads | cv | 4 | 0.77 | 2.22× |
| hayro threads | moderna | 1 | 0.36 | 1.00× |
| hayro threads | moderna | 2 | 0.22 | 1.64× |
| hayro threads | moderna | 4 | 0.15 | 2.49× |
| hayro threads | rfc9110 | 1 | 0.34 | 1.00× |
| hayro threads | rfc9110 | 2 | 0.21 | 1.65× |
| hayro threads | rfc9110 | 4 | 0.14 | 2.41× |
| pdfium processes | cv | 1 | 0.97 | 1.00× |
| pdfium processes | cv | 2 | 1.16 | 1.67× |
| pdfium processes | cv | 4 | 1.36 | 2.86× |
| pdfium processes | moderna | 1 | 0.70 | 1.00× |
| pdfium processes | moderna | 2 | 0.80 | 1.76× |
| pdfium processes | moderna | 4 | 0.95 | 2.97× |
| pdfium processes | rfc9110 | 1 | 0.29 | 1.00× |
| pdfium processes | rfc9110 | 2 | 0.37 | 1.59× |
| pdfium processes | rfc9110 | 4 | 0.44 | 2.66× |

**Tiles.** One 512 × 512 tile against a full page at 4x. Each is a fresh process, so times include `FPDF_LoadPage` and first font and
image work:

| doc | backend | kind | pixels | n | ms p50 | ms p95 | peak fp MiB |
|---|---|---|---|---:|---:|---:|---:|
| cv | hayro | full | 2448×3168 | 5 | 12.02 | 12.37 | 44.5 |
| cv | hayro | tile | 512×512 | 5 | 5.95 | 5.99 | 10.6 |
| cv | pdfium | full | 2448×3168 | 5 | 15.40 | 16.62 | 38.3 |
| cv | pdfium | tile | 512×512 | 5 | 8.98 | 9.67 | 8.6 |
| nasa | hayro | full | 2448×3168 | 5 | 8.93 | 12.40 | 45.4 |
| nasa | hayro | tile | 512×512 | 5 | 3.53 | 3.56 | 12.8 |
| nasa | pdfium | full | 2448×3168 | 5 | 10.67 | 10.82 | 39.6 |
| nasa | pdfium | tile | 512×512 | 5 | 4.83 | 4.95 | 10.0 |
| rfc9110 | hayro | full | 2380×3364 | 5 | 5.82 | 5.89 | 37.2 |
| rfc9110 | hayro | tile | 512×512 | 5 | 1.45 | 1.47 | 5.8 |
| rfc9110 | pdfium | full | 2380×3364 | 5 | 9.46 | 9.75 | 37.0 |
| rfc9110 | pdfium | tile | 512×512 | 5 | 4.09 | 4.23 | 6.6 |
| spacex | hayro | full | 2448×3168 | 5 | 8.60 | 8.95 | 49.9 |
| spacex | hayro | tile | 512×512 | 5 | 3.49 | 3.52 | 17.2 |
| spacex | pdfium | full | 2448×3168 | 5 | 15.83 | 17.04 | 46.3 |
| spacex | pdfium | tile | 512×512 | 5 | 5.15 | 5.25 | 16.8 |
| usgs | hayro | full | 8928×8064 | 5 | 652.20 | 658.29 | 779.0 |
| usgs | hayro | tile | 512×512 | 5 | 404.79 | 409.40 | 444.2 |
| usgs | pdfium | full | 8928×8064 | 5 | 900.89 | 908.49 | 980.5 |
| usgs | pdfium | tile | 512×512 | 5 | 273.86 | 276.91 | 323.5 |

## 5. Open time and footprint, next to the lopdf fork

Same method as the audit's §4.1: release build, fresh process, warm cache, `phys_footprint` after open and page table. PDFium's
page table is `FPDF_GetPageSizeByIndex` for every page, which is the closest equivalent to `page_map()`. PDFium's and lopdf's run
in the same batch, 7 repeats, median. **The lopdf row was rerun here with a new probe using the same calls,** and it matches the audit:
1.15 ms against 1.17 ms on the large file, 36.3 ms against 38.6 ms on the S-1.

| doc | backend | n | pages | open ms p50 | open ms p95 | page table ms p50 | fp after open MiB | fp after page table MiB | open + page 1 at 2x ms | fp after page 1 MiB | MiB read at open |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| f8949 | pdfium | 7 | 2 | 0.40 | 0.41 | 0.03 | 2.77 | 2.77 | 26.81 | 15.88 | 0.02 |
| f8949 | lopdf | 7 | 2 | 0.24 | 0.46 | 0.05 | 2.06 | 2.09 | n/a | n/a | 0.09 |
| f8949 | hayro | 7 | 2 | 0.14 | 1.61 | 0.00 | 1.86 | 1.86 | 5.98 | 15.95 | 0.00 |
| attention | pdfium | 7 | 15 | 0.35 | 0.36 | 0.14 | 2.88 | 2.92 | 20.05 | 15.77 | 0.13 |
| attention | lopdf | 7 | 15 | 1.33 | 1.38 | 0.12 | 2.23 | 2.36 | n/a | n/a | 0.38 |
| attention | hayro | 7 | 15 | 0.17 | 0.17 | 0.00 | 2.16 | 2.16 | 5.35 | 15.02 | 0.00 |
| rfc9110 | pdfium | 7 | 194 | 0.67 | 0.68 | 1.99 | 3.47 | 4.33 | 10.52 | 14.69 | 0.46 |
| rfc9110 | lopdf | 7 | 194 | 2.95 | 2.99 | 1.27 | 2.67 | 2.94 | n/a | n/a | 1.36 |
| rfc9110 | hayro | 7 | 194 | 0.62 | 0.68 | 0.00 | 3.19 | 3.19 | 4.16 | 14.27 | 0.00 |
| moderna | pdfium | 7 | 244 | 1.27 | 1.37 | 1.49 | 4.05 | 4.70 | 19.54 | 21.25 | 0.47 |
| moderna | lopdf | 7 | 244 | 5.09 | 5.32 | 1.45 | 3.12 | 3.34 | n/a | n/a | 2.10 |
| moderna | hayro | 7 | 244 | 0.70 | 0.71 | 0.00 | 4.11 | 4.11 | 6.55 | 16.83 | 0.00 |
| spacex | pdfium | 7 | 308 | 13.74 | 14.16 | 2.34 | 12.16 | 13.27 | 29.56 | 26.44 | 2.36 |
| spacex | lopdf | 7 | 308 | 36.27 | 36.96 | 2.39 | 11.23 | 11.48 | n/a | n/a | 7.75 |
| spacex | hayro | 7 | 308 | 2.98 | 3.02 | 0.00 | 15.38 | 15.38 | 10.55 | 27.25 | 0.00 |
| nasa | pdfium | 7 | 383 | 2.73 | 2.78 | 8.90 | 5.49 | 9.48 | 19.22 | 19.92 | 0.16 |
| nasa | lopdf | 7 | 383 | 3.89 | 4.09 | 3.70 | 4.28 | 4.72 | n/a | n/a | 0.24 |
| nasa | hayro | 7 | 383 | 3.21 | 3.25 | 0.00 | 7.81 | 7.81 | 10.60 | 19.38 | 0.00 |
| usgs | pdfium | 7 | 1 | 0.94 | 0.98 | 0.07 | 3.78 | 3.81 | 673.49 | 492.70 | 0.15 |
| usgs | lopdf | 7 | 1 | 1.15 | 1.20 | 0.08 | 2.19 | 2.36 | n/a | n/a | 0.34 |
| usgs | hayro | 7 | 1 | 0.13 | 0.13 | 0.00 | 1.92 | 1.92 | 431.58 | 258.08 | 0.00 |
| s20k | pdfium | 5 | 20000 | 3.25 | 3.38 | 45.88 | 5.83 | 17.55 | 62.82 | 27.22 | 4.45 |
| s20k | lopdf | 5 | 20000 | 10.40 | 10.63 | 45.77 | 4.42 | 5.00 | n/a | n/a | 87.53 |
| s20k | hayro | 5 | 20000 | 10.76 | 10.94 | 0.00 | 49.20 | 49.20 | 11.96 | 57.02 | 0.00 |
| s100k | pdfium | 5 | 100000 | 28.14 | 28.51 | 268.80 | 19.91 | 78.14 | 309.40 | 88.73 | 23.77 |
| s100k | lopdf | 5 | 100000 | 47.81 | 49.07 | 230.93 | 16.67 | 18.42 | n/a | n/a | 436.38 |
| s100k | hayro | 5 | 100000 | 54.43 | 55.38 | 0.00 | 226.56 | 226.56 | 55.44 | 234.38 | 0.00 |

| Question | Answer |
|---|---|
| 330 MiB file, open and page table | **PDFium 0.94 + 0.07 ms, 3.81 MiB.** lopdf 1.15 + 0.08 ms, 2.36 MiB |
| S-1, open and page table | **PDFium 13.74 + 2.34 ms, 13.27 MiB.** lopdf 36.27 + 2.39 ms, 11.48 MiB |
| Difference that matters | PDFium also needs the page for pixels: first page of the 330 MiB file is 673 ms and 493 MiB |
| Not comparable | lopdf's page table has no sizes resolved. PDFium's call resolves and returns them. hayro's open includes every page |

**Budget note:** the 2 GB budget's file isn't in the corpus. On synthetic 100k-page files PDFium's page-size walk is 269 ms and 78 MiB.

## 6. Footprint

**Idle worker:**

| what | n | median | max |
|---|---:|---:|---:|
| pdfium_dlopen | 7 | 2.266 | 2.282 |
| pdfium_main_start | 7 | 1.766 | 1.766 |
| pdfium_init | 7 | 2.266 | 2.297 |
| dlopen_ms | 7 | 1.596 | 2.539 |
| init_ms | 7 | 0.031 | 1.807 |
| pdfium | 7 | 2.266 | 2.297 |
| hayro | 7 | 1.594 | 1.594 |

Reference: the empty C XPC service measured 2.17 to 2.20 MiB (platform §1.6). This is a plain process over pipes, not XPC.

**Worker cycle,** kill, respawn, reopen, first 512 px tile at 2x, 20 runs per document after one warm-up. `kill_ms` is
`SIGKILL` to `wait` returning:

| doc | measure | n | p50 | p95 | max |
|---|---|---:|---:|---:|---:|
| rfc9110 | spawn_ready_ms | 20 | 7.01 | 7.48 | 9.67 |
| rfc9110 | open_ms | 20 | 0.70 | 0.72 | 0.73 |
| rfc9110 | tile_ms | 20 | 5.49 | 6.06 | 6.50 |
| rfc9110 | respawn_to_tile_ms | 20 | 13.28 | 14.15 | 16.31 |
| rfc9110 | kill_ms | 20 | 0.83 | 0.88 | 0.99 |
| rfc9110 | outside_after_tile_fp | 20 | 6.89 | 6.97 | 6.99 |
| rfc9110 | outside_after25_fp | 20 | 18.58 | 18.84 | 18.84 |
| rfc9110 | ready_fp (inside) | 20 | 2.28 | 2.31 | 2.31 |
| spacex | spawn_ready_ms | 20 | 7.16 | 7.91 | 8.50 |
| spacex | open_ms | 20 | 13.58 | 14.84 | 15.61 |
| spacex | tile_ms | 20 | 6.39 | 6.90 | 7.32 |
| spacex | respawn_to_tile_ms | 20 | 27.13 | 28.97 | 29.64 |
| spacex | kill_ms | 20 | 0.86 | 1.05 | 1.21 |
| spacex | outside_after_tile_fp | 20 | 18.02 | 18.13 | 18.16 |
| spacex | outside_after25_fp | 20 | 30.78 | 30.88 | 31.02 |
| spacex | ready_fp (inside) | 20 | 2.29 | 2.31 | 2.31 |

**Curve over many pages.** Figures are `pages:MiB`, whole document drawn three times over, then close. Median run shown.

**pdfium:plain:rfc9110** (3 runs, status ok, peak 29.88 MiB, wall 0.774 s): 1:13.9, 25:17.6, 50:18.9, 75:20.2, 100:21.4, 125:22.8, 150:24.6, 175:26.5, 200:29.6, 225:29.6, 250:29.6, 275:29.6, 300:29.6, 325:29.6, 350:29.6, 375:29.7, 400:29.7, 425:29.7, 450:29.7, 475:29.7, 500:29.7, 525:29.7, 550:29.7, 575:29.8, 575:29.8

  peak by run: [29.95, 29.88, 30.22]; before close [29.876], after close [29.876], after pressure relief [29.876]

**pdfium:reopen50:rfc9110** (3 runs, status ok, peak 23.42 MiB, wall 0.942 s): 1:13.9, 25:17.7, 50:19.0, 75:19.1, 100:19.2, 125:19.4, 150:19.4, 175:19.8, 200:22.0, 225:22.1, 250:22.1, 275:22.1, 300:22.2, 325:22.2, 350:22.2, 375:22.3, 400:23.1, 425:23.2, 450:23.2, 475:23.3, 500:23.3, 525:23.4, 550:23.4, 575:23.4, 575:23.4

  peak by run: [23.61, 23.42, 23.64]; before close [23.423], after close [23.423], after pressure relief [23.423]; reopen samples after_close [18.985, 19.188, 19.407, 22.048, 22.11, 22.157, 22.188, 23.141]

**hayro:plain:rfc9110** (3 runs, status ok, peak 24.66 MiB, wall 0.358 s): 1:14.2, 25:18.0, 50:19.2, 75:20.8, 100:21.7, 125:22.5, 150:23.5, 175:24.2

  peak by run: [25.73, 24.66, 24.28]; before close [24.875], after close [24.875], after pressure relief []

**pdfium:plain:spacex** (3 runs, status ok, peak 47.25 MiB, wall 5.681 s): 1:25.4, 50:31.5, 100:35.8, 150:36.6, 200:37.6, 250:43.5, 300:44.6, 350:45.7, 400:45.9, 450:46.9, 500:46.9, 550:47.2, 600:47.2, 650:47.2, 700:47.2, 750:47.2, 800:47.2, 850:47.2, 900:47.2, 900:47.2

  peak by run: [44.13, 47.25, 46.99]; before close [47.219], after close [47.219], after pressure relief [47.219]

**pdfium:reopen50:spacex** (3 runs, status ok, peak 39.39 MiB, wall 6.08 s): 1:25.7, 50:31.8, 100:34.3, 150:34.6, 200:34.9, 250:35.9, 300:36.3, 350:36.9, 400:36.9, 450:37.0, 500:37.0, 550:37.0, 600:37.0, 650:38.2, 700:38.2, 750:38.2, 800:38.2, 850:38.2, 900:38.2, 900:38.2

  peak by run: [39.19, 39.39, 38.58]; before close [39.391], after close [39.391], after pressure relief [39.391]; reopen samples after_close [31.782, 34.251, 34.626, 34.938, 35.938, 36.251, 36.923, 36.938]

**hayro:plain:spacex** (3 runs, status ok, peak 92.83 MiB, wall 0.988 s): 1:27.2, 25:34.5, 50:39.6, 75:46.1, 100:52.2, 125:60.1, 150:64.5, 175:65.4, 200:69.7, 225:77.7, 250:82.1, 275:87.7, 300:92.5

  peak by run: [89.72, 92.83, 92.25]; before close [96.61], after close [96.61], after pressure relief []

**pdfium:plain:nasa** (3 runs, status ok, peak 487.61 MiB, wall 13.777 s): 1:16.1, 75:168.2, 150:218.3, 225:297.1, 300:388.1, 375:441.5, 450:456.5, 525:340.8, 600:368.8, 675:376.6, 750:434.3, 825:434.2, 900:466.2, 975:466.2, 1050:466.3, 1125:466.3, 1125:466.3

  peak by run: [485.14, 487.61, 489.64]; before close [466.298], after close [241.407], after pressure relief [241.407]

**pdfium:reopen50:nasa** (3 runs, status ok, peak 229.31 MiB, wall 15.389 s): 1:16.2, 75:151.4, 150:160.5, 225:108.5, 300:170.5, 375:188.9, 450:189.0, 525:223.3, 600:78.8, 675:85.1, 750:148.1, 825:148.1, 900:181.0, 975:181.0, 1050:181.1, 1125:181.2, 1125:181.2

  peak by run: [236.36, 229.31, 229.66]; before close [181.173], after close [181.173], after pressure relief [181.173]; reopen samples after_close [88.36, 79.251, 38.86, 104.36, 112.751, 170.485, 188.86, 188.97]

**hayro:plain:nasa** (3 runs, status ok, peak 454.05 MiB, wall 4.119 s): 1:19.5, 25:29.8, 50:104.5, 75:170.2, 100:207.2, 125:251.1, 150:190.8, 175:215.8, 200:247.7, 225:268.2, 250:270.8, 275:318.0, 300:366.3, 325:408.5, 350:445.8, 375:453.6, 375:453.6

  peak by run: [451.78, 454.05, 455.24]; before close [454.048], after close [207.032], after pressure relief []

**pdfium:plain:moderna** (3 runs, status ok, peak 31.52 MiB, wall 5.046 s): 1:20.7, 50:24.1, 100:24.5, 150:25.2, 200:26.8, 250:27.8, 300:28.0, 350:28.0, 400:28.0, 450:28.3, 500:30.0, 550:30.0, 600:30.0, 650:30.0, 700:30.0, 725:30.0

  peak by run: [33.17, 31.52, 29.66]; before close [31.516], after close [31.516], after pressure relief [31.516]

**pdfium:reopen50:moderna** (3 runs, status ok, peak 30.06 MiB, wall 5.162 s): 1:20.8, 50:24.1, 100:24.7, 150:25.2, 200:26.1, 250:29.9, 300:29.9, 350:29.9, 400:29.9, 450:30.0, 500:30.0, 550:30.0, 600:30.0, 650:30.0, 700:30.1, 725:30.1

  peak by run: [30.19, 30.06, 33.17]; before close [30.063], after close [30.063], after pressure relief [30.063]; reopen samples after_close [24.094, 24.704, 25.188, 26.11, 29.923, 29.923, 29.923, 29.938]

**hayro:plain:moderna** (3 runs, status ok, peak 49.59 MiB, wall 0.497 s): 1:16.7, 25:23.5, 50:28.1, 75:32.2, 100:32.7, 125:36.9, 150:41.0, 175:41.4, 200:45.4, 225:45.6

  peak by run: [46.8, 49.59, 47.02]; before close [49.594], after close [49.594], after pressure relief []

What the curves show:

- **Growth follows new pages and stops when pages repeat.** `rfc9110` goes from 13.9 to 29.6 MiB across its 194 pages, then stays
  within 0.3 MiB through two more passes. The S-1 is flat at 47.2 MiB from page 550.
- **Closing the document doesn't lower the footprint on text documents.** `FPDF_CloseDocument`, `FPDF_DestroyLibrary` and
  `malloc_zone_pressure_relief` all left it unchanged at 29.9 MiB; on NASA close gave 466 to 241 MiB. In-process reopening every 50
  pages does cap it, because freed memory is reused: NASA peak 229 MiB against 488 MiB.
- **Only exiting returns it all.** The next process starts at 2.3 MiB. That is what "retire" must mean.

**Without the full page bitmap.** About 7.6 MiB of each curve above is the page bitmap, which production replaces with tiles. One 512 px tile
per page instead:

rfc9110 194 open_fp 3.516 pts [[1, 6.907], [25, 10.704], [75, 13.235], [125, 15.829], [175, 19.579], [175, 19.579]] end 22.69

moderna 244 open_fp 4.047 pts [[1, 8.251], [25, 12.001], [75, 12.751], [125, 14.532], [175, 15.782], [225, 17.11], [225, 17.11]] end 17.8

spacex 308 open_fp 12.219 pts [[1, 18.001], [25, 23.751], [75, 26.516], [125, 27.469], [175, 28.282], [225, 29.469], [275, 31.001], [300, 31.673]] end 32.0

attention 15 open_fp 2.891 pts [[1, 8.923], [1, 8.923]] end 37.08

f8949 2 open_fp 2.813 pts [[1, 9.048], [1, 9.048]] end 9.49

The estimate of "≤ 15 renderer heap and fonts" (platform §4) holds for `rfc9110` to about page 100 and `moderna` to about page 150. The S-1 is 18.0 MiB after its first tile, 12.2 MiB of that from opening its 116,576 objects. Retiring can't get under that floor. Against the 60 MiB total that is 3 MiB over the heap's share.

**Retire policy.** Kill and respawn when footprint after a page exceeds the limit, then continue. Full 2x bitmaps, first 200 pages:

| doc | pages | retire above MiB | retires | peak MiB | median fp | p95 fp | end fp | total s | respawn s | worst page |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| cv28611 | 40 | 128 | 3 | 278.1 | 47.9 | 161.7 | 53.5 | 1.088 | 0.029 | 4 |
| cv28611 | 40 | 64 | 4 | 277.8 | 45.3 | 132.5 | 53.4 | 1.104 | 0.037 | 4 |
| cv28611 | 40 | 48 | 6 | 278.1 | 39.3 | 132.6 | 25.6 | 1.162 | 0.054 | 4 |
| cv28611 | 40 | none | 0 | 393.3 | 330.6 | 393.3 | 393.3 | 0.963 | 0.0 | 39 |
| moderna | 200 | 128 | 0 | 26.8 | 24.8 | 26.8 | 26.8 | 0.695 | 0.0 | 196 |
| moderna | 200 | 64 | 0 | 27.1 | 24.7 | 27.0 | 27.1 | 0.698 | 0.0 | 198 |
| moderna | 200 | 48 | 0 | 31.7 | 29.5 | 31.6 | 31.7 | 0.709 | 0.0 | 198 |
| moderna | 200 | none | 0 | 30.2 | 27.9 | 30.1 | 30.2 | 0.722 | 0.0 | 198 |
| nasa2628 | 200 | 128 | 4 | 148.2 | 82.6 | 125.1 | 57.8 | 2.76 | 0.047 | 50 |
| nasa2628 | 200 | 64 | 13 | 115.8 | 39.9 | 65.2 | 52.9 | 3.068 | 0.147 | 50 |
| nasa2628 | 200 | 48 | 21 | 114.7 | 33.6 | 56.2 | 29.1 | 3.231 | 0.237 | 50 |
| nasa2628 | 200 | none | 0 | 260.2 | 163.5 | 256.0 | 260.2 | 2.695 | 0.0 | 199 |
| nasa5693 | 152 | 128 | 4 | 161.4 | 73.6 | 125.7 | 138.7 | 3.011 | 0.049 | 122 |
| nasa5693 | 152 | 64 | 16 | 137.5 | 45.3 | 70.0 | 70.7 | 3.189 | 0.151 | 122 |
| nasa5693 | 152 | 48 | 25 | 114.5 | 40.3 | 67.1 | 69.3 | 3.415 | 0.242 | 122 |
| nasa5693 | 152 | none | 0 | 280.5 | 187.9 | 278.8 | 280.5 | 2.799 | 0.0 | 151 |
| nasa5969 | 196 | 128 | 22 | 325.9 | 48.0 | 275.6 | 192.7 | 14.377 | 0.284 | 174 |
| nasa5969 | 196 | 64 | 26 | 301.4 | 44.9 | 276.8 | 192.8 | 14.621 | 0.326 | 174 |
| nasa5969 | 196 | 48 | 35 | 301.4 | 39.1 | 275.0 | 186.9 | 15.185 | 0.427 | 174 |
| nasa5969 | 196 | none | 0 | 405.2 | 351.4 | 380.9 | 405.2 | 13.322 | 0.0 | 195 |
| qbio28601 | 35 | 128 | 3 | 322.7 | 79.4 | 193.8 | 13.5 | 0.673 | 0.04 | 8 |
| qbio28601 | 35 | 64 | 7 | 260.6 | 41.1 | 125.3 | 13.6 | 0.707 | 0.063 | 8 |
| qbio28601 | 35 | 48 | 8 | 261.2 | 34.7 | 119.8 | 13.4 | 0.74 | 0.075 | 8 |
| qbio28601 | 35 | none | 0 | 545.4 | 370.6 | 545.3 | 545.4 | 0.584 | 0.0 | 34 |
| rfc9110 | 194 | 128 | 0 | 29.7 | 21.5 | 28.6 | 29.7 | 0.292 | 0.0 | 192 |
| rfc9110 | 194 | 64 | 0 | 29.7 | 21.5 | 28.6 | 29.7 | 0.279 | 0.0 | 192 |
| rfc9110 | 194 | 48 | 0 | 30.2 | 21.8 | 29.1 | 30.2 | 0.283 | 0.0 | 192 |
| rfc9110 | 194 | none | 0 | 29.8 | 21.6 | 28.6 | 29.8 | 0.281 | 0.0 | 192 |
| spacex | 200 | 128 | 0 | 38.5 | 36.5 | 38.2 | 38.5 | 1.044 | 0.0 | 198 |
| spacex | 200 | 64 | 0 | 39.6 | 36.0 | 39.2 | 39.6 | 1.067 | 0.0 | 199 |
| spacex | 200 | 48 | 0 | 37.4 | 35.7 | 37.1 | 37.4 | 1.065 | 0.0 | 198 |
| spacex | 200 | none | 0 | 37.8 | 36.1 | 37.5 | 37.8 | 1.046 | 0.0 | 198 |
| usgs | 1 | 128 | 0 | 493.2 | 493.2 | 493.2 | 493.2 | 0.688 | 0.0 | 0 |
| usgs | 1 | 64 | 0 | 493.1 | 493.1 | 493.1 | 493.1 | 0.739 | 0.0 | 0 |
| usgs | 1 | 48 | 0 | 492.9 | 492.9 | 492.9 | 492.9 | 0.693 | 0.0 | 0 |
| usgs | 1 | none | 0 | 494.9 | 494.9 | 494.9 | 494.9 | 0.81 | 0.0 | 0 |

`gov_nasa_ntrs20240002628`, the file §8 names: **no retire 260 MiB, retire at 64 MiB gives median 39.9, p95 65.2, peak 115.8,** with 13
respawns costing 0.15 s of 3.1 s. So §8's "growth cannot be held" condition is not met.

**One page in a fresh worker,** every page of the 25 files whose whole-run peak passed 100 MiB:

pages 1547 in 25 files, statuses {'ok': 1547}; peak in a fresh worker p50 20.8, p95 73.4, max 493.1 MiB; over 64: 87, over 100: 43, over 170: 28, over 256: 18

| peak MiB | doc | page | ms |
|---:|---|---:|---:|
| 493.1 | gov_usgs_usgs70277338 | 0 | 676.587 |
| 293.7 | gov_nasa_ntrs20220005969 | 182 | 430.709 |
| 292.4 | gov_nasa_ntrs20220005969 | 174 | 440.694 |
| 281.8 | gov_nasa_ntrs20220005969 | 178 | 655.394 |
| 280.9 | gov_nasa_ntrs20220005969 | 108 | 442.972 |
| 280.0 | gov_nasa_ntrs20220005969 | 98 | 430.023 |
| 277.6 | gov_nasa_ntrs20220005969 | 78 | 433.907 |
| 275.6 | gov_nasa_ntrs20220005969 | 52 | 430.209 |
| 275.0 | gov_nasa_ntrs20220005969 | 38 | 435.248 |
| 273.6 | gov_nasa_ntrs20220005969 | 130 | 661.442 |
| 270.8 | gov_nasa_ntrs20220005969 | 104 | 662.911 |
| 268.6 | gov_nasa_ntrs20220005969 | 88 | 659.444 |

| adversarial run | status | wall s | peak MiB | cancel latency ms | pause checks | status code |
|---|---|---:|---:|---:|---:|---:|
| bombcancel:form_bomb:0 | KILLED:memory_cap | 3.19 | 2044.9 | None | None | None |
| bombcancel:form_bomb:1 | KILLED:memory_cap | 3.1 | 2040.0 | None | None | None |
| bombcancel:form_bomb:2 | KILLED:memory_cap | 3.13 | 2046.5 | None | None | None |
| bombcancel:form_repeat:0 | ok | 0.03 | 7.3 | None | 4 | 2 |
| bombcancel:form_repeat:1 | ok | 0.03 | 7.3 | None | 4 | 2 |
| bombcancel:form_repeat:2 | ok | 0.03 | 7.3 | None | 4 | 2 |

The rest of the corpus stayed under 100 MiB for its whole run. So in the full corpus of 5,689 pages, 43 pages exceed 100 MiB and 28 exceed the 170 MiB hard budget, the worst of them image or map pages. These numbers include the full-page bitmap, which matters on the USGS map (69 MiB at 2x), and a 512 px tile at 4x there still peaks at 293 MiB.

Across the 141 files without retiring, peak footprint has p50 27.5, p95 256.7 and max 545.5 MiB. 37 files exceed 60 MiB.

## 7. Cancellation

The flag is set 50 ms after `FPDF_LoadPage` begins, on a second thread; `NeedToPauseNow` reads it. 99 pages slower than 60 ms, at most 8 per file, 3 repeats. None
finished before the flag.

| measure | n | p50 ms | p95 ms | max ms | share under 100 ms |
|---|---:|---:|---:|---:|---:|
| flag set → render call returned | 297 | 25.17 | 188.13 | 550.80 | 83.8% |
| flag set → page closed | 297 | 25.90 | 188.17 | 551.37 | 83.2% |

| doc | page | latency ms | closed ms | load ms | pause checks | total ms |
|---|---:|---:|---:|---:|---:|---:|
| gov_usgs_usgs70277338 | 0 | 550.799 | 551.373 | 69.151 | 1 | 601.633 |
| gov_usgs_usgs70277338 | 0 | 548.643 | 549.2 | 68.223 | 1 | 599.458 |
| gov_usgs_usgs70277338 | 0 | 544.822 | 545.388 | 71.271 | 1 | 595.596 |
| gov_usgs_usgs70277269 | 0 | 243.695 | 243.725 | 2.377 | 1 | 293.882 |
| gov_usgs_usgs70277269 | 0 | 243.128 | 243.16 | 2.306 | 1 | 293.399 |
| gov_usgs_usgs70277269 | 0 | 242.584 | 242.615 | 2.463 | 1 | 292.841 |
| gov_usgs_usgs70277796 | 0 | 225.579 | 225.618 | 3.044 | 1 | 275.854 |
| gov_usgs_usgs70277796 | 0 | 223.868 | 223.9 | 2.954 | 1 | 274.233 |
| gov_usgs_usgs70277796 | 0 | 222.36 | 222.392 | 2.934 | 1 | 272.486 |
| gov_nasa_ntrs20220005969 | 178 | 200.088 | 200.144 | 2.237 | 2 | 250.23 |
| gov_nasa_ntrs20220005969 | 177 | 195.907 | 196.017 | 1.313 | 2 | 246.299 |
| gov_nasa_ntrs20220005969 | 178 | 194.786 | 194.842 | 2.185 | 2 | 245.173 |

- **The callback runs between objects.** One large image or shading can't be interrupted, which is why the worst pages report one or two pause checks.
- **`FPDF_LoadPage` has no pause at all.** It outlasted the flag 26 times, and took up to 115 ms.
- **`form_bomb.pdf` ignored the flag.** With a 50 ms cancel it reached the 2 GiB cap in 3.1 s in 3 of 3 runs, and only the kill stopped it. `form_repeat.pdf` finished normally.
- **hayro has no equivalent.** Only a kill stops it, so it was not measured.

## 8. Failures

**bench100, 141 files, 5,689 pages: no crash, timeout or memory cap in any run,** for PDFium (423 render runs plus text, gate and attribute passes) or hayro. No page failed to load.

**Fixtures and outside files, 193 files:**

| backend | file | status | open | failed pages | error | peak MiB | wall s | stderr tail |
|---|---|---|---|---|---|---|---|---|
| cgpdf | adversarial/deep_broad_page_tree.pdf | ok | open_failed | 0 | [None] | 0.09 | 0.031 | CoreGraphics PDF has logged an error. Set environment variable "CG_PDF_VERBOSE" to learn more.  |
| cgpdf | adversarial/deep_page_tree.pdf | ok | open_failed | 0 | [None] | 0.09 | 0.031 | CoreGraphics PDF has logged an error. Set environment variable "CG_PDF_VERBOSE" to learn more.  |
| pdfium | adversarial/form_bomb.pdf | KILLED:memory_cap |  | 0 | [] | 4101.88 | 6.269 |  |
| pdfium | encrypted/inline_encrypt_userpw.pdf | ok | open_failed | 0 | [4] | 2.329 | 0.031 |  |
| hayro | encrypted/inline_encrypt_userpw.pdf | ok | open_failed | 0 | ['Decryption(PasswordProtected)'] | 1.641 | 0.031 |  |
| pdfium | encrypted/userpw.pdf | ok | open_failed | 0 | [4] | 2.36 | 0.031 |  |
| hayro | encrypted/userpw.pdf | ok | open_failed | 0 | ['Decryption(PasswordProtected)'] | 1.625 | 0.031 |  |
| cgpdf | UnknownFilter-Linearized.pdf | ok | open_failed | 0 | [None] | 0.09 | 0.031 | CoreGraphics PDF has logged an error. Set environment variable "CG_PDF_VERBOSE" to learn more.  |

`userpw.pdf` and `inline_encrypt_userpw.pdf` are refused without a password, and both open and draw in PDFium with `--password secret` (from `gen_fixtures.py`). All other encrypted fixtures and `un_report_un_sdg2023` open without one. Not a failure.

**Adversarial set.** Pages drawn is from a render run:

| file | backend | status | pages drawn | open failed | peak MiB | wall s |
|---|---|---|---:|---|---:|---:|
| damaged_startxref.pdf | cgpdf | ok | 2 | False | 0.09 | 0.032 |
| damaged_startxref.pdf | hayro | ok | 2 | False | 11.563 | 0.031 |
| damaged_startxref.pdf | pdfium | ok | 2 | False | 13.36 | 0.026 |
| damaged_startxref_intact.pdf | cgpdf | ok | 2 | False | 0.09 | 0.031 |
| damaged_startxref_intact.pdf | hayro | ok | 2 | False | 11.61 | 0.031 |
| damaged_startxref_intact.pdf | pdfium | ok | 2 | False | 13.438 | 0.031 |
| deep_broad_page_tree.pdf | cgpdf | ok | 0 | True | 0.09 | 0.031 |
| deep_broad_page_tree.pdf | hayro | ok | 200 | False | 10.969 | 0.061 |
| deep_broad_page_tree.pdf | pdfium | ok | 200 | False | 14.048 | 0.075 |
| deep_page_tree.pdf | cgpdf | ok | 0 | True | 0.09 | 0.031 |
| deep_page_tree.pdf | hayro | ok | 1 | False | 9.438 | 0.031 |
| deep_page_tree.pdf | pdfium | ok | 1 | False | 13.079 | 0.031 |
| form_bomb.pdf | cgpdf | ok | 1 | False | 0.09 | 0.031 |
| form_bomb.pdf | hayro | ok | 1 | False | 2.375 | 0.031 |
| form_bomb.pdf | pdfium | KILLED:memory_cap | 0 | False | 4101.88 | 6.269 |
| form_repeat.pdf | cgpdf | ok | 1 | False | 0.09 | 0.022 |
| form_repeat.pdf | hayro | ok | 1 | False | 3.969 | 0.031 |
| form_repeat.pdf | pdfium | ok | 1 | False | 7.423 | 0.031 |
| stream-missing-length.pdf | cgpdf | ok | 1 | False | 0.09 | 0.031 |
| stream-missing-length.pdf | hayro | ok | 1 | False | 9.282 | 0.031 |
| stream-missing-length.pdf | pdfium | ok | 1 | False | 12.891 | 0.031 |
| stream-short-length.pdf | cgpdf | ok | 1 | False | 0.09 | 0.03 |
| stream-short-length.pdf | hayro | ok | 1 | False | 9.282 | 0.031 |
| stream-short-length.pdf | pdfium | ok | 1 | False | 12.891 | 0.031 |

**`form_bomb.pdf` passes hayro and CoreGraphics but takes PDFium to the 4 GiB cap in 6.3 s.** The harness survived, as the rule asks, but **the real worker needs a hard memory limit of its own.** hayro's open bomb reports are not exercised by this set.

**Agreement gate (§6.3), lopdf against PDFium.** 337 paths including duplicates of the named set, 330 compared across 6,768 pages:

| gate row | mismatches | files | examples |
|---|---:|---:|---|
| open: one parser refuses | 5 | 5 | deep_broad_page_tree.pdf: pdfium=ok lopdf=PageTreeDepthLimitExceeded { limit: 256 }; deep_page_tree.pdf: pdfium=ok lopdf=PageTreeDepthLimitExceeded { limit: 256 }; form_bomb.pdf: pdfium=KILLED:memory_cap lopdf=ok; UnknownFilter-Linearized.pdf: pdfium=ok lopdf=XrefDecompression(Unimplemented("decompression algorithms")) |
| annotation count | 1 | 1 | freed_compressed_link.pdf: p0 lopdf 1 pdfium 2 |
| page box | 1 | 1 | indirect_mediabox.pdf: p1 lopdf 612.0x792.0 pdfium 400.0x650.0 |
| repaired flag | 2 | 2 | InlineAbbreviations.pdf: lopdf recovered=True pdfium valid_xref=True; annotation-polyline-polygon-without-appearance.pdf: lopdf recovered=True pdfium valid_xref=True |
| hayro: page box | 2 | 1 | annots_action_handling.pdf: p0 hayro 595.276x841.89 pdfium 612.0x792.0; annots_action_handling.pdf: p1 hayro 595.276x841.89 pdfium 612.0x792.0 |

| Row | Reading |
|---|---|
| `freed_compressed_link.pdf` | **A real differential.** The second revision frees object 7, a Link. lopdf returns 1 annotation; **PDFium still returns 2.** This is the kind of difference the gate exists for |
| `deep_page_tree`, `deep_broad_page_tree` | lopdf refuses at its limit of 256; PDFium draws them. CoreGraphics also refuses |
| `UnknownFilter-*` (2) | lopdf refuses a filter on the xref stream; PDFium opens |
| `form_bomb` | Only "disagrees" because PDFium's run hit the cap. Gate calls must not load pages in a process without a limit |
| `InlineAbbreviations`, `annotation-polyline-polygon-…` | lopdf marks them recovered; PDFium reports a valid xref. The gate would disable saving on them |
| `indirect_mediabox` p1 | **Spike artefact, not a parser difference.** No MediaBox anywhere, only a CropBox; PDFium uses the CropBox, and my lopdf-side code defaulted to Letter. The gate must copy PDFium's rule for this |
| Signatures | Count and byte ranges agree on every compared file, including the four signed World Bank files |
| Revisions | **lopdf side not testable:** the fork doesn't expose the chain (§6.5). `FPDF_GetTrailerEnds` returned more than one entry on 73 files |
| After every save | Not tested: there is no writer |

hayro disagrees on page size on `annots_action_handling.pdf`, which has no MediaBox: 595 × 842 where PDFium uses 612 × 792.

## 9. Visual failures

§9.4's method over bench100, fixtures and outside files. 6,222 pages scored, 6,019 by all three backends. CoreGraphics refused 3 files (202 pages), and `form_bomb` has no PDFium frame.

| pair | pages | dil p50 | p95 | p99 | over 1% |
|---|---:|---:|---:|---:|---:|
| pdfium_cgpdf | 6019 | 0.00416 | 0.03624 | 0.05170 | 2032 |
| pdfium_hayro | 6221 | 0.01969 | 0.04268 | 0.05628 | 5026 |
| cgpdf_hayro | 6020 | 0.02232 | 0.04479 | 0.05163 | 5131 |

| Page is suspect for | Pages |
|---|---|
| PDFium | **23** |
| CoreGraphics | 138 |
| hayro | 3,081 |
| All three differ, so no majority | 1,865 |

**The 1% threshold doesn't separate failures from noise on text.** hayro draws text lighter (ink share about 0.123 against 0.146 on the same page), which alone passes 1% at max-channel 32. Every text page I opened from the hayro and three-way groups looks right to a reader. **These thresholds need retuning before they mean anything for hayro.**

**PDFium suspects reviewed: all 23 classified, 14 of them opened as images** (in `out/visual`, left to right: PDFium, CoreGraphics, hayro), the rest judged by same file and similar score. Plus 2 fixtures from the three-way group whose scores stood out.

| File | Verdict | What |
|---|---|---|
| `knockout_blend_multiply`, `knockout_inner_backdrop` | **Real** | Knockout groups aren't honoured: green instead of cyan; a black block instead of red and green borders |
| `ColorDodge`, `ColorBurn` | **Real** | Compared with `ColorDodge-correct.png`: magenta and cyan bands where the reference has none. ColorBurn compared with the two others only. Edge cases of blend formulas |
| `inline_image` p1 | **Real** | **A filtered inline image is missing**; the other two draw it |
| `PDF 2.0 image with BPC` | **Real** | CalRGB image drawn orange where the file itself says it should be red |
| `NegativeFontSize` | **Real, minor** | Text at font size set via `gs` is drawn wider than in the other two; not compared with the file's reference image |
| `IndexedCS_negative_and_high` (three-way) | **Real, edge** | Out-of-range index colours shift along a row |
| `OverlappingGlyphClipping` (three-way) | **Possibly real** | Clip differs slightly from both; counted against PDFium |
| `SelfIntersecting-Transparency` | **PDFium right** | The file's README reference shows a single-colour stroke. PDFium matches; the "majority" is wrong |
| `annotation-square-circle-without-appearance`, `annotation_markup_multiline_no_ap` | **PDFium ahead** | It generates appearances for annotations lacking `/AP`. The others draw nothing |
| `gov_usgs_usgs70275236` p18, 33, 34, 35 | Noise | Map cells resampled differently. Opened p34; others by same file and similar score |
| `policy_worldbank_wbD34442285` (6 pages) | Noise | Table text weight. Opened p12; others by same file and similar score |
| `q-bio_NC_2607.26666` p39, `gov_nasa_ntrs20240002628` p10 | Noise | Opened; indistinguishable to a reader |

**Real failures: 9 of 6,019 pages, 0.15%, all on synthetic test files. None on bench100 pages among the suspects.**

**Limits of that number.** Majority voting can't flag anything without a majority: the 1,865 three-way pages are checked only by sampling (4 opened, including the 2 above). A failure shared with CoreGraphics or hayro is invisible. Text-heavy noise can hide a small real defect. CoreGraphics left out annotations on the pages I looked at, so it is a weak vote on those.

Other reviews: `InlineAbbreviations` is garbled by CoreGraphics, not PDFium. On hayro I opened 4 suspects: 3 text-weight only, and `gov_nasa_ntrs20240002628` p86 draws a hatch noticeably heavier. **hayro's real-failure rate is not established.** Also opened: five bench100 pages for the filter in §10, and overprint-tier pages showed no PDFium suspects except the USGS resampling above.

## 10. PDFium text

### 10.1 Speed and memory

`FPDF_LoadPage` + `FPDFText_LoadPage` + `FPDFText_GetText`, no bitmap, first 200 pages, 5 repeats. Char boxes means `FPDFText_GetCharBox` for every character.

| doc | pages | chars | text MiB | ms/page p50 (load+text+get) | p95 | max | text-page build p50 | GetText p50 | all char boxes p50 | peak fp MiB | doc total ms | pdftotext wall ms | pdftotext peak MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| rfc9110 | 194 | 469,903 | 0.45 | 0.73 | 0.89 | 1.15 | 0.40 | 0.00 | 0.01 | 8.4 | 134.2 | 313.0 | 17.9 |
| spacex | 200 | 928,053 | 0.89 | 3.40 | 5.04 | 5.65 | 2.00 | 0.01 | 0.01 | 25.2 | 621.5 | 1,005.0 | 17.4 |
| nasa | 200 | 365,448 | 0.35 | 1.05 | 11.77 | 32.33 | 0.30 | 0.00 | 0.00 | 133.5 | 564.3 | 1,980.0 | 21.5 |
| moderna | 200 | 515,853 | 0.49 | 1.81 | 3.79 | 4.36 | 1.12 | 0.01 | 0.01 | 13.6 | 347.7 | 401.0 | 8.1 |
| usgs | 1 | 9,913 | 0.01 | 71.67 | 71.67 | 71.67 | 1.57 | 0.02 | 0.03 | 185.7 | 71.7 | 167.0 | 5.9 |
| attention | 15 | 40,407 | 0.04 | 0.94 | 10.54 | 12.64 | 0.42 | 0.01 | 0.01 | 19.6 | 47.8 | 92.0 | 6.3 |
| aiact | 144 | 606,001 | 0.58 | 1.11 | 1.69 | 2.69 | 0.77 | 0.01 | 0.01 | 9.7 | 168.3 | 182.0 | 10.0 |
| sdg2024 | 51 | 239,135 | 0.23 | 2.30 | 11.95 | 50.83 | 0.70 | 0.01 | 0.01 | 58.9 | 213.7 | 395.0 | 10.1 |

Text costs less than drawing: p50 0.83 ms per page over all of bench100. On the two files fully covered by both tools PDFium took 134 ms against 313 ms (`rfc9110`) and 168 ms against 182 ms (`aiact`). Elsewhere `pdftotext` read the whole file and PDFium only 200 pages, so those two columns aren't comparable, and its figures come from the 20 ms poll and include process start and writing.

**PDFium uses more memory than `pdftotext` on files with images,** because `FPDF_LoadPage` builds the page objects: 134 MiB against 21 MiB on NASA, and 186 MiB for 9,913 characters on the USGS map. A long-lived text worker needs the same retire logic.

**Highlight positions cost almost nothing once the text page exists:** `FPDFText_GetCharBox` for every character on a page has a p50 of 0.03 ms or less. Building the text page is the cost, 0.3 to 2.0 ms here.

Text returned, measured against `pdftotext` as an independent reference, word multiset over all 141 files: **98.17% raw, 98.92% after the search normaliser below.** Worst file after normalising is 89.86% (`math_PR_2607.28413`, heavy mathematics). The reference isn't ground truth.

### 10.2 Check 1: reading order on two-column pages

Sampled page indices 1 to 5, 8 and 12 of every bench100 file, selected by a central gutter in char positions, with at least 800 characters: 84 pages in 31 documents.

| Result | Pages |
|---|---|
| Body text one column then the other | **79** |
| Left, right, left, right: fits two sections in a page (`un_report_un_sdg2024` p8 opened and correct; 2 others not opened) | 3 |
| No long run on either side | 1 |
| Figure text jumping between columns, body still in order | 1 (`cs_SE_2607.28587` p2) |

**No page alternated between columns in runs of body text** (runs of at least 300 characters; shorter jumps exist in headers and captions). Example, `eess_SP_2607.28224` p1, end of left column into the right column, matching the page as drawn:

```
as sensing metrics in BF and PA for cooperative ISAC networks.\r\n
Related works [11], [26], [27], [28], [29], [31], [32], [33], [34]\r\n
```

This follows the PDF's content order, so it can't be guaranteed for PDFs written in another order. Running headers come first, as `L 58, R 65, L 3217, R 3056` shows. Tables and sidebars were not checked.

### 10.3 Check 2: ligatures, hyphenation, NBSP

| What a person sees | What PDFium returns | Does `rg` find it? | Fix |
|---|---|---|---|
| The `fi` ligature in "field" (`astro-ph_GA_2607.28278` p0) | `field`. **No U+FB00–FB06 in 16.4 M chars;** poppler returns U+FB01 2,494 times | **Yes.** `rg -i field`: PDFium 4,087 lines, `pdftotext` 3,712 | None needed |
| `valu-` / `able` across a line | **`valu￾able`:** the line break is gone and U+FFFE takes the place of the hyphen. 13,924 times in 128 files | **No.** `valuable` misses it | Remove U+FFFE |
| `simulation-` / `based` across a line | `simulation￾based`, same marker | **No,** and removing the marker gives `simulationbased`: 4 such hits, plus 2 `datadriven` | Index both forms |
| `a poppet` with a real NBSP (`gov_nasa_ntrs20210011071` p21) | `a poppet`. 3,039 in 26 files, none in the S-1 or Moderna | **No,** for plain space | NBSP to space |
| Words across a line | `\r\n` between them. 334,468 CRLF, almost no bare LF | Needs `rg -U`: `United\s+States` 122 against 106 for the plain phrase | Unwrap lines, or use `-U` |
| A printed hyphen that PDFium keeps (`21-` / `cm`) | `21-\r\ncm`. 185 cases | No, without handling | Same unwrap |
| `≳ 2 M_Jup` in math (`astro-ph_GA_2607.28278` p0) | `¦2 Jup`: the file's mapping gives `¦` (poppler too), and **PDFium drops the 𝑀 that poppler returns** | `MJup` misses it | None found |

**Real `rg` over the 5,689 page dumps,** line hits, case-insensitive:

| Query | PDFium raw | After normaliser | `pdftotext` |
|---|---:|---:|---:|
| `available` | 1,120 | 1,135 | 1,135 |
| `information` | 3,163 | 3,202 | 3,196 |
| `different` | 1,365 | 1,397 | 1,286 |
| `significant` | 854 | 879 | 826 |
| `first` | 2,027 | 2,027 | 1,907 |
| `efficient` | 857 | 873 | 773 |
| `field` | 4,087 | 4,087 | 3,712 |
| `development` | 1,540 | 1,599 | 1,599 |
| `office` | 634 | 634 | 633 |
| `simulation-based` | 10 | 10 | 10 |

The normaliser tested removes U+FFFE, U+00AD and zero-width characters, turns NBSP and thin spaces into a space, and CRLF into LF. **It's needed: raw text misses up to 4% of hits on common words here (`development`), and none on others (`first`).** U+FFFE is not in the agent report's list (§6.3 there), which expects ligatures instead. `FPDFText_IsHyphen` marks these characters, so it can be told apart while building the corpus.

Control characters and private-use code points also appear in mathematics: 6,417 and 1,645 in dumps, in 53 and 42 files. U+FFFD 111 times in 6 files. Strip them.

### 10.4 Check 3: hidden text

**PDFium returns it.** Fixture `hidden_text.pdf`, generated by `gen_hidden.py`, opened as an image to confirm none of the `HIDDEN-` lines show:

| Sentinel | PDFium text | `pdftotext` | mode | size | fill RGBA | ink |
|---|---|---|---:|---:|---|---:|
| `VISIBLE-SENTINEL` | yes | yes | 0 | 12.000 | [0, 0, 0, 255] | 255 |
| `VISIBLE-RED` | yes | yes | 0 | 12.000 | [204, 0, 0, 255] | 255 |
| `HIDDEN-WHITE` | yes | yes | 0 | 12.000 | [255, 255, 255, 255] | 0 |
| `HIDDEN-MODE3` | yes | yes | 3 | 12.000 | [128, 0, 0, 255] | 0 |
| `HIDDEN-ZEROSIZE` | no | yes |  |  |  |  |
| `HIDDEN-TINY` | yes | yes | 0 | 0.010 | [128, 0, 0, 255] | 0 |
| `HIDDEN-TMSCALE` | yes | yes | 0 | 12.000 | [128, 0, 0, 255] | 0 |
| `HIDDEN-OFFPAGE-RIGHT` | yes | no | 0 | 12.000 | [128, 0, 0, 255] | -255 |
| `HIDDEN-OFFPAGE-BELOW` | yes | no | 0 | 12.000 | [128, 0, 0, 255] | -255 |
| `HIDDEN-CLIPPED` | yes | yes | 0 | 12.000 | [128, 0, 0, 255] | 0 |
| `HIDDEN-BEHINDRECT` | yes | yes | 0 | 12.000 | [128, 0, 0, 255] | 0 |
| `HIDDEN-OCGOFF` | yes | no | 0 | 12.000 | [128, 0, 0, 255] | 0 |
| `HIDDEN-ALPHA0` | yes | yes | 0 | 12.000 | [128, 0, 0, 0] | 0 |
| `HIDDEN-MODE7` | yes | yes | 7 | 12.000 | [128, 0, 0, 255] | 0 |
| `HIDDEN-SAMEASBG` | yes | yes | 0 | 12.000 | [230, 230, 230, 255] | 0 |
| `HIDDEN-FORMOCG` | yes | no | 0 | 12.000 | [128, 0, 0, 255] | 0 |
| `HIDDEN-ANNOTCONTENTS` | no | yes |  |  |  |  |
| `HIDDEN-ANNOTAP` | no | no |  |  |  |  |
| `VISIBLE-AFTER` | yes | yes | 0 | 12.000 | [128, 0, 0, 255] | 255 |
| `VISIBLE-PAGE2` | yes | yes | 0 | 12.000 | [0, 0, 0, 255] | 255 |
| `HIDDEN-OUTSIDECROP` | yes | yes | 0 | 12.000 | [0, 0, 0, 255] | -255 |

The `ink` column is the max-min channel range across the char's box in a 2x render; 0 means nothing drawn there, and -255 means the box is off the bitmap. `mode`, size and fill come from `FPDFTextObj_GetTextRenderMode`, `FPDFText_GetFontSize` and `FPDFText_GetFillColor`.

- **PDFium returned 14 of 17 hidden techniques.** Not returned: zero font size, annotation `/Contents`, hidden annotation appearance.
- **`HIDDEN-ANNOTCONTENTS` is misnamed: it is visible.** PDFium draws a generated box with the text, but the text API skips it. **So annotation text the user can see is not in page text,** and needs `FPDFAnnot_GetStringValue` separately.
- `pdftotext` drops off-page and hidden-layer text but returns the rest. It is not a safe reference either.

**The attributes alone aren't enough.** Mode, size, alpha and position catch some. Clip, covered-by-rectangle, hidden layer and same-colour-as-background leave no trace in them.

**Prototype visibility filter:** after the page render, keep a character only if its box has ink (range ≥ 16). Fixture: **all 14 removed, all visible lines kept.** Corpus:

| Measure | No padding | Box padded by 2 px |
|---|---:|---:|
| Chars flagged, of about 13.4 M non-space | 23,975 (0.18%) | 19,731 (0.15%) |
| Pages with a flagged char, of 5,689 | 248 | 152 |
| Flagged runs of at least 6 letters (first 6 runs per page recorded) | 154 | 156 |
| Cost after the render, p50 / p95 | 0.59 / 1.05 ms | 0.98 / 1.82 ms |

**Real documents contain hidden text.** Checked at character level or by opening the page:

| File | Text | Why it's invisible |
|---|---|---|
| `policy_worldbank_wbD34466164` p0 | `For Official Use Only` × 5 | Off-page at x = −110 |
| Same | `Public Disclosure Authorized` × 2 of 4 | White on white; the other 2 are visible |
| `gov_usgs_usgs70277269` p17 | `DELETED` | Off-page at x = 632 on a 612-wide page |
| `gov_nasa_ntrs20180005693` p0 | Agency name and address | Not on the cover as drawn |
| `cs_SE_2607.28587` p2 | Figure text repeated | Column pattern `806, 636` repeats three times; flagged in part |

Attribute census over 16.6 M characters: render mode 3 on 0, font under 1 pt on 1,202 (1 file), white fill on 37,333 (57 files, many legitimately on dark boxes), off-page on 16,508 (24 files). **This corpus has no OCR layer, so `Tr 3` over a scan is untested.** An image under the box should count as ink and keep it, but that needs a real scan.

The filter is a prototype. Single-character false positives remain, such as thin `–`, `·`, `.`, and I didn't review the remaining flags systematically. Hidden text next to visible text would pass, because the box then has ink. **The sibling report's T6 assumed distillPDF flags hidden text; with distillPDF parked that needs a new home.** A filter also means text extraction needs a render, so this cost belongs in the indexing budget.

## 11. §9.6 rule, line by line

| # | Rule | Measured | Result |
|---|---|---|---|
| 1 | No crash in bench100 outside the adversarial set | 0 crashes, 0 timeouts, 0 cap kills in 423 PDFium render runs over 5,689 pages | **PASS** |
| 1b | Adversarial files may kill the worker, not the harness | `form_bomb` killed at the 4 GiB cap in 6.3 s; harness continued | **PASS** |
| 2a | Idle baseline leaves room for the app budget | 2.27 MiB idle, 2.2 MiB floor | **PASS** |
| 2b | Curve can be held flat by retiring | `gov_nasa_ntrs20240002628`: 260 MiB peak becomes median 39.9, p95 65.2, peak 115.8 at 64 MiB; respawn 13–27 ms | **PASS** |
| 2c | Footprint leaves room for the app budget | Text: yes. 28 of 5,689 pages pass the 170 MiB hard budget in a fresh worker, max 493 MiB; retiring can't fix one page | **FAIL** on those pages |
| 3 | p95 at 2x under 150 ms on text and vector pages | **8.64 ms**, 4,401 pages; max 161.5 ms | **PASS** |
| 4 | Cancel returns under 100 ms on 95% of slow pages, kill covers the rest | **83.8%**, p95 188 ms, max 551 ms; kill needed 0 of 297, except `form_bomb` 3 of 3 | **FAIL** |
| 5 | Real visual failures on ≤ 1% of pages, reviewed | 9 of 6,019, **0.15%**, all synthetic; see limits in §9 | **PASS,** partial review |

**The rule says "confirmed if all hold". Two lines don't, so PDFium is not confirmed by the rule as written.** §8 says what would make us switch; none of its conditions is met: hayro has no cancellation and its bombs are not closed, and growth is held on the named file.

## 12. Recommendation on RD1

**Stay with option A, PDFium non-V8 in a worker. Amend §9.6 rather than switch.** Your call, because it changes a rule we set in advance.

| Change to the rule | Why |
|---|---|
| **Line 4 becomes: a stale render is gone within 100 ms, by return or kill.** The worker draws on with the pause callback, and the host kills at a deadline | Kill is 0.85 ms and the replacement draws in 13 to 27 ms. A kill at a deadline just under 100 ms meets this for every page measured, at the cost of a respawn |
| **Line 2 keeps the curve, adds a per-page limit.** Worker hard limit at about 512 MiB, then a per-page "too large at this zoom" state or lower resolution | No renderer here holds one such page under 170 MiB: USGS map 493 MiB in PDFium, 258 MiB in hayro |
| **Budget 170 MiB hard needs an exception** for the 0.5% of pages above it, or a patched image path | Platform §4 |

**Why not hayro.**

| | PDFium | hayro |
|---|---|---|
| p95 text and vector | 8.64 ms | **4.64 ms** |
| p95 all pages | 30.39 ms | **24.55 ms** |
| Largest peak, 141 files | **545 MiB** | 846 MiB |
| `gov_usgs_usgs70278165` peak | **113 MiB** | 843 MiB |
| USGS map peak | 493 MiB | **258 MiB** |
| S-1 peak after 200 pages | **37.6 MiB** | 70.2 MiB |
| Throughput with 4 workers | 2.66–2.97× (processes) | 2.22–2.49× (threads) |
| Cancel | Pause callback, then kill | Kill only |
| `form_bomb` | 4 GiB, killed | **Fine** |
| Text API tested | Yes | Not evaluated |
| Real visual failures | 0.15%, known | Unknown; review not done |

**Speed no longer argues for PDFium; fidelity evidence, memory on image pages, and text do.** Keep hayro as the second opinion and rerun this when it gains cancellation.

**Conditions to carry forward:**

1. Worker hard memory limit, enforced from outside. `form_bomb` needs it.
2. Retire by footprint at about 64 MiB, and by kill after any overrun. Closing the document isn't enough.
3. Text for agents passes a visibility filter and a normaliser including U+FFFE. Annotation text is collected separately.
4. Treat knockout groups, `inline_image` and CalRGB images as known gaps, and check the inline image one upstream.
5. The gate compares `/Annots` after freed objects (`freed_compressed_link`).

## 13. What this changes in other reports

| Report | Statement | Now |
|---|---|---|
| `SIGNOFF.md`, table and Risk 2 | "PDFium's own memory and speed unmeasured" | Measured here |
| `SIGNOFF.md`, distillPDF row, Not proven | PDFium text quality and hidden text untested | Tested: it returns hidden text; quality in §10 |
| `rendering-interactive` §9.6 | Rule proposal | Lines 2c and 4 fail; thresholds of §9.4 too noisy on text |
| `rendering-interactive` §11, Not verified | "PDFium memory growth in practice" | Measured: growth by new pages, close doesn't return it, exit does |
| `rendering-interactive` §6.3 rule 5 | "Text for agents may use the lopdf path" | Replaced by PDFium, with `freed_compressed_link` as a reason to report mismatches |
| `rendering-interactive` §1, RD1 | hayro's cost list | Add: as fast or faster here. Keep the rest |
| `platform-architecture` §4 | "≤ 15 renderer heap and fonts" (inferred) | Holds on ordinary text early in a file; S-1 18 MiB at first tile. Retire limit should be below 64 MiB for a text-page budget |
| `platform-architecture` §4 | Fast scrolling ≤ 170 MiB hard | Not reachable on 28 corpus pages |
| `platform-architecture` §4 | Renderer crash ≤ 100 ms | Supported: 13–27 ms without XPC |
| `agent-integration` §6.3 | Ligature expansion | Not needed from PDFium; U+FFFE and CRLF needed instead |
| `agent-integration` T6 | distillPDF flags hidden text | No longer available; §10.4 prototype is the candidate |
| `engine-audit` §4.1 | lopdf figures | Reproduced within 7% |

I changed none of those files.

## 14. Confirmed and Not tested

### Confirmed by running it

| Claim | Evidence |
|---|---|
| Digest and attestation of the PDFium tarball match the spec | §2 |
| PDFium draws text and vector pages at p95 8.64 ms at 2x | §4 |
| Idle worker 2.27 MiB; `footprint` agrees with `proc_pid_rusage` | §3, §6 |
| Open of the 330 MiB file 0.94 ms, 3.81 MiB | §5 |
| Footprint stays after closing the document and returns only on exit | §6 |
| Retiring holds growth on the §8-named file | §6 |
| A single page can exceed the hard budget in a fresh worker | §6 |
| Cancel misses 100 ms on 16% of slow pages; a kill takes under 1.3 ms | §7, §6 |
| No crash on bench100; `form_bomb` reaches the cap in PDFium | §8 |
| `freed_compressed_link`: PDFium keeps a freed annotation | §8 |
| PDFium returns 14 of 17 hidden-text techniques, and corpus files contain hidden text | §10.4 |
| Hyphen marker U+FFFE, NBSP and CRLF defeat plain `rg`; ligatures don't | §10.3 |
| hayro speed and footprint on the same pages | §4, §12 |

### Not tested

| Item | Why | Who or what would settle it |
|---|---|---|
| **Inside a sandboxed XPC service,** including AppKit linkage and font access | Spike runs plain processes | Platform spike |
| Cold-cache timings | Needs a purge that disturbs the other agent | Later run alone |
| Files above 346 MB or the 2 GB budget file | Not in the corpus | Generated file |
| PDFium on a real display, IOSurface tiles, zoom | Off-screen spike | App milestone |
| Tile cost across a full page, as opposed to one centre tile | Time-box | Scroll simulation |
| Overnight or 24 h footprint soak | Time-box | Soak rig |
| Windows | Mac only | PC |
| V8, XFA, dynamic XFA | Out of scope (RD9 B). The two `xfa_*.pdf.link` files were not fetched | Spike D |
| PDF-Attacker shadow samples | Not fetched; no signing verification here | Signing work |
| Form field values, typing, `FPDF_FFLDraw` beyond drawing | No form work here. IRS hybrids drew and opened; XFA and UR3 behaviour not checked | Form spike |
| Revision chain on the lopdf side; gate after save | Fork exposes neither | Fork work |
| hayro real-failure rate; 100-page suspect review per renderer | Threshold too noisy; time-box. PDFium classified 23 of 23, opened 14; three-way 4 of 1,865; hayro 4 of 3,081 | Tuned threshold, then review |
| Text in tables, RTL scripts, vertical text; CJK beyond file census | Not sampled | Text spike |
| Filter on scanned PDFs with an OCR layer | None in the corpus | Scans |
| False-positive rate of the filter, systematically | Only samples read | Labelled set |
| Real agent run with injected text | Out of scope | Agent canary test 14 |
| Tiles with `FPDF_FFLDraw` on rotated pages at 4x | Not separately | Tile test |

## 15. Commands and raw data

**Code:** `/private/tmp/claude-501/-Volumes-EksternalHome-Koding-Rust-PDF-reader/0b5d3263-1607-4095-a9bd-b761e0233857/scratchpad/renderer/code`. **Raw logs:** `…/scratchpad/renderer/logs` under the same prefix, JSON lines per batch, with `SHA256SUMS.txt`, `attestation.json`, `Cargo.lock`, `outside_files.sha256`, `wordrecall.txt`. Page images: `…/renderer/out/visual`, `…/renderer/out/samples`. Text dumps: `…/renderer/out/text`. Scratch is temporary, so copy anything you want to keep. The build folder was deleted (R11).

Files are read from `distillPDF/tests/fixtures_pdf`, `benchmarking/bench100`, and `benchmarking/secondopinion/corpus` (S-1, Moderna), `benchmarking/corpus_tests/corpus` (attention). The S-1 is not in bench100.

```sh
H=/Volumes/EksternalHome/Koding/Rust/SeedWell/tools/sysguard/heavy.sh
R=<scratch>/renderer; C=$R/code

# Download and verify
curl -sSfL -o $R/dl-pdfium-8125/pdfium-mac-arm64.tgz \
  https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/8125/pdfium-mac-arm64.tgz
shasum -a 256 -c <<< "4939bd93…139d  $R/dl-pdfium-8125/pdfium-mac-arm64.tgz"
gh attestation verify $R/dl-pdfium-8125/pdfium-mac-arm64.tgz --repo bblanchon/pdfium-binaries --format json
tar -tzf …; tar -xzf … -C $R/pdfium-8125-unpacked

# Build
git -C lopdf-lazy-engine archive 909771af… | tar -x -C $C/lopdf-src
$H env CARGO_TARGET_DIR=$R/target cargo build --release --bins
$H swiftc -O -o $R/target/release/spike-cgpdf $C/spike-cgpdf.swift

# Each batch: python3 -I harness.py ROOT BATCH [args]
for b in idle "open synth/s20k.pdf synth/s100k.pdf" timed classify curve text attrs ink inkpad \
         "gate @outside_files.txt" "survival @outside_files.txt" tile cycle parallel hayro_timed tiers \
         curvetile retire footprint "cancel out/cancel_spec.json" "freshpeak out/fresh_docs.json" \
         "visual --extra @outside_files.txt"; do
  $H /usr/bin/python3 -I $C/harness.py $R $b
done

# Single-purpose scripts
$H /usr/bin/python3 -I $C/order.py $R $R/logs/order.jsonl
$H /usr/bin/python3 -I $C/wordrecall.py $R
/usr/bin/python3 -I $C/gen_hidden.py $R/fixtures/hidden_text.pdf
/usr/bin/python3 -I $C/analyze.py $R timed curve cancel …   # prints the tables here

# What a run looks like
spike-pdfium LIB render PDF --max 200 [--raw DIR] [--reopen-every N] [--loops N]
spike-pdfium LIB open|text|gate|chars|attrs|ink|cancel|tile|classify|worker …
```

**Two harness bugs were found and rerun, so logs are from the corrected versions:** arguments split wrongly in one batch script, so nothing ran; and the worker handshake read one line short, which would have shifted the cycle timings.

**Started about 21:27 local; about an hour of wall time,** including queueing for the shared lock.
