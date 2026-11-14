//! **A file that asks not to be copied asks "anyway"** (owner, 2026-11-07: text copy, with "Copy anyway", and an agent keeps `--anyway`; 2026-11-08:
//! images, snapshots and exports, SVG, PNG and a print to a PDF, "Yes, same step"; print to paper never asks). The rule and its words, once: every
//! command and tool that copies or exports a document asks [`asks_anyway`] or [`refuse_unless_anyway`].
//!
//! It isn't on [`crate::Actor`], as it doesn't turn on who asks: the person passes it at a notice's button and an agent with `--anyway`, alike, and
//! that an agent's use goes in the activity log is the server's, which logs agents' commands only.

use crate::{AgentError, Code, exit};

/// What's being copied, for the words of its refusal.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Copying {
    /// A document's text: `copy`, `select`.
    Text,
    /// A picture or an export of it: `image`, `snapshot`, `render`, `export svg`.
    File,
    /// A print to a PDF file, which is an export.
    PrintToPdf,
}

/// **Whether this copy is held back:** the file asks not to be copied and "anyway" hasn't been said. `restricted` reads the file's flag, and is only
/// asked when "anyway" hasn't been said.
pub fn asks_anyway(anyway: bool, restricted: impl FnOnce() -> bool) -> bool {
    !anyway && restricted()
}

/// **Refuses a copy held back** ([`asks_anyway`]), exit 8 and `copy_restricted`, with what's copied's own words.
pub(crate) fn refuse_unless_anyway(anyway: bool, restricted: impl FnOnce() -> bool, what: Copying) -> Result<(), AgentError> {
    if !asks_anyway(anyway, restricted) {
        return Ok(());
    }
    let (message, next) = match what {
        Copying::Text => (
            "This file's permissions ask that its text isn't copied.",
            "Tell the user. `--anyway` copies it, once they've said to. Each use goes in the activity log.",
        ),
        Copying::File => (
            "This file's permissions ask that it isn't copied.",
            "Tell the user. `--anyway` copies it, once they've said to. Each use goes in the activity log.",
        ),
        Copying::PrintToPdf => (
            "This file's permissions ask that it isn't copied.",
            "Tell the user. `--anyway` prints it to a PDF, once they've said to. Each use goes in the activity log. Print to paper never asks.",
        ),
    };
    Err(AgentError::new(Code::COPY_RESTRICTED, message, exit::PROTECTED).then(next))
}

/// **What a print says of "anyway":** a print to paper never asks (SPEC, Printing), so it goes as if it had been said; one to a PDF file is an export, and
/// asks.
#[must_use]
pub fn print_anyway(to_file: bool) -> bool {
    !to_file
}
