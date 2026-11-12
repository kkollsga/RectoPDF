//! **The snapshot tool and the clipboard, as the window has them:** what a copy offers.

/// What the clipboard is offered: its types in the order to offer them, which of those are made only when a paste asks, and the number that names this offer for the bridge's `clipboard_data`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardOffer {
    pub serial: u64,
    /// Uniform type identifiers, such as `public.png`, in the order a paste should prefer them.
    pub types: Vec<String>,
    /// The ones made only when a paste asks for them: the host declares them and gives them when asked.
    pub promised: Vec<String>,
}
