//! **A command on a host that mustn't wait on a disk** (step R2.3): the window's session thread, which runs the person's commands. What a command reads from a
//! file (a cold tab's, or one being opened) is read off that thread instead, and the command runs again once it's in. A command does nothing before its read
//! but look things up and refuse, so running it again from the start is running it once. Every other host reads in place, with the session free, as it always
//! has, and never hears of this.

use crate::Code;
use crate::{AgentError, exit};
use recto_core::{DocFacts, OpenError};
use recto_platform::FileStamp;
use recto_session::{TabId, TakenHistory};
use std::path::{Path, PathBuf};

/// What a file being opened was found to be, read off the thread that asked.
#[derive(Debug)]
pub struct Fetched {
    pub stamp: Option<FileStamp>,
    pub facts: Result<DocFacts, OpenError>,
    /// The history it was left with when it was closed, if it's to be taken up.
    pub history: Option<TakenHistory>,
    /// Read with the password the person typed for it: nothing read from it goes to an agent.
    pub unlocked: bool,
}

/// How a host that mustn't wait reads what a command needs.
pub trait ReadLater {
    /// Reads this tab's file on a thread of its own and gives it to the session, then runs the command again.
    fn wake(&self, tab: TabId, path: PathBuf);
    /// What was read of a file being opened, once it's in. Until then it starts the read, runs the command again when it's done, and says `None`.
    fn load(&self, path: &Path) -> Option<Fetched>;
}

/// The code of [`later`].
pub const LATER: crate::Code = Code::LATER;

/// What a command says when it's waiting for a read: nothing's done yet, and it runs again once the read's in. Never an answer anyone sees.
#[must_use]
pub fn later() -> AgentError {
    AgentError::new(LATER, "This runs again once its file has been read.", exit::FAILED)
}

/// Whether a command's failure is only that it's waiting for a read.
#[must_use]
pub fn is_later(error: &AgentError) -> bool {
    error.code == LATER
}
