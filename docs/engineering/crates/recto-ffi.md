# recto-ffi

> Part of the engineering notes: [`../README.md`](../README.md).

## Contract

| Crate | Status | Contract | Must not |
|---|---|---|---|
| `recto-ffi` | **Started** | UniFFI, pinned to what the C# generator supports (0.31); every call cheap or handed to a parked thread. `AppCore`'s exported calls, the records' restatements, the host's listener and the launch's two answers; the app's machinery (the session thread, the host the socket runs commands on, the state) is `recto_app::core`'s | Hold rules |

## Records

The host's records are plain types in `recto-app`, restated in the bridge under `#[uniffi::remote(Record)]` (or `Enum`, `Error`) with the same fields and doc comments, so the compiler holds the two together and the bindings don't change. The bridge holds no conversions, except where a model is projected into a record (the reader model's parts, by concern, into `ReaderView`, which `recto-app` does too).

**An exported call or a record never changes module.** UniFFI's checksum of a method includes its module path and the module path of each record it takes or returns, and an object's methods are ordered by module path, then name, so moving an `#[uniffi::export]` block or a record's restatement to another module changes the bindings (found by the byte comparison, 2026-11-12). A feature whose state moves to `recto-app` leaves its exported block and its records' restatements in the bridge module of its old name: `printing.rs`, `qa.rs`, `snapshot_tool.rs`, `saving.rs`, `find.rs` and `updates.rs` are those, with the state and the machinery in `recto_app::core`, which each re-exports for `lib.rs`. `AppCore`'s own exported methods stay in `lib.rs` and forward in one line.

## Dependencies

| Crate | Why |
|---|---|
| `uniffi` | Swift bindings for the app, and C# later from the same definitions. Held at 0.31. `default-features = false`, so its command-line generator, with `clap` and the like, is only built behind the `bindgen` feature for `uniffi-bindgen`, and not linked into the app |

## Menus

**Menus and the `⋯` panel** are lists in `crates/recto-ffi/src/menus.rs`: words, order, shortcuts, enabled and ticked. Swift draws them and sends back an id, and `core.perform(id)` returns the `HostAction`, if the host has to do something only it can, like showing an open dialog. `Show Agent Icon` is remembered in the state record, as the chosen paper is. The panel is docked in the sidebar above the footer, quiet at rest, and holds Rotate and paper, and greys Rotate out where it can't turn anything.
