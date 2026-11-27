# recto-control

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-control` | **Started** | The long-running side: the server on the local socket, the level's file, the activity log, the state folder, the thin client. A `Host` trait, so the app can carry the same socket. `structure`: the one runner for jobs of the walled structure helper (document by descriptor, checked from outside before the job, every line read as hostile, limits), with `structure::headings` the generated-contents job; `generating`: the queue, and `generating/ink.rs`, the render worker for the check for hidden text | Name a UI toolkit; hold a rule about documents |

## Per-item sizes

| Struct | Held per | Limit |
|---|---|---|
| `Conn` | Connected client, at most 128 | ≤ 320 B plus what it's sending and being sent |
| `indexing::IndexJob` | File waiting for an index, at most 16 and one running. No tab holds one | A path and 32 B |
