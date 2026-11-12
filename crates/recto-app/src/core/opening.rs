//! **Opening and waking files, as the session thread does them:** a tap, an open, and a file that turned out other than the one asleep, each read off the thread.

use super::*;

/// `CoreCall::LookAtShown`, on the session thread.
pub fn look_at_shown(inner: &Arc<Inner>) {
    let host = &inner.host;
    if let Some(inner) = inner.saver.inner() {
        for id in host.session.with_session(|session| session.panes()) {
            sleeper::check_woken_file(&inner, id);
        }
    }
}

/// `CoreCall::Tap`, on the session thread.
pub fn tap(inner: &Arc<Inner>, reads: &mut FileReads, id: TabId, events: &mut Vec<Event>) {
    let host = &inner.host;
    inner.engine.note_wake(id.0, Instant::now());
    // A cold tab's file is read on a thread of its own, as an `open` is, so a file on a dead volume holds up neither this thread nor anything that asks about tabs, and a later tap goes ahead.
    match host.session.with_session(|session| session.wake_for_tap(id)) {
        Some((tab, path)) => {
            reads.wanted_tab = Some(tab);
            reads.start(inner, Reading::Tab(tab), path);
        }
        None => {
            reads.wanted_tab = None;
            tap_now(inner, id, events);
        }
    }
}

/// `CoreCall::Open`, on the session thread.
pub fn open(inner: &Arc<Inner>, reads: &mut FileReads, path: PathBuf) {
    let host = &inner.host;
    *lock(&inner.window.open_failure) = None;
    // Opening a file again is what starts its password tries afresh (SPEC).
    let (tab, own) = host.session.with_session(|session| {
        let tab = session.tab_for_path(&path);
        (tab, tab.and_then(|id| session.path_of(id)))
    });
    lock(&inner.tools.prompts.passwords).opened_again(&[Some(path.clone()), own].into_iter().flatten().collect::<Vec<_>>(), tab);
    let cold = host
        .session
        .with_session(|session| session.tab_for_path(&path).and_then(|id| session.wake_for_tap(id)));
    if let Some((tab, file)) = cold {
        reads.wanted_tab = Some(tab);
        reads.start(inner, Reading::Tab(tab), file);
    } else if host.session.with_session(|session| session.go_to_open_file(&path)).is_some() {
        reads.wanted_tab = None;
    } else {
        // A link or a moved name is found once it's been looked at, on the thread reading it.
        reads.wanted_tab = None;
        reads.start(inner, Reading::File(path.clone()), path);
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
        if !self.life.restored.load(Ordering::SeqCst) {
            updates::note_asked_to_open(std::slice::from_ref(&path));
        }
        self.core.push(CoreCall::Open(path));
    }

    /// A row was tapped.
    pub fn tap_row(&self, row: SidebarRow) {
        let call = match (row.kind, row.page) {
            (crate::RowKind::Tab, _) => CoreCall::Tap(TabId(row.tab)),
            (crate::RowKind::Contents, Some(page)) => CoreCall::Jump(TabId(row.tab), page.saturating_sub(1), 0.0),
            (crate::RowKind::Contents, None) => return,
            (crate::RowKind::Note, _) => match row.action.as_deref().and_then(recto_session::ContentsAction::from_id) {
                Some(action) => CoreCall::ContentsAction(TabId(row.tab), action),
                None => return,
            },
        };
        self.core.push(call);
    }

    /// ⌘-click or ⌃-click on a row: a tab goes into a comparison, or out of it. A contents row does what a click does.
    pub fn tap_row_adding(&self, row: SidebarRow) {
        match row.kind {
            crate::RowKind::Tab => self.core.push(CoreCall::CompareClick(TabId(row.tab))),
            crate::RowKind::Contents | crate::RowKind::Note => self.tap_row(row),
        }
    }

    /// A contents row's chevron was tapped: its sections fold or open.
    pub fn toggle_contents_section(&self, first: u32) {
        let entry = lock(&self.published)
            .contents
            .get(first as usize)
            .map(|entry| (entry.tab, entry.entry, entry.expandable));
        if let Some((tab, entry, true)) = entry {
            self.core.push(CoreCall::ToggleSection(TabId(tab), entry));
        }
    }
}
