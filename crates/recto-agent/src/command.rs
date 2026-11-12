//! The top-level command: a family's, or one of the commands the app's own code still builds and matches by their fields (`recto-ffi`, `recto-app`). Those move into families when the window runs commands itself (step R2.2's next slice, then R2.3). Every list here is spelled out for every command, with no default arm, so a command can't be left out of one.

use crate::table::{self, Access};
use crate::{Command, Level, clean_range, file_label};
use serde_json::{Value, json};

impl Command {
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
            Self::Open { .. } => "open",
            Self::Tap { .. } => "tap",
            Self::View { .. } => "view",
            Self::Compare { .. } => "compare",
            Self::Palette { .. } => "palette",
            Self::Find { .. } => "find",
            Self::QaRead { .. } => "qa_read",
            Self::QaDrive { .. } => "qa_drive",
            Self::PageRotate { .. } => "page_rotate",
            Self::Undo { .. } => "undo",
            Self::Redo { .. } => "redo",
            Self::ClearHistory { .. } => "clear_history",
            Self::SessionSleep { .. } => "session_sleep",
            Self::Save { .. } => "save",
            Self::SessionShow {} => "session_show",
            Self::SessionSave {} => "session_save",
            Self::SessionRestore {} => "session_restore",
            Self::SessionCheck {} => "session_check",
            Self::UpdateStatus {} => "update_status",
            Self::UpdateCheck {} => "update_check",
            Self::UpdateInstall { .. } => "update_install",
            Self::UpdateDevBuilds { .. } => "update_dev_builds",
            Self::Settings { .. } => "settings",
        }
    }

    /// Who may run this: its row's level, or what it asks for when that's more.
    pub fn access(&self) -> Access {
        // A command with no row can't run: the table is the list.
        let row = || table::by_tag(self.tag()).map_or(Access::Nobody, |spec| spec.access);
        match self {
            Self::Measure(command) => command.access(),
            Self::Edit(command) => command.access(),
            Self::Move(command) => command.access(),
            Self::Read(command) => command.access(),
            Self::Start(command) => command.access(),
            Self::Waits(command) => command.access(),
            Self::Rights(command) => command.access(),
            Self::Session(command) => command.access(),
            // Asking how things are shown changes nothing. Changing it is moving around.
            Self::View {
                paper: None,
                sidebar: None,
                agent_icon: None,
                zoom: None,
                layout: None,
            } => row(),
            Self::View { .. } => Access::At(Level::Control),
            Self::Compare {
                tabs: None,
                sync: None,
                off: false,
            } => row(),
            Self::Compare { .. } => Access::At(Level::Control),
            // Running a command is doing it, as its menu item would.
            Self::Palette { run: true, .. } => Access::At(Level::Control),
            // Asking is reading. Whether this app replaces itself is as far-reaching as installing, so it's the same level.
            Self::Settings { auto_update: Some(_) } => Access::At(Level::Full),
            // The same for development builds: the switch is what lets a build that isn't a release replace the app.
            Self::UpdateDevBuilds { set: Some(_) } => Access::At(Level::Full),
            Self::Palette { .. }
            | Self::Settings { .. }
            | Self::UpdateDevBuilds { .. }
            | Self::Open { .. }
            | Self::Tap { .. }
            | Self::Find { .. }
            | Self::QaRead { .. }
            | Self::QaDrive { .. }
            | Self::PageRotate { .. }
            | Self::Undo { .. }
            | Self::Redo { .. }
            | Self::ClearHistory { .. }
            | Self::SessionSleep { .. }
            | Self::Save { .. }
            | Self::SessionShow {}
            | Self::SessionSave {}
            | Self::SessionRestore {}
            | Self::SessionCheck {}
            | Self::UpdateStatus {}
            | Self::UpdateCheck {}
            | Self::UpdateInstall { .. } => row(),
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
            Self::Open { .. }
            | Self::Tap { .. }
            | Self::View { .. }
            | Self::Compare { .. }
            | Self::Palette { .. }
            | Self::Find { .. }
            | Self::QaRead { .. }
            | Self::QaDrive { .. }
            | Self::PageRotate { .. }
            | Self::Undo { .. }
            | Self::Redo { .. }
            | Self::ClearHistory { .. }
            | Self::SessionSleep { .. }
            | Self::Save { .. }
            | Self::SessionShow {}
            | Self::SessionSave {}
            | Self::SessionRestore {}
            | Self::SessionCheck {}
            | Self::UpdateStatus {}
            | Self::UpdateCheck {}
            | Self::UpdateInstall { .. }
            | Self::UpdateDevBuilds { .. }
            | Self::Settings { .. } => false,
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
            Self::Open { .. }
            | Self::Tap { .. }
            | Self::View { .. }
            | Self::Compare { .. }
            | Self::Palette { .. }
            | Self::Find { .. }
            | Self::QaRead { .. }
            | Self::QaDrive { .. }
            | Self::PageRotate { .. }
            | Self::Undo { .. }
            | Self::Redo { .. }
            | Self::ClearHistory { .. }
            | Self::SessionSleep { .. }
            | Self::Save { .. }
            | Self::SessionShow {}
            | Self::SessionSave {}
            | Self::SessionRestore {}
            | Self::SessionCheck {}
            | Self::UpdateStatus {}
            | Self::UpdateCheck {}
            | Self::UpdateInstall { .. }
            | Self::UpdateDevBuilds { .. }
            | Self::Settings { .. } => false,
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
            Self::Tap { .. } => true,
            Self::Open { .. }
            | Self::View { .. }
            | Self::Compare { .. }
            | Self::Palette { .. }
            | Self::Find { .. }
            | Self::QaRead { .. }
            | Self::QaDrive { .. }
            | Self::PageRotate { .. }
            | Self::Undo { .. }
            | Self::Redo { .. }
            | Self::ClearHistory { .. }
            | Self::SessionSleep { .. }
            | Self::Save { .. }
            | Self::SessionShow {}
            | Self::SessionSave {}
            | Self::SessionRestore {}
            | Self::SessionCheck {}
            | Self::UpdateStatus {}
            | Self::UpdateCheck {}
            | Self::UpdateInstall { .. }
            | Self::UpdateDevBuilds { .. }
            | Self::Settings { .. } => false,
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
            Self::Tap { tab }
            | Self::PageRotate { tab, .. }
            | Self::Undo { tab }
            | Self::Redo { tab }
            | Self::ClearHistory { tab }
            | Self::SessionSleep { tab }
            | Self::Save { tab, .. } => Some(tab),
            Self::Open { .. }
            | Self::View { .. }
            | Self::Compare { .. }
            | Self::Palette { .. }
            | Self::Find { .. }
            | Self::QaRead { .. }
            | Self::QaDrive { .. }
            | Self::SessionShow {}
            | Self::SessionSave {}
            | Self::SessionRestore {}
            | Self::SessionCheck {}
            | Self::UpdateStatus {}
            | Self::UpdateCheck {}
            | Self::UpdateInstall { .. }
            | Self::UpdateDevBuilds { .. }
            | Self::Settings { .. } => None,
        }
    }

    /// What goes in the activity log about this command's arguments. Numbers and cleaned file names, and never anything that could hold document text: a tab's name as typed or a search's words aren't kept.
    pub fn log_details(&self) -> Value {
        let file = crate::commands::log_file;
        let tab_of = crate::commands::log_tab;
        match self {
            Self::Measure(command) => command.log_details(),
            Self::Edit(command) => command.log_details(),
            Self::Move(command) => command.log_details(),
            Self::Read(command) => command.log_details(),
            Self::Start(command) => command.log_details(),
            Self::Waits(command) => command.log_details(),
            Self::Rights(command) => command.log_details(),
            Self::Session(command) => command.log_details(),
            Self::Open { path, page } => json!({ "page": page, "untrusted": file(path) }),
            Self::Save { tab, out: Some(out), replace } => json!({ "tab": tab_of(tab), "replace": replace, "untrusted": file(out) }),
            Self::PageRotate { tab, pages, degrees } => json!({ "tab": tab_of(tab), "pages": clean_range(pages), "degrees": degrees }),
            Self::Tap { tab }
            | Self::Undo { tab }
            | Self::Redo { tab }
            | Self::ClearHistory { tab }
            | Self::SessionSleep { tab }
            | Self::Save { tab, out: None, .. } => json!({ "tab": tab_of(tab) }),
            Self::QaRead { verb, .. } | Self::QaDrive { verb, .. } => json!({ "verb": recto_core::clean_line(verb) }),
            Self::Settings { auto_update } => json!({ "auto_update": auto_update.as_deref().map(recto_core::clean_line) }),
            Self::UpdateDevBuilds { set } => json!({ "set": set.as_deref().map(recto_core::clean_line) }),
            Self::UpdateInstall { zip, now } => json!({ "build": zip.as_deref().map(file_label), "now": now }),
            Self::Compare { sync, off, .. } => json!({ "sync": sync.as_deref().map(recto_core::clean_line), "off": off }),
            Self::Palette { query, run, .. } => json!({ "query": query.as_deref().map(recto_core::clean_line), "run": run }),
            // Not what's looked for: that's the document's business, as a search's words are.
            Self::Find {
                query, next, previous, close, ..
            } => json!({ "query": query.is_some(), "next": next, "previous": previous, "close": close }),
            Self::View { .. }
            | Self::SessionShow {}
            | Self::SessionSave {}
            | Self::SessionRestore {}
            | Self::SessionCheck {}
            | Self::UpdateStatus {}
            | Self::UpdateCheck {} => Value::Null,
        }
    }
}
