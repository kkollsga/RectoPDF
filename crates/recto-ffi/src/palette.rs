//! **⌘K, the command palette: its records.** What it lists, how it's worded and ranked, and what a row runs are `recto_app::palette`'s; `core::palette` gives it the
//! window's lists and does what a row comes to.

pub(crate) use recto_app::palette::ForWhom;
pub use recto_app::palette::{PaletteRow, PaletteView};

/// One of the palette's rows.
#[uniffi::remote(Record)]
pub struct PaletteRow {
    /// What running it does, by Rust's own name: a menu item's id, or `tab:N`.
    pub id: String,
    pub title: String,
    /// Where it lives, as `View` or `Open tab`, in a quieter colour.
    pub detail: String,
    /// Its key, as `⇧⌘G`, or empty.
    pub shortcut: String,
    /// Where it lives and its key together, as the row shows them on its right.
    pub aside: String,
    /// The title comes from a file, as a tab's name does, so it's only ever drawn as text.
    pub from_file: bool,
    /// What VoiceOver says for the row: its title, where it lives, and its key in words, as "Find Next, Edit, Command G".
    pub spoken: String,
}

/// Everything the palette draws.
#[uniffi::remote(Record)]
pub struct PaletteView {
    pub open: bool,
    pub text: String,
    pub placeholder: String,
    pub rows: Vec<PaletteRow>,
    pub selected: Option<u32>,
    /// What's said with no rows.
    pub empty: String,
    /// What VoiceOver calls the list of rows.
    pub list_label: String,
}
