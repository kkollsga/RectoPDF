//! **The clipboard's calls:** what a copy offers, one type's bytes, and letting it go. The tool and its state are `recto_app::core::snapshot_tool`'s.

use crate::AppCore;
pub use recto_app::core::snapshot_tool::ClipboardOffer;

/// What the clipboard is offered: its types in the order to offer them, which of those are made only when a paste asks, and the number that names this offer for [`AppCore::clipboard_data`].
#[uniffi::remote(Record)]
pub struct ClipboardOffer {
    pub serial: u64,
    /// Uniform type identifiers, such as `public.png`, in the order a paste should prefer them.
    pub types: Vec<String>,
    /// The ones made only when a paste asks for them: the host declares them and gives them when asked.
    pub promised: Vec<String>,
}

#[uniffi::export]
impl AppCore {
    /// What a copy has for the clipboard, a snapshot's or the selection's text, as one offer: the types in order, and which are made only when a paste asks. `copy_ready` says when. The bytes come one type at a time from `clipboard_data`, so no more than one is copied across at once.
    pub fn take_clipboard(&self) -> Option<ClipboardOffer> {
        self.inner.tools.tool().take_offer(&self.inner.panes.engines(&self.inner.engine))
    }

    /// One type's prepared bytes for this offer, once. The synchronous system callback never starts a command or waits for rendering. Nothing for an old offer, an absent type or bytes not prepared.
    pub fn clipboard_data(&self, serial: u64, uti: String) -> Option<Vec<u8>> {
        self.inner.tools.tool().ready_bytes(serial, &uti)
    }

    /// The clipboard has something else on it now: what this offer promised is let go.
    pub fn clipboard_released(&self, serial: u64) {
        self.inner.tools.tool().offer_released(serial);
    }
}
