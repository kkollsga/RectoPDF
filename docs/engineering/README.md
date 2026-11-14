# Engineering

How the code is laid out. One file per crate under [`crates/`](crates/) and one per subsystem that crosses crates, so two branches on different crates or features never edit the same file.

## Crate graph

A crate may depend only on crates below it in this list. `recto-platform` sits under everything and is the only crate that names an OS.

| Crate | File |
|---|---|
| `recto-platform` | [`crates/recto-platform.md`](crates/recto-platform.md) |
| `recto-update` | [`crates/recto-update.md`](crates/recto-update.md) |
| `recto-core` | [`crates/recto-core.md`](crates/recto-core.md) |
| `recto-proto` | [`crates/recto-proto.md`](crates/recto-proto.md) |
| `recto-render` | [`crates/recto-render.md`](crates/recto-render.md) |
| `recto-render-service` | [`crates/recto-render-service.md`](crates/recto-render-service.md) |
| `recto-index` | [`crates/recto-index.md`](crates/recto-index.md) |
| `recto-indexer` | [`crates/recto-indexer.md`](crates/recto-indexer.md) |
| `recto-headings` | [`crates/recto-headings.md`](crates/recto-headings.md) |
| `recto-svg` | [`crates/recto-svg.md`](crates/recto-svg.md) |
| `recto-reader` | [`crates/recto-reader.md`](crates/recto-reader.md) |
| `recto-sign` | [`crates/recto-sign.md`](crates/recto-sign.md) |
| `recto-session` | [`crates/recto-session.md`](crates/recto-session.md) |
| `recto-app` | [`crates/recto-app.md`](crates/recto-app.md) |
| `recto-agent` | [`crates/recto-agent.md`](crates/recto-agent.md) |
| `recto-control` | [`crates/recto-control.md`](crates/recto-control.md) |
| `recto-ffi` | [`crates/recto-ffi.md`](crates/recto-ffi.md) |
| `recto-cli` | [`crates/recto-cli.md`](crates/recto-cli.md) |
| `app/ (AppKit host)` | [`crates/app-host.md`](crates/app-host.md) |
| `recto-testpdf, recto-fuzz` | [`crates/recto-fuzz.md`](crates/recto-fuzz.md) |

## Subsystems that cross crates

| Topic | File |
|---|---|
| The renderer sandbox | [`sandbox.md`](sandbox.md) |
| Hidden text | [`hidden-text.md`](hidden-text.md) |
| Images and snapshots | [`images-and-snapshots.md`](images-and-snapshots.md) |
| Printing | [`printing.md`](printing.md) |
| Dark and warm paper | [`paper.md`](paper.md) |
| Selecting text | [`selection.md`](selection.md) |
| The sidebar | [`sidebar.md`](sidebar.md) |
| Comparison view | [`comparison.md`](comparison.md) |
| The contents follow the reader | [`toc-follow.md`](toc-follow.md) |
| The app: memory, many tabs, measurements | [`app-memory.md`](app-memory.md) |
| The QA channel | [`qa.md`](qa.md) |
| Saving | [`saving.md`](saving.md) |
| The state record | [`state-record.md`](state-record.md) |
| Deep sleep | [`sleep.md`](sleep.md) |
| Updates | [`updates.md`](updates.md) |
| Opening, encrypted files, passwords | [`opening.md`](opening.md) |
| Links, Back and Forward, page labels | [`navigation.md`](navigation.md) |
| The search index | [`search-index.md`](search-index.md) |
| The search command | [`search.md`](search.md) |
| Find | [`find.md`](find.md) |
| Highlights | [`highlight.md`](highlight.md) |
| Generated contents | [`generated-contents.md`](generated-contents.md) |
| The agent interface | [`agent-interface.md`](agent-interface.md) |
| Slow mode | [`slow-mode.md`](slow-mode.md) |
| One failure is enough: evidence, reruns, like-CI, main's CI | [`failures.md`](failures.md) |
| Notices (each dependency is in its crate file) | [`dependencies.md`](dependencies.md) |
| Machine notes | [`machine-notes.md`](machine-notes.md) |

## Quality bar

- `unsafe` is forbidden in every crate except the OS seams (`recto-platform`, `recto-ffi`) and the PDFium seam (`recto-render/src/pdfium.rs` and `pdfium/contents.rs`). Every `unsafe` has a SAFETY comment.
- Clippy with `-D warnings`.
- No panics on untrusted input. Collections grown from outside input have a cap.
- A struct held per item documents its size and asserts it in a test.
- Dependencies count: `default-features = false`, few crates, and a line below saying why each one is here.

## How to add to these docs

- A feature branch edits only its own crate's file under `crates/` or its own subsystem file, and nothing else in this folder.
- A new crate gets its own file under `crates/` (its contract, what it must not do, per-item sizes, dependencies and measurements) and one line in the table above, in layering order. That line is the only shared edit.
- A new subsystem gets a file here and one line in the second table.
- Per-item sizes and dependencies go in the file of the crate that holds the struct or uses the dependency, each under its own heading. Run `tools/notices.py` after any dependency change.
- Measurements keep their date, the build and the load they were taken under, and the owner's quotes stay with the decision they belong to.
