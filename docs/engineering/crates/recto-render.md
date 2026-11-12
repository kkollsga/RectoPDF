# recto-render

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-render` | **Started** | `render_tile(page, rect, scale, &mut [u8], cancel)` behind `RenderBackend`, page text, and a page's contents a frame at a time. PDFium, by raw FFI | Allocate surfaces, name an OS, decide policy |

## Dependencies

| Crate | Why |
|---|---|
| PDFium (not a crate) | Draws pages. `bblanchon/pdfium-binaries` release `chromium/8125`, PDFium 158.0.8125.0, no V8, no XFA. Loaded at run time, by the worker alone. `tools/pdfium.sh` fetches it from that release's URL to `target/pdfium/` and stops unless the tarball is SHA-256 `4939bd93…139d` and the library `a45c4464…373a`; `--attest` also runs `gh attestation verify`. It links AppKit; see the sandbox note below |

**No `pdfium-render`.** The spike used it only for raw bindings, because its high-level loaders can't take a `pread` callback and `PdfDocument::handle()` is crate-private. 27 signatures in one file cost no crate; `pdfium-render` costs 24 in the tree. The worker is 0.48 MB here against the spike's 1.3 MB, which also carried JSON.

**A missing renderer** names itself: the error says `recto-render-service` or `libpdfium.dylib`, and `next` says how to get it.

**Fetching PDFium.** Run `tools/pdfium.sh`, which `tools/check.sh` also runs. The worker finds the library beside itself or in `../pdfium/`. The worker binary is `recto-render-service`, found in the app's helper or beside `recto`. `RECTO_PDFIUM_LIB` and `RECTO_RENDER_SERVICE` override, in debug builds only: nothing shipped loads code from a path in its environment. The digest is checked when the script fetches, not at each load; code signing covers that in the app. `cargo build -p recto-cli` alone doesn't build it: use `--workspace`.
