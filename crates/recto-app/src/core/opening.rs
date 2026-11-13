//! **Opening and waking files, as the session thread does them:** a tap, an open, and a file that turned out other than the one asleep, each read off the thread.
//! A tap on a tab's row, opening a file (the Open panel, a drop, the Finder, Open Recent, files named at launch) and File > Open Recent > Clear Menu are `tap`,
//! `open` and `recent clear`, the commands an agent sends, run for the person (R2.3, C5c), with the window's own steps around them here: [`before`] and
//! [`answered`].

use super::*;
use recto_agent::commands::Move;
use recto_agent::commands::tabs::{Open, RecentClear, Tap};

/// The tab a person's tap is for, as the window names it, `#3`.
fn tap(tab: TabId) -> Command {
    Tap { tab: format!("#{}", tab.0) }.into()
}

/// **The window's steps before a person's tap:** the tab's wake is timed from now, for how long it takes to show, as a tap always timed it; and it's the tab
/// last asked for from here, so a read for another tab asked for before, landing later, doesn't take the window there. A tap that needs its tab read says
/// it's wanted again as its read starts.
///
/// **And before a person's open:** it's what was asked for last, as a tap is; what it said of a failure before goes; and opening a file again is what starts
/// its password tries afresh (SPEC), for the file by this name and the tab's own.
pub fn before(inner: &Inner, reads: &mut FileReads, command: &Command) {
    match command {
        Command::Move(Move::Tap(Tap { tab })) => {
            reads.wanted_tab = None;
            if let Some(id) = tab.strip_prefix('#').and_then(|id| id.parse().ok()) {
                inner.engine.note_wake(id, Instant::now());
            }
        }
        Command::Move(Move::Open(Open { path, .. })) => {
            reads.wanted_tab = None;
            *lock(&inner.window.open_failure) = None;
            let (tab, own) = inner.host.session.with_session(|session| {
                let tab = session.tab_for_path(path);
                (tab, tab.and_then(|id| session.path_of(id)))
            });
            lock(&inner.tools.prompts.passwords).opened_again(&[Some(path.clone()), own].into_iter().flatten().collect::<Vec<_>>(), tab);
        }
        _ => {}
    }
}

/// **What the window does once a person's tab command has run:** a list of files opened lately that's been cleared is written at once, as Clear Menu always
/// had it, and not when the record's next due. A tap that can't be done (a tab closed meanwhile) says nothing, as it never did.
///
/// **And once a person's open has:** a file that needs a password asks for it, on the system's sheet, rather than only saying it needs one; any other file
/// that didn't open says why by the window, in the session's words.
pub fn answered(inner: &std::sync::Arc<Inner>, ran: &person::Ran<'_>) {
    match (ran.command, ran.refused()) {
        (Command::Move(Move::RecentClear(_)), None) if ran.result.is_ok() => inner.saver.write_soon(),
        (Command::Move(Move::Open(Open { path, .. })), Some(error)) if error.code == OpenError::PasswordNeeded.code() => {
            lock(&inner.tools.prompts.passwords).ask(passwords::Target::File, path);
        }
        (Command::Move(Move::Open(_)), Some(error)) => {
            if let Some(words) = person::session_words(error) {
                *lock(&inner.window.open_failure) = Some(words.to_string());
            }
        }
        _ => {}
    }
}

/// A file to open, for the person: `open`, queued for the session thread, which holds it until the state's been read, as an open always waited. Never waits.
pub fn open(core: &CoreQueue, path: PathBuf) {
    core.push(CoreCall::Run(crate::queue::PersonCommand::input(Open { path, page: None }.into())));
}

/// Whether a person's command is an open, which waits for the state to be read.
pub fn is_open(command: &Command) -> bool {
    matches!(command, Command::Move(Move::Open(_)))
}

/// File > Open Recent > Clear Menu: `recent clear`.
pub fn clear_recent(inner: &Arc<Inner>) {
    person::submit(inner, RecentClear {}.into());
}

/// `CoreCall::LookAtShown`, on the session thread.
pub fn look_at_shown(inner: &Arc<Inner>) {
    let host = &inner.host;
    if let Some(inner) = inner.saver.inner() {
        for id in host.session.with_session(|session| session.panes()) {
            sleeper::check_woken_file(&inner, id);
        }
    }
}

/// `CoreCall::WokenFileIsOther`, on the session thread.
pub fn woken_file_is_other(inner: &Arc<Inner>, reads: &mut FileReads, id: TabId, status: FileStatus) {
    let host = &inner.host;
    host.session.with_session(|session| {
        session.apply_file_status(id, status);
        session.forget_read(id);
    });
    let reread = host.session.with_session(|session| {
        session
            .active()
            .filter(|active| *active == id)
            .and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id))))
    });
    if let Some((id, path, stamp)) = reread {
        reads.wanted_tab = Some(id);
        reads.start_restored(inner, id, path, stamp);
    }
}

/// `CoreCall::FileLength`, on the session thread.
pub fn file_length(inner: &Arc<Inner>, reads: &mut FileReads, id: TabId, pages: u32, version: u64) {
    let host = &inner.host;
    // What a sleep file said of it is wrong, so it's read after all.
    let wrong = host.session.with_session(|session| session.file_length_is(id, pages, version));
    let reread = host.session.with_session(|session| {
        session
            .active()
            .filter(|active| wrong && *active == id)
            .and_then(|id| Some((id, session.path_of(id)?, session.own_stamp(id))))
    });
    if let Some((id, path, stamp)) = reread {
        reads.wanted_tab = Some(id);
        reads.start_restored(inner, id, path, stamp);
    }
}

/// **The window's side of opening and tapping,** as `AppCore` forwards it: each goes on the session thread, as waking a cold tab or reading a file can wait on a disk.
impl Inner {
    pub fn open_file(&self, path: PathBuf) {
        // Asked for before the state's read: what a launch that goes back passes on.
        if !self.life.state_read() {
            updates::note_asked_to_open(std::slice::from_ref(&path));
        }
        open(&self.core, path);
    }

    /// A tab's row in the palette, run: `tap`, as a click on the row is.
    pub fn tap_tab(self: &Arc<Self>, tab: TabId) {
        person::submit(self, tap(tab));
    }

    /// A row was tapped: a tab's is `tap`.
    pub fn tap_row(self: &Arc<Self>, row: SidebarRow) {
        match (row.kind, row.page) {
            (crate::RowKind::Tab, _) => person::submit(self, tap(TabId(row.tab))),
            (crate::RowKind::Contents, Some(page)) => self.contents_tapped(TabId(row.tab), page),
            (crate::RowKind::Contents, None) => {}
            (crate::RowKind::Note, _) => {
                if let Some(action) = row.action.as_deref().and_then(recto_session::ContentsAction::from_id) {
                    self.contents_note_for_person(TabId(row.tab), action);
                }
            }
        }
    }

    /// ⌘-click or ⌃-click on a row: a tab goes into a comparison, or out of it. A contents row does what a click does.
    pub fn tap_row_adding(self: &Arc<Self>, row: SidebarRow) {
        match row.kind {
            crate::RowKind::Tab => self.compare_toggle_for_person(TabId(row.tab)),
            crate::RowKind::Contents | crate::RowKind::Note => self.tap_row(row),
        }
    }
}
