# recto-platform

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-platform` | **Started** | File identity, process footprint, handle count, `isolate`: child process, channel with handles, shared memory, loaded library. `surface`: IOSurface, and the link that carries one to a worker. Later `fswatch` | Hold policy |

## Dependencies

| Crate | Why |
|---|---|
| `libc` | `proc_pid_rusage` for footprint, process, channel and shared-memory calls, and the control socket's `poll` and `getpeereid`, `recto-platform` only |
