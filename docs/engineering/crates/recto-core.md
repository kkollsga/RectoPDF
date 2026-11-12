# recto-core

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-core` | **Started** | Lazy document structure over the lopdf fork's `IndexedReader`: facts, page count, contents. `save`: the one place that changes a document's bytes, through the fork's `append_atomically` and `rewrite_atomically`. Other writers (a print to PDF, an export) only create new files (AGENTS rule 5) | Know about tiles, threads or an OS |

## Per-item sizes

| Struct | Held per | Limit |
|---|---|---|
| `TocEntry` | Contents item, warm tab only | ≤ 32 B |

## Dependencies

| Crate | Why |
|---|---|
| `lopdf` (fork) | The lazy `IndexedReader`, its append writer and streaming rewrite. Pinned to a git revision, `8c8ba88` on `work/streaming-rewrite` |
