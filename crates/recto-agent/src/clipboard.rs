//! **The window's clipboard, for the person's own snapshot** (S2's follow-up (1), 2026-11-14: one action, one implementation). The snapshot tool's box and
//! click are the person's `snapshot` and `image` run by the one executor, with the output only the window has: `--to-clipboard`, the clipboard's every type
//! at once, held in memory, never a file. Only the actor at the window is given a [`ClipboardSink`] ([`crate::SessionAccess::clipboard`]), on the
//! person's own door; anyone else's command asking for it is refused ([`not_for_agents`]), so nothing an agent sends reaches the pasteboard.
//!
//! **What a snapshot is, by where it goes,** is [`crate::extract::drawn_for`]'s, in one place.

use crate::commands::Read;
use crate::commands::export::{Image, Snapshot};
use crate::extract::{Clip, ClipStop};
use crate::{AgentError, Code, Command, exit};

/// **What a command to the window's clipboard is, the one place that says** (the branch's review, F1): the snapshot tool's box, its click on an image, or
/// a paste of a box's promised SVG. The person's lane orders the box and the click and not the paste, and the tool reads its answers back by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardKind {
    /// `snapshot --to-clipboard`, every type at once.
    Box,
    /// `image --at x,y --to-clipboard`.
    Click,
    /// `snapshot --as svg --to-clipboard`: the promised SVG, made for the paste waiting for it.
    Paste,
}

impl Snapshot {
    /// What this is to the window's clipboard, if it goes there.
    #[must_use]
    pub fn clipboard_kind(&self) -> Option<ClipboardKind> {
        self.to_clipboard
            .then(|| if self.format == "svg" { ClipboardKind::Paste } else { ClipboardKind::Box })
    }
}

impl Image {
    /// What this is to the window's clipboard, if it goes there.
    #[must_use]
    pub fn clipboard_kind(&self) -> Option<ClipboardKind> {
        self.to_clipboard.then_some(ClipboardKind::Click)
    }
}

/// What a command is to the window's clipboard: none for any but a `snapshot` or `image` to it.
#[must_use]
pub fn kind_of(command: &Command) -> Option<ClipboardKind> {
    match command {
        Command::Read(Read::Snapshot(asked)) => asked.clipboard_kind(),
        Command::Read(Read::Image(asked)) => asked.clipboard_kind(),
        _ => None,
    }
}

/// **The window's side of the person's snapshot to the clipboard:** the stop a newer snapshot or a quit sets, what's made for the clipboard, and the bytes a
/// paste is waiting for. Implemented by the app; a session with no window has none.
pub trait ClipboardSink: Send + Sync {
    /// The snapshot's page is found, and its drawing begins: the stop for it, which a newer snapshot, the tool turned off or a quit sets. The person's
    /// next edit goes on from here (the person's lane), as the page this reads is fixed.
    fn begin(&self) -> ClipStop;
    /// A paste's promised SVG begins: the stop for it, which the paste's patience running out sets.
    fn begin_paste(&self) -> ClipStop;
    /// What a box or a click made, every type in the order a paste should prefer them, for the window to offer the clipboard.
    fn offer(&self, clip: Clip);
    /// A promised type, made for the paste waiting for it.
    fn made(&self, bytes: Vec<u8>);
    /// **The person's Copy Link:** the file address of a page (`recto link --to-clipboard`), as plain text for the window to offer the clipboard. Given only
    /// to the person, as every part of this sink is, and never put in an answer.
    fn text(&self, text: String);
    /// **The person's look for a click's image begins** (the window's `images --at x,y` with the page as seen, on `tab`'s page from 1): the stop for it,
    /// which a newer click, a copy, the click let go or a quit sets. Asked only for that form, never for a plain `images`. One that's never stopped, for a
    /// sink with no looks of its own.
    fn begin_look(&self, _tab: u32, _page: u32, _at: [f32; 2]) -> ClipStop {
        ClipStop::default()
    }
}

/// **An agent's command asking for the window's clipboard,** refused: only the person's own snapshot goes there.
#[must_use]
pub fn not_for_agents() -> AgentError {
    AgentError::new(
        Code::PERMISSION_DENIED,
        "Only the person's own snapshot goes to the clipboard, at the window.",
        exit::PERMISSION,
    )
    .then("Give `--out` for a new file, or `--as text` for the words in the answer.")
}

/// **The person's snapshot to the clipboard without the page it was taken from,** refused, failing closed: with nothing to hold the page at its place
/// against, it could copy another page (the branch's review, S1). The window always names it.
#[must_use]
pub fn unseen() -> AgentError {
    AgentError::new(
        Code::USAGE,
        "A snapshot to the clipboard names the page it was taken from, as it was seen.",
        exit::USAGE,
    )
}

/// **The page a box, a click or a paste was taken from isn't the one at its place now** (a page moved, deleted or put in front of it since): refused,
/// rather than taking another page's picture and text.
#[must_use]
pub fn page_moved() -> AgentError {
    AgentError::new(Code::SOURCE_CHANGED, "The page the box was taken from has moved since.", exit::FAILED)
}
