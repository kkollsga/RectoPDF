//! **Links, Back and Forward, and the go-to field: the words and the choices,** for the host to draw: the records here, the rules in `recto_app::linking`. Rust says what a link does and how it's put; the host shows a hint, a hand, and the system's sheet, and opens an address only when this says the person chose to.

pub use recto_app::linking::LinkPrompt;

/// What hovering over a page shows: the pointer's look, and a line saying where a link goes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PointerLook {
    /// `link`, `ibeam` or `arrow`.
    pub cursor: String,
    /// Where the link under the pointer goes, in words. Nothing off a link.
    pub hint: Option<String>,
}

/// The sheet that asks before a web or email address opens. Owner's brief, 2026-11-08: "External URIs open only after a clear, Rust-worded confirmation that shows the real target."
#[uniffi::remote(Record)]
pub struct LinkPrompt {
    /// Which asking this is, so an answer is to the sheet that was shown and no later one.
    pub id: u64,
    pub title: String,
    pub message: String,
    pub open_title: String,
    pub cancel_title: String,
    /// What opens, exactly as the PDF wrote it and as it's shown.
    pub address: String,
}

/// What the go-to field starts with and says.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct PageField {
    /// The page being read, by its label if it has one, to type over.
    pub editing_text: String,
    pub tooltip: String,
}
