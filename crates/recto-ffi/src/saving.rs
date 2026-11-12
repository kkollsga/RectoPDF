//! **Saving's records, as the host draws them.** The rules are `recto_app::saving`'s, and the window's saves `recto_app::core::saving`'s.

pub use recto_app::saving::SaveProblem;
pub use recto_app::saving::{CloseButton, CloseChoice, ClosePrompt, SavePanelModel};

/// A save that didn't happen, as an alert over the window. Every word is Rust's.
#[uniffi::remote(Record)]
pub struct SaveProblem {
    pub title: String,
    pub message: String,
    /// A button that opens Save As, when a copy is the way on.
    pub save_as_title: Option<String>,
    pub ok_title: String,
}

/// What Save As's sheet says.
#[uniffi::remote(Record)]
pub struct SavePanelModel {
    /// The tab it's for, as it was when the sheet opened: the answer saves that tab, whichever is showing by then.
    pub tab: u32,
    pub prompt: String,
    pub suggested_name: String,
    /// The folder it opens in: the document's own.
    pub directory: String,
    /// A line on the sheet, as for a signed document. `None` for most.
    pub message: Option<String>,
}

/// What closing a document with unsaved edits asks, as any Mac app asks it.
#[uniffi::remote(Record)]
pub struct ClosePrompt {
    /// The tab it's about, as it was when it was asked.
    pub tab: u32,
    pub message: String,
    pub informative: String,
    /// In the order they're shown, each with what it does.
    pub buttons: Vec<CloseButton>,
}

/// One of the close prompt's buttons.
#[uniffi::remote(Record)]
pub struct CloseButton {
    pub title: String,
    pub choice: CloseChoice,
    /// Throws something away, so it's drawn as such.
    pub destructive: bool,
}

/// The answer to it.
#[uniffi::remote(Enum)]
pub enum CloseChoice {
    Save,
    /// Save is off for this document, as for a signed one: Save As's sheet, for an unsigned copy.
    SaveAs,
    DontSave,
    Cancel,
}
