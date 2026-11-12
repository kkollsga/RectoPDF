# recto-headings

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-headings` | **Started** | A helper process in a workspace of its own: finds one document's headings with distillPDF, keeps those it can see through a render worker's check, says them on stdout, and exits. Generated contents' format and folder are `recto_index::generated`, the verdict on a file's own `recto_core::verdict` | Write anything; be built with the main workspace, which would give the app distillPDF's lopdf features |

## Dependencies

| Crate | Why |
|---|---|
| `distillpdf` (git, `fbf5b22f`, 0.1.7, no features) | Headings for generated contents, in `recto-headings` only. A workspace of its own, because it asks for lopdf's default features: `rayon`, which parses object streams on a thread pool, and `chrono-clock`. Cargo unifies features across what's built together, so in the main workspace the app and session would have had that pool. It brings 33 crates the main build doesn't have: `image`, `png`, `zune-jpeg`, `jpeg-decoder`, `fax`, `rayon` and `crossbeam-*`, `chrono` and `iana-time-zone`, `sha2` 0.10 and `digest`, `moxcms`, `base64` and their small dependencies. The helper is 4.00 MB on a release build, beside `recto-indexer`'s 1.44; the app's own binary grew by 0.16 MB and `recto` by 0.12 MB with this work, none of it distillPDF. Its own `Cargo.lock` has to be kept in step with the main one's pins. **No image is decoded there** (owner, 2026-11-08: "Turn its image decoding off"): the page-at-a-time path the helper calls places images without reading their samples, which `tests/images.rs` holds against a decompression bomb, so the decoders it links never run |
