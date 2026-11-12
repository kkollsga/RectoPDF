# Engineering

This file used to hold everything and was edited in 17 of 29 recent merges, so it conflicted constantly. It is now an index: the text moved, unchanged, into [`docs/engineering/`](engineering/README.md), one file per crate (`engineering/crates/<crate>.md`) and one per subsystem that crosses crates. **The rule: a feature branch edits only its own crate's or subsystem's file; the README's layering table gets one line when a crate is added.** Other documents and code comments that say "`docs/ENGINEERING.md`, \"Section\"" still resolve through the table below.

Start at [`engineering/README.md`](engineering/README.md): the crate graph and layering order, the quality bar, and how to add to these docs.

## Where the old sections went

| Old section | Now in |
|---|---|
| Crate graph, Quality bar | [`engineering/README.md`](engineering/README.md), each crate's contract in its `engineering/crates/<crate>.md` |
| Per-item sizes | The crate that holds the struct, under "Per-item sizes" in [`engineering/crates/`](engineering/crates/) |
| Dependencies | The crate that uses it, under "Dependencies" in [`engineering/crates/`](engineering/crates/); notices in [`engineering/dependencies.md`](engineering/dependencies.md) |
| Renderer: process, limits, memory sequence, regions, measurements | [`engineering/crates/recto-render-service.md`](engineering/crates/recto-render-service.md) |
| Renderer: sandbox | [`engineering/sandbox.md`](engineering/sandbox.md) |
| Renderer: search text, line order | [`engineering/crates/recto-index.md`](engineering/crates/recto-index.md) |
| Renderer: encrypted files | [`engineering/opening.md`](engineering/opening.md) |
| Renderer: hidden text | [`engineering/hidden-text.md`](engineering/hidden-text.md) |
| Renderer: a missing renderer, fetching PDFium | [`engineering/crates/recto-render.md`](engineering/crates/recto-render.md) |
| SVG export | [`engineering/crates/recto-svg.md`](engineering/crates/recto-svg.md) |
| Images and snapshots | [`engineering/images-and-snapshots.md`](engineering/images-and-snapshots.md) |
| Printing | [`engineering/printing.md`](engineering/printing.md) |
| The app: host, threads, tiles to the screen, AppKit | [`engineering/crates/app-host.md`](engineering/crates/app-host.md) |
| The app: regions, what's kept, zoom, files that don't answer | [`engineering/crates/recto-reader.md`](engineering/crates/recto-reader.md) |
| The app: dark and warm paper | [`engineering/paper.md`](engineering/paper.md) |
| The app: selecting text | [`engineering/selection.md`](engineering/selection.md) |
| The app: sections fold, the contents area, shortened names | [`engineering/sidebar.md`](engineering/sidebar.md) |
| The app: comparison view | [`engineering/comparison.md`](engineering/comparison.md) |
| The app: reposition and rest | [`engineering/toc-follow.md`](engineering/toc-follow.md) |
| The app: many tabs, measurements | [`engineering/app-memory.md`](engineering/app-memory.md) |
| The app: the QA channel, the watchdog | [`engineering/qa.md`](engineering/qa.md) |
| Saving | [`engineering/saving.md`](engineering/saving.md) |
| The state record | [`engineering/state-record.md`](engineering/state-record.md) |
| Deep sleep | [`engineering/sleep.md`](engineering/sleep.md) |
| Updates | [`engineering/updates.md`](engineering/updates.md) |
| Opening: recent files, files that change, passwords | [`engineering/opening.md`](engineering/opening.md) |
| Links, Back and Forward, page labels | [`engineering/navigation.md`](engineering/navigation.md) |
| The search index | [`engineering/search-index.md`](engineering/search-index.md) |
| Find | [`engineering/find.md`](engineering/find.md) |
| Generated contents | [`engineering/generated-contents.md`](engineering/generated-contents.md) |
| The agent interface | [`engineering/agent-interface.md`](engineering/agent-interface.md); the search command in [`engineering/search.md`](engineering/search.md) |
| Slow mode | [`engineering/slow-mode.md`](engineering/slow-mode.md) |
| Fuzzing | [`engineering/crates/recto-fuzz.md`](engineering/crates/recto-fuzz.md) |
| Machine notes | [`engineering/machine-notes.md`](engineering/machine-notes.md) |
