//! **The sidebar's Find: its records.** What it shows and what each call does are `recto_app::find`'s, and the app's parts it reaches `recto_app::core::find`'s.

pub(crate) use recto_app::core::find::*;
pub use recto_app::find::{FindRow, FindRowKind, FindView};

/// Everything the sidebar draws of Find but its rows, which are pulled with [`rows`] as they come on screen.
#[uniffi::remote(Record)]
pub struct FindView {
    pub open: bool,
    /// What's in the field.
    pub text: String,
    pub placeholder: String,
    /// The line under the field, in words. Empty with nothing typed.
    pub status: String,
    /// Goes up with every change to the rows or the line, which is when to pull them.
    pub version: u64,
    pub row_count: u32,
    /// The row of the hit gone to last.
    pub current_row: Option<u32>,
    /// A query's running, which `qa wait-settled` waits for.
    pub running: bool,
    pub close_tooltip: String,
}

#[uniffi::remote(Enum)]
pub enum FindRowKind {
    /// A page, heading its hits.
    Page,
    Hit,
}

/// A row of the hit list. A hit's words are the document's, so they're only ever drawn as text.
#[uniffi::remote(Record)]
pub struct FindRow {
    pub kind: FindRowKind,
    /// A page row's label, or the words before a hit.
    pub label: String,
    pub matched: String,
    pub after: String,
    pub current: bool,
    /// Its words include text that looks hidden, so it isn't gone to. What to say of it, quietly, or empty.
    pub note: String,
}
