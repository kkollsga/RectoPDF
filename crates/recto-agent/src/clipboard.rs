//! **The window's clipboard, for the person's own snapshot** (S2's follow-up (1), 2026-11-14: one action, one implementation). The snapshot tool's box and
//! click are the person's `snapshot` and `image` run by the one executor, with the output only the window has: `--to-clipboard`, the clipboard's every type
//! at once, held in memory, never a file. Only the actor at the window is given a [`ClipboardSink`] ([`crate::SessionAccess::clipboard`]), on the
//! person's own door; anyone else's command asking for it is refused ([`not_for_agents`]), so nothing an agent sends reaches the pasteboard.
//!
//! **What a snapshot is, by where it goes,** is [`crate::extract::drawn_for`]'s, in one place.

use crate::extract::{Clip, ClipStop};
use crate::{AgentError, Code, exit};

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
