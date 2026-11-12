# recto-proto

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-proto` | **Started** | Job and tile wire types, no dependencies. The host validates everything from the renderer | |

## Per-item sizes

| Struct | Held per | Limit |
|---|---|---|
| `RenderJob`, `TileReply` | Job in a capped queue, reply in flight | ≤ 64 B |
| `CharRecord` | Character of a page in hand | 4 B |
