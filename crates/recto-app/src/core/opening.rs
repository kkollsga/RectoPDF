//! **Opening and waking files, as the session thread does them:** a tap, an open, and a file that turned out other than the one asleep, each read off the thread.
//! A tap on a tab's row, opening a file (the Open panel, a drop, the Finder, Open Recent, files named at launch) and File > Open Recent > Clear Menu are `tap`,
//! `open` and `recent clear`, the commands an agent sends, run for the person (R2.3, C5c), with the window's own steps around them here: [`before`] and
//! [`answered`].

use super::*;
use recto_agent::Code;
use recto_agent::TabRef;
use recto_agent::commands::Move;
use recto_agent::commands::tabs::{Open, RecentClear, Tap};

/// The tab a person's tap is for, as the window names it: by its id. A tap on its row, or its row in the palette.
pub fn tap(tab: TabId) -> Command {
    Tap { tab: TabRef::Id(tab) }.into()
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
            if let TabRef::Id(id) = tab {
                inner.engine.waking().note_wake(id.0, Instant::now());
            }
        }
        Command::Move(Move::Open(Open { path, .. })) => {
            reads.wanted_tab = None;
            inner.window.notices().clear_open_failure();
            let (tab, own) = inner.host.session.with_session(|session| {
                let tab = session.tabs().for_path(path);
                (tab, tab.and_then(|id| session.tabs().path_of(id)))
            });
            lock(&inner.tools.prompts().passwords).opened_again(&[Some(path.clone()), own].into_iter().flatten().collect::<Vec<_>>(), tab);
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
        (Command::Move(Move::Open(Open { path, .. })), Some(error)) if error.code == Code::lower(OpenError::PasswordNeeded.code()) => {
            lock(&inner.tools.prompts().passwords).ask(passwords::Target::File, path);
        }
        (Command::Move(Move::Open(_)), Some(error)) => {
            if let Some(words) = person::session_words(error) {
                inner.window.notices().fail_open(words.to_string());
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

/// **Opening's menu ids** ([`super::menu_rows`]): an Open Recent file is `open` on the very file the menu showed, read off the session thread as every open
/// the window makes is, and nothing once it's left the list; Clear Menu is `recent clear`; Open… shows the system's panel.
pub fn row<'a>(inner: &'a Arc<Inner>, asked: &menu_rows::Asked<'_>) -> Option<menu_rows::Row<'a>> {
    use menu_rows::Row;
    match (asked.action.as_ref()?, asked.id) {
        (menus::Action::OpenRecent(key), _) => {
            let path = recent_path(&lock(&inner.published).recent, key);
            Some(path.map_or(Row::Nothing, |path| {
                Row::own_way(
                    Open {
                        path: path.clone(),
                        page: None,
                    },
                    "an open is read off the session thread, on its own queue, as every open the window makes is",
                    move || {
                        open_for_person(&inner.life, &inner.core, path);
                        HostAction::None
                    },
                )
            }))
        }
        (menus::Action::ClearRecent, _) => Some(Row::command(RecentClear {})),
        (menus::Action::Host(host), "file.open") => Some(Row::host(
            "opens the system's Open panel for the person; what's chosen there is `Open`",
            menu_rows::Agents::ItsOwn("`recto open <path>`"),
            *host,
        )),
        (menus::Action::Host(host), "file.open_recent") => Some(Row::host("a submenu's own item only opens it", menu_rows::Agents::Never, *host)),
        _ => None,
    }
}

/// `CoreCall::LookAtShown`, on the session thread.
pub fn look_at_shown(inner: &Arc<Inner>) {
    for id in inner.host.session.with_session(|session| session.panes()) {
        inner.on_disk.sleeper().check_woken_file(&inner.host.session, &inner.core, id);
    }
}

/// `CoreCall::WokenFileIsOther`, on the session thread.
pub fn woken_file_is_other(inner: &Arc<Inner>, reads: &mut FileReads, id: TabId, status: FileStatus, against: Option<recto_platform::FileStamp>) {
    let host = &inner.host;
    // `NotLooked` comes from a look before a show that's taken the news in already, and wants the read. Otherwise what's held is forgotten and read again only if
    // the look found something new. A look made against a stamp the tab has moved on from (a save landed meanwhile) is about nothing now (F8).
    let again = matches!(status, FileStatus::NotLooked);
    let new = host.session.with_session(|session| {
        if session.tabs().own_stamp(id) != against {
            return false;
        }
        let new = session.apply_file_status(id, status) || again;
        if new {
            session.sleeper().forget_read(id);
        }
        new
    });
    if !new {
        return;
    }
    let reread = host.session.with_session(|session| {
        session
            .tabs()
            .active()
            .filter(|active| *active == id)
            .and_then(|id| Some((id, session.tabs().path_of(id)?, session.tabs().own_stamp(id))))
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
    let wrong = host.session.with_session(|session| session.sleeper().file_length_is(id, pages, version));
    let reread = host.session.with_session(|session| {
        session
            .tabs()
            .active()
            .filter(|active| wrong && *active == id)
            .and_then(|id| Some((id, session.tabs().path_of(id)?, session.tabs().own_stamp(id))))
    });
    if let Some((id, path, stamp)) = reread {
        reads.wanted_tab = Some(id);
        reads.start_restored(inner, id, path, stamp);
    }
}

/// **A file the person opens:** the Open panel's, a drop, the Finder's, File > Open Recent's and the palette's recent file, all this one way. `open`, queued
/// for the session thread, as [`open`]; and asked for before the state's read, it's what a launch that goes back passes on.
pub fn open_for_person(life: &Lifecycle, core: &CoreQueue, path: PathBuf) {
    if !life.state_read() {
        updates::note_asked_to_open(std::slice::from_ref(&path));
    }
    open(core, path);
}

/// **Opens a file from the files opened lately by its key,** as File > Open Recent and the palette's row do: the very file that was offered, however the
/// list has moved since, and nothing if it's left the list. `recent` is the list the session thread last published: it never waits.
pub fn open_recent(recent: &[RecentFile], life: &Lifecycle, core: &CoreQueue, key: &str) {
    if let Some(path) = recent_path(recent, key) {
        open_for_person(life, core, path);
    }
}

/// The file opened lately that a menu row or palette row offered, by its key: `None` once it's left the list.
fn recent_path(recent: &[RecentFile], key: &str) -> Option<PathBuf> {
    recent.iter().find(|file| file.key == key).map(|file| PathBuf::from(&file.path))
}

/// **What a row of the sidebar comes to, tapped** (`adding` for a ⌘- or ⌃-click): a tab's is `tap`, or with `adding` `compare --toggle`; a contents row's
/// is `goto` its page, and the note over a tab's contents is its action, whichever the click. A contents row with no page does nothing.
pub fn row_command(row: SidebarRow, adding: bool, panes: &panes::Panes, engine: &Engine, contents: &[crate::ContentsEntry]) -> Option<Command> {
    match (row.kind, row.page) {
        (crate::RowKind::Tab, _) if adding => Some(super::comparison::toggle(TabId(row.tab))),
        (crate::RowKind::Tab, _) => Some(tap(TabId(row.tab))),
        (crate::RowKind::Contents, Some(page)) => Some(super::navigation::contents_tapped(panes, engine, contents, &row, page)),
        (crate::RowKind::Contents, None) => None,
        (crate::RowKind::Note, _) => row
            .action
            .as_deref()
            .and_then(recto_session::ContentsAction::from_id)
            .map(|action| super::contents::note_command(TabId(row.tab), action)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recto_update::updater::Host as _;

    /// Failure (R3.7): a way a person opens a file goes around [`open_for_person`], the one function every open a person makes goes through (the Open
    /// panel's, a drop and the Finder's directly; File > Open Recent's and the palette's recent row by [`open_recent`]), so what it does for each open isn't
    /// done for that one. Seen here by what it does before the state's read: it notes the file for a launch that goes back, besides queuing the open. A key
    /// that's left the list opens nothing. The note is a process-wide list other tests may write to as well, so this asks only that its own files are in it.
    #[test]
    fn every_way_a_person_opens_a_file_goes_through_one_function() {
        let life = Lifecycle::new(Arc::new(crate::clock::SystemClock::new()));
        assert!(!life.state_read(), "a launch's life, before the state's read");
        let core = CoreQueue::default();
        let (panel, recent, gone) = (
            PathBuf::from("/files/one-way/panel.pdf"),
            PathBuf::from("/files/one-way/recent.pdf"),
            PathBuf::from("/files/one-way/gone.pdf"),
        );
        let file = |key: &str, path: &PathBuf| RecentFile {
            key: key.into(),
            name: "recent".into(),
            folder: "/files/one-way".into(),
            path: path.display().to_string(),
            open: false,
        };
        let list = [file("k1", &recent)];
        open_for_person(&life, &core, panel.clone());
        open_recent(&list, &life, &core, "k1");
        open_recent(&[file("k2", &gone)], &life, &core, "a key that's left the list");
        let asked = super::updates::Hooks::new(Arc::new(super::updates::RealInstaller)).documents_asked_for();
        assert!(asked.contains(&panel), "the Open panel's file noted: {asked:?}");
        assert!(asked.contains(&recent), "Open Recent's file noted, as the Open panel's is: {asked:?}");
        assert!(!asked.contains(&gone), "nothing for a key that's left the list: {asked:?}");
        let mut opened = Vec::new();
        while let Some(CoreCall::Run(run)) = core.next_or_due(|| Some(Duration::ZERO)) {
            opened.push(run.command.wire());
        }
        assert_eq!(opened.len(), 2, "each queued its open, and a key that's left the list nothing: {opened:?}");
    }
}
