//! Where each command is answered: the session, the server, the host's record or only the app. One exhaustive list, which the server, the hosts and the app route by.

use crate::{AgentError, Command, exit};

/// Where a command is answered. `execute` runs the session's; the others it only says it can't, and the host sends them on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// By `execute`, on the session.
    Session,
    /// By the server's own loop: it holds the connections, the level and the log.
    Server,
    /// By the host's state record and its lifecycle: each host keeps its own.
    Record,
    /// Only by the app: its window, its updater, its QA.
    App,
}

impl Command {
    /// Where this is answered. Spelled out for every command, so a new one can't go to the wrong place by default.
    pub fn route(&self) -> Runs {
        match self {
            Self::Move(command) => command.route(),
            Self::Read(command) => command.route(),
            Self::Start(command) => command.route(),
            Self::Waits(command) => command.route(),
            Self::Rights(command) => command.route(),
            Self::Session(command) => command.route(),
            Self::SessionSleep { .. } | Self::SessionShow {} | Self::SessionSave {} | Self::SessionRestore {} | Self::SessionCheck {} => Runs::Record,
            Self::QaRead { .. }
            | Self::QaDrive { .. }
            | Self::Palette { .. }
            | Self::UpdateStatus {}
            | Self::UpdateCheck {}
            | Self::UpdateInstall { .. }
            | Self::UpdateDevBuilds { .. }
            | Self::Settings { .. }
            | Self::View { zoom: Some(_), .. } => Runs::App,
            Self::Open { .. }
            | Self::Tap { .. }
            | Self::Budget {}
            | Self::View { zoom: None, .. }
            | Self::Find { .. }
            | Self::Compare { .. }
            | Self::BudgetRender { .. }
            | Self::BudgetViewport { .. }
            | Self::Edits { .. }
            | Self::PageMove { .. }
            | Self::PageRotate { .. }
            | Self::PageDelete { .. }
            | Self::PageInsert { .. }
            | Self::Undo { .. }
            | Self::Redo { .. }
            | Self::Discard { .. }
            | Self::ClearHistory { .. }
            | Self::Save { .. } => Runs::Session,
        }
    }
}

/// Updates and settings are the app's. A session with no window has nothing to replace, and nothing to set.
pub fn update_needs_app() -> AgentError {
    AgentError::new(
        "needs_app",
        "Updates and settings belong to the app, and this session has no window.",
        exit::FAILED,
    )
    .then("Open RectoPDF, and `recto` will talk to that.")
}

/// QA commands reach an app started in QA mode, and nothing else. Other sessions, a shipped app's included, say this.
pub fn qa_needs_app() -> AgentError {
    AgentError::new("qa_needs_app", "QA commands only work on an app started with `recto qa start`.", exit::FAILED)
        .then("Set RECTO_STATE_DIR to a temp folder and run `recto qa start`.")
}
