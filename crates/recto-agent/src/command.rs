//! The top-level command: every command is a family's, and each list here hands it to its family. Spelled out for every family, with no default arm, so a family can't be left out of one.

use crate::Command;
use crate::table::Access;
use serde_json::Value;

impl Command {
    /// **The line an agent sends for this command,** which [`crate::parse`] reads back as the same command: so a person's command, made by the window, is
    /// literally a line an agent could send (R2.3, C6).
    #[must_use]
    pub fn wire(&self) -> String {
        let line = match self {
            Self::Measure(command) => serde_json::to_string(command),
            Self::Edit(command) => serde_json::to_string(command),
            Self::Move(command) => serde_json::to_string(command),
            Self::Read(command) => serde_json::to_string(command),
            Self::Start(command) => serde_json::to_string(command),
            Self::Waits(command) => serde_json::to_string(command),
            Self::Rights(command) => serde_json::to_string(command),
            Self::Session(command) => serde_json::to_string(command),
            Self::Qa(command) => serde_json::to_string(command),
            Self::Record(command) => serde_json::to_string(command),
            Self::Updates(command) => serde_json::to_string(command),
        };
        // Every payload is plain fields of strings, numbers, switches and paths, which always make JSON.
        line.unwrap_or_default()
    }

    /// The name on the wire, which is the first column of the command table with spaces as underscores.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Measure(command) => command.tag(),
            Self::Edit(command) => command.tag(),
            Self::Move(command) => command.tag(),
            Self::Read(command) => command.tag(),
            Self::Start(command) => command.tag(),
            Self::Waits(command) => command.tag(),
            Self::Rights(command) => command.tag(),
            Self::Session(command) => command.tag(),
            Self::Qa(command) => command.tag(),
            Self::Record(command) => command.tag(),
            Self::Updates(command) => command.tag(),
        }
    }

    /// Who may run this: its row's level, or what it asks for when that's more.
    pub fn access(&self) -> Access {
        match self {
            Self::Measure(command) => command.access(),
            Self::Edit(command) => command.access(),
            Self::Move(command) => command.access(),
            Self::Read(command) => command.access(),
            Self::Start(command) => command.access(),
            Self::Waits(command) => command.access(),
            Self::Rights(command) => command.access(),
            Self::Session(command) => command.access(),
            Self::Qa(command) => command.access(),
            Self::Record(command) => command.access(),
            Self::Updates(command) => command.access(),
        }
    }

    /// Whether this draws or reads pages through a worker. These run at most a few at a time, because each can use hundreds of megabytes.
    pub fn draws(&self) -> bool {
        match self {
            Self::Measure(command) => command.draws(),
            Self::Edit(command) => command.draws(),
            Self::Move(command) => command.draws(),
            Self::Read(command) => command.draws(),
            Self::Start(command) => command.draws(),
            Self::Waits(command) => command.draws(),
            Self::Rights(command) => command.draws(),
            Self::Session(command) => command.draws(),
            Self::Qa(command) => command.draws(),
            Self::Record(command) => command.draws(),
            Self::Updates(command) => command.draws(),
        }
    }

    /// Whether it reads what's on a tab's pages, which a tab's edits say: refused while they can't be read.
    pub(crate) fn reads_pages(&self) -> bool {
        match self {
            Self::Measure(command) => command.reads_pages(),
            Self::Edit(command) => command.reads_pages(),
            Self::Move(command) => command.reads_pages(),
            Self::Read(command) => command.reads_pages(),
            Self::Start(command) => command.reads_pages(),
            Self::Waits(command) => command.reads_pages(),
            Self::Rights(command) => command.reads_pages(),
            Self::Session(command) => command.reads_pages(),
            Self::Qa(command) => command.reads_pages(),
            Self::Record(command) => command.reads_pages(),
            Self::Updates(command) => command.reads_pages(),
        }
    }

    /// Whether, run for the person, it may hold its thread for long: see [`crate::commands::Cmd::WAITS`].
    pub fn waits(&self) -> bool {
        match self {
            Self::Measure(command) => command.waits(),
            Self::Edit(command) => command.waits(),
            Self::Move(command) => command.waits(),
            Self::Read(command) => command.waits(),
            Self::Start(command) => command.waits(),
            Self::Waits(command) => command.waits(),
            Self::Rights(command) => command.waits(),
            Self::Session(command) => command.waits(),
            Self::Qa(command) => command.waits(),
            Self::Record(command) => command.waits(),
            Self::Updates(command) => command.waits(),
        }
    }

    /// Whether it reads nothing of a document, so it may act on one the user opened with its password.
    pub(crate) fn protected_ok(&self) -> bool {
        match self {
            Self::Measure(command) => command.protected_ok(),
            Self::Edit(command) => command.protected_ok(),
            Self::Move(command) => command.protected_ok(),
            Self::Read(command) => command.protected_ok(),
            Self::Start(command) => command.protected_ok(),
            Self::Waits(command) => command.protected_ok(),
            Self::Rights(command) => command.protected_ok(),
            Self::Session(command) => command.protected_ok(),
            Self::Qa(command) => command.protected_ok(),
            Self::Record(command) => command.protected_ok(),
            Self::Updates(command) => command.protected_ok(),
        }
    }

    /// The selector of the tab this acts on, which `execute` holds awake while it runs.
    pub(crate) fn tab_selector(&self) -> Option<&str> {
        match self {
            Self::Measure(command) => command.tab(),
            Self::Edit(command) => command.tab(),
            Self::Move(command) => command.tab(),
            Self::Read(command) => command.tab(),
            Self::Start(command) => command.tab(),
            Self::Waits(command) => command.tab(),
            Self::Rights(command) => command.tab(),
            Self::Session(command) => command.tab(),
            Self::Qa(command) => command.tab(),
            Self::Record(command) => command.tab(),
            Self::Updates(command) => command.tab(),
        }
    }

    /// What goes in the activity log about this command's arguments. Numbers and cleaned file names, and never anything that could hold document text: a tab's name as typed or a search's words aren't kept.
    pub fn log_details(&self) -> Value {
        match self {
            Self::Measure(command) => command.log_details(),
            Self::Edit(command) => command.log_details(),
            Self::Move(command) => command.log_details(),
            Self::Read(command) => command.log_details(),
            Self::Start(command) => command.log_details(),
            Self::Waits(command) => command.log_details(),
            Self::Rights(command) => command.log_details(),
            Self::Session(command) => command.log_details(),
            Self::Qa(command) => command.log_details(),
            Self::Record(command) => command.log_details(),
            Self::Updates(command) => command.log_details(),
        }
    }
}
