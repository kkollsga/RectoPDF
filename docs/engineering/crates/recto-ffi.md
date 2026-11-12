# recto-ffi

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-ffi` | **Started** | UniFFI, pinned to what the C# generator supports (0.31); every call cheap or handed to a parked thread. Joins the app to the session and socket. Hang watchdog, the way QA commands reach the UI thread | Hold rules |

## Dependencies

| Crate | Why |
|---|---|
| `uniffi` | Swift bindings for the app, and C# later from the same definitions. Held at 0.31. `default-features = false`, so its command-line generator, with `clap` and the like, is only built behind the `bindgen` feature for `uniffi-bindgen`, and not linked into the app |

## Menus

**Menus and the `⋯` panel** are lists in `crates/recto-ffi/src/menus.rs`: words, order, shortcuts, enabled and ticked. Swift draws them and sends back an id, and `core.perform(id)` returns the `HostAction`, if the host has to do something only it can, like showing an open dialog. `Show Agent Icon` is remembered in the state record, as the chosen paper is. The panel is docked in the sidebar above the footer, quiet at rest, and holds Rotate and paper, and greys Rotate out where it can't turn anything.
