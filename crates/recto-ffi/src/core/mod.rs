//! **The window's parts with state of their own:** printing, the snapshot tool and the QA bridge, apart from their exported calls, which are the bridge's.
//! They move to `recto-app` whole. Their exported calls, and the records those return, stay in the bridge's module of the same name: UniFFI's checksum of a
//! call, and its place in the bindings, include its module path and its records'.

mod actions;
pub(crate) mod palette;
pub(crate) mod printing;
pub(crate) mod qa;
pub(crate) mod snapshot_tool;
