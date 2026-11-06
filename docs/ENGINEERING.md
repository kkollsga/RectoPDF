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
| `recto-index` | **Started** | Text normaliser and hidden-text report: pure functions on the host's side. Later the page-text corpus | Touch the UI |
| `recto-svg` | Planned | Streaming SVG writer | Build a document in memory |
| `recto-sign` | **Started (placeholder)** | Final seams for PAdES creation and the honest report; `PlaceholderSigner` always refuses | Depend on `recto-render`; produce anything that looks like a signature |
| `recto-session` | **Started** | Tabs, tiers, sidebar and view models: all the rules | Name a UI toolkit |
| `recto-agent` | **Started** | Typed, versioned commands, permission levels, the JSON envelope | Hold state of its own |
| `recto-ffi` | Planned | UniFFI, pinned to what the C# generator supports (0.31 in Seedwell); every call async or bounded | Hold rules |
| `recto-cli` | **Started** | `recto`: thin client of `recto-agent`, with a headless session for tests | Link AppKit |
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

## Dependencies

| Crate | Why |
|---|---|
| `lopdf` (fork) | The lazy `IndexedReader`, and later the append writer. Pinned to a git revision |
| `serde`, `serde_json` | The agent JSON envelope |
| `libc` | `proc_pid_rusage` for footprint, and process, channel and shared-memory calls, `recto-platform` only |
| `flate2` | Compresses `render`'s PNG. Already in the tree through lopdf, so it adds no crate. `recto-agent` only |
| PDFium (not a crate) | Draws pages. `bblanchon/pdfium-binaries` release `chromium/8125`, PDFium 158.0.8125.0, no V8, no XFA. Loaded at run time, by the worker alone. `tools/pdfium.sh` fetches it from that release's URL to `target/pdfium/` and stops unless the tarball is SHA-256 `4939bd93…139d` and the library `a45c4464…373a`; `--attest` also runs `gh attestation verify`. It links AppKit; see the sandbox note below |

**No `pdfium-render`.** The spike used it only for raw bindings, because its high-level loaders can't take a `pread` callback and `PdfDocument::handle()` is crate-private. 27 signatures in one file cost nothing; the crate cost 24 dependencies and 7 more MiB of binary to run a bigger worker.

## Renderer

**The process.** One worker per document, in a plain child process on a socket pair, with shared memory for pixels and one shared word for cancelling. It inherits nothing but its channel (`POSIX_SPAWN_CLOEXEC_DEFAULT`) and an empty environment, gets the document as a descriptor, and never learns a path. No thread in the host: the call blocks its caller, which is never the UI thread, and watches while a job is in flight. A cold tab has no `DocRenderer`, so it pays for none of this.

**Limits**, all held from outside the worker, because it may be compromised:

| What | Default | Why |
|---|---|---|
| Hard memory limit | 512 MiB footprint, read every 5 ms during a job | The worst real page reaches 493 MiB. macOS has no OS limit for this: `setrlimit` refuses `RLIMIT_AS`, `RLIMIT_DATA` and `RLIMIT_RSS` with `EINVAL` |
| Page deadline, with kill | 5 s | Platform §2.6. The slowest real page at 2x took 939 ms |
| Retirement | 64 MiB after any job | Only exit returns PDFium's memory (spike §6) |
| Cancel by generation | Kill 90 ms after a job goes stale, if it hasn't returned | A stale render is gone within 100 ms, by return or kill. PDFium's pause callback runs only between objects |
| Idle exit | 30 s after the last job | |

A page that stops a worker with history gets one more try in a fresh one, because what stopped it may be what earlier pages left behind. A page that stops a fresh worker is remembered and refused until `forget_failures`. A worker the host doesn't watch would be free to grow, so during a job the worker also runs a small thread of its own that checks its footprint, its deadline and whether the host is still there. That thread starts with the first job and parks between jobs.

**The renderer is hostile.** Replies go through `recto-proto`'s decoders and validators before anything uses them; any violation kills the worker. A frame's claimed length costs nothing until bytes arrive. Pixels are copied out of shared memory, never referenced in place, and shared memory can't be resized once sized on macOS, so a worker can't make the host fault. A compromised worker can still draw a wrong picture and rewrite a surface it was given earlier; that's the limit of this design until surfaces are per-job IOSurfaces.

**Sandbox: not in place yet.** The worker has `RLIMIT_CORE`, `RLIMIT_FSIZE` and `RLIMIT_NPROC` at zero and nothing else, so a compromised worker can read what the user can. The XPC sandbox is a separate spike, whose known problem is that `libpdfium.dylib` links AppKit. Everything crosses as frames and handles so XPC can replace the transport: the worker takes no path to a document, and argv names only limits and the library.

**Search text.** `recto-index::normalize` turns PDFium's characters into text `rg` can search. Its rules and their limits are in that crate's header. In short: at a line-end hyphen it follows the rest of the page if the page shows a form, keeps the hyphen for capitals, one- or two-letter tails and a short closed list of compound words, and otherwise joins. It's a guess, so the other spelling of each decision goes in `also_spelled`. Lines stay lines.

**Hidden text.** The spike's prototype: draw the page at 2x, mark a character with no ink under its box (range under 16, box padded by 2 px). `hidden_text.flagged` means a run of 3 or more such characters; shorter ones are thin glyphs, and stay in the count. It's a flag: every character stays in the text, under `untrusted`. Its limits: hidden text next to visible text passes, because the box has ink. An OCR layer over a scan is expected to pass, but this corpus has no scan to prove it on. Annotation text the page draws isn't in page text at all (spike §10.4). If the extra render stops the worker, the text comes back with `checked: false`, which never reads as clean.

**Fetching PDFium.** Run `tools/pdfium.sh`, which `tools/check.sh` also runs. The worker finds the library beside itself or in `../pdfium/`; `RECTO_PDFIUM_LIB` overrides. The worker binary is `recto-render-service`, found beside `recto`; `RECTO_RENDER_SERVICE` overrides. `cargo build -p recto-cli` alone doesn't build it: use `--workspace`.

## Machine notes

Lints and profiles are set once in the root `Cargo.toml`. `tools/check.sh` is the one local check, and takes the shared heavy-job lock.
