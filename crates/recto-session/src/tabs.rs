//! Tabs: the registry, opening, closing, tapping, recent files and passwords.

use super::*;

impl<L: DocLoader> Session<L> {
    pub fn new(loader: L) -> Self {
        Self {
            loader,
            viewing: viewing::Viewing::default(),
            tabs: registry::Tabs::default(),
            warm: warm::Warm::default(),
            lists: lists::Lists::default(),
            dormancy: dormancy::Dormancy::default(),
            mail: mail::PageViewMail::default(),
            journals: journals::Journals::default(),
            generated: generated::Generated::default(),
            comparison: None,
            saves: saves::Saves::default(),
            recent: recent::Recent::default(),
        }
    }

    /// How documents are read. Cheap to reach, so a caller can read a file without holding the session.
    pub fn loader(&self) -> &L {
        &self.loader
    }

    /// Opens a PDF as a new active tab. **A PDF can only be opened once** (SPEC decision 26): opening a file that already has a tab goes to that tab instead, by its path or as the same file on disk, so a link goes to its tab and a copy is another file.
    pub fn open(&mut self, path: &Path) -> Result<TabId, SessionError> {
        if let Some(id) = self.go_to_open_file(path) {
            return Ok(id);
        }
        // Looked at first, so a change during the read shows as a change.
        let stamp = file_stamp(path).ok();
        if let Some(id) = self.tab_for_file(path, stamp) {
            self.reach_open_file(id, path);
            return Ok(id);
        }
        let facts = self.loader.load(path).map_err(SessionError::Open)?;
        let history = self.journals.history().cloned().and_then(|source| source.history_of(stamp, path));
        self.open_with_facts(path, facts, stamp, history)
    }

    /// Goes to the tab this file is open in, if it is, however it's reached. The first half of opening one.
    pub fn go_to_file(&mut self, path: &Path, stamp: Option<FileStamp>) -> Option<TabId> {
        let id = self.tab_for_file(path, stamp)?;
        self.reach_open_file(id, path);
        Some(id)
    }

    /// The tab already showing this file: by its path, or as the same file on disk, which a link is and a copy isn't. A tab whose file couldn't be looked at goes by its path alone.
    ///
    /// **A number is only the same file if the rest says so.** A move and a rewrite in place keep one, and another volume can have the same number under the same device. So its contents have to be what the tab knows, or the tab's own path has to lead to that number now, as it does for a file changed in place. Otherwise it's another PDF, and a tab's edits don't follow it. That stat is only for a tab with this very number, of which there's next to none. A link also leads where it leads: a tab whose file has been replaced under it, with a new number, is still what its own path is.
    pub fn tab_for_file(&self, path: &Path, stamp: Option<FileStamp>) -> Option<TabId> {
        self.tabs.for_path(path).or_else(|| {
            let stamp = stamp.filter(|stamp| stamp.inode != 0)?;
            let same_number = self.tabs.all().iter().find(|tab| {
                tab.file.stamp.is_some_and(|known| {
                    known.inode == stamp.inode
                        && known.device == stamp.device
                        && (known.same_contents(stamp)
                            || self
                                .tabs()
                                .path_of(tab.id)
                                .and_then(|own| file_stamp(&own).ok())
                                .is_some_and(|now| now.inode == stamp.inode && now.device == stamp.device))
                })
            });
            same_number
                .map(|tab| tab.id)
                .or_else(|| path.canonicalize().ok().and_then(|real| self.tabs.for_path(&real)))
        })
    }

    /// Goes to the tab a file is open in, reached by another name. If the name it was open under has gone, the file moved, and the tab goes with it. A stat on the volume this path has just answered from, so it can't be a dead one.
    fn reach_open_file(&mut self, id: TabId, path: &Path) {
        let gone = self.tabs.path_of(id).is_some_and(|known| known != path && !known.exists());
        if gone {
            self.relocate(id, path);
        }
        self.activate(id);
        if let Some(own) = self.tabs.path_of(id) {
            self.recent.note(&own);
        }
    }

    /// Files opened lately, newest first, with the tab each is open in now. Never reads a file: one that's gone says so when it's opened.
    pub fn recent_files(&self) -> Vec<RecentFile> {
        self.recent
            .paths()
            .map(|path| RecentFile {
                path: path.clone(),
                name: tab_name(path),
                open: self.tabs.for_path(path),
            })
            .collect()
    }

    /// Activates the tab already showing this file, if there is one. The first half of [`Self::open`], for a caller that reads the file without holding the session.
    pub fn go_to_open_file(&mut self, path: &Path) -> Option<TabId> {
        let id = self.tabs.for_path(path)?;
        self.activate(id);
        self.recent.note(path);
        Some(id)
    }

    /// The second half of [`Self::open`]: makes the tab from facts already read, and what's left of the PDF's history, if a [`HistorySource`] found any. It's the one way a tab comes to exist, so every route has to say. Someone may have opened the same file meanwhile, and then that's the tab, and the history stays where it is.
    pub fn open_with_facts(&mut self, path: &Path, facts: DocFacts, stamp: Option<FileStamp>, history: Option<TakenHistory>) -> Result<TabId, SessionError> {
        if let Some(id) = self.tab_for_file(path, stamp) {
            // Read already, so no read with the session held if it's cold.
            self.give_woken(id, Ok(facts), stamp);
            self.reach_open_file(id, path);
            return Ok(id);
        }
        let dir_path = path.parent().unwrap_or(Path::new(""));
        let file_name = path.file_name().ok_or(SessionError::Open(OpenError::NotFound))?;
        if self.tabs.len() >= MAX_TABS {
            return Err(SessionError::TooManyTabs);
        }
        // Refused before its folder's kept: a folder is let go of only when its last tab goes, and this would have none.
        if self.tabs.next_id() >= MAX_TAB_ID {
            return Err(SessionError::TooManyTabs);
        }
        let dir = self.tabs.intern_dir(dir_path);
        let id = self.tabs.take_id();
        let name = tab_name(path);
        self.tabs.push(TabRecord {
            from_sleep: None,
            touched: 0,
            id,
            name: name.into(),
            page: 0,
            page_count: facts.page_count,
            toc_open: true,
            problem: None,
            renderer_only: facts.renderer_only,
            view_rotation: 0,
            spread: record::Spread::Single,
            place_dirty: true,
            contents_verdict: ContentsVerdict::Fine,
            generated_contents: false,
            folds: None,
            file: TabFile {
                dir,
                file_name: file_name.into(),
                stamp,
                news: None,
                encrypted: false,
                password: false,
            },
            reading: TabPlace {
                place: None,
                selection: None,
                trail: None,
            },
        });
        if let Some(tab) = self.tabs.tab_mut(id) {
            tab.file.encrypted = facts.encrypted;
        }
        self.remember_warm(id, Arc::new(facts.toc));
        self.tabs.set_active(Some(id));
        self.recent.note(path);
        self.generated.keep_ready_only_for(id);
        // A new PDF shows alone, as going to any tab that isn't side by side does.
        self.comparison = None;
        if let Some(history) = history {
            let rolled_off = history.restored.rolled_off;
            self.take_up_history(id, &history.name, history.restored.edits, history.restored.seq);
            if rolled_off && let Some(tab) = self.tabs.tab_mut(id) {
                tab.file.set_notice(Some(FileNotice::HistoryRolledOff));
            }
        }
        Ok(id)
    }

    /// **A protected tab, opened with the password the person typed:** its facts, read with that password off this lock, take the place of the problem it had, as a wake does. The password itself never comes here.
    pub fn unlock(&mut self, id: TabId, facts: DocFacts, stamp: Option<FileStamp>) {
        if self.tabs.tab(id).is_none() {
            return;
        }
        self.tabs.note_password_used(id);
        self.give_woken(id, Ok(facts), stamp);
        if self.tabs.active() == Some(id) {
            self.activate(id);
        }
    }

    /// **A file opened with the password the person typed,** as [`Self::open_with_facts`] opens one: the tab is marked, so nothing read from it goes to an agent.
    pub fn open_unlocked(&mut self, path: &Path, facts: DocFacts, stamp: Option<FileStamp>) -> Result<TabId, SessionError> {
        let id = self.open_with_facts(path, facts, stamp, None)?;
        self.tabs.note_password_used(id);
        Ok(id)
    }

    /// **A tab's password went missing:** the renderer couldn't open its file with any password it has, as when the file moved to another volume or was replaced while it was open. What's held of it is let go, and the tab says it needs its password, so the app asks for it again with the sheet rather than showing pages it can't draw.
    pub fn needs_password_again(&mut self, id: TabId) {
        if self.tabs.tab(id).is_none() || self.tabs.needs_password(id) {
            return;
        }
        self.forget_read(id);
        if let Some(tab) = self.tabs.tab_mut(id) {
            tab.problem = Some(OpenError::PasswordNeeded);
        }
    }

    /// **The sidebar's tap rule.** Tapping another PDF switches to it. Tapping the active PDF folds or opens its contents.
    pub fn tap(&mut self, id: TabId) -> Result<Option<TabId>, SessionError> {
        if self.tabs.tab(id).is_none() {
            return Err(SessionError::NoSuchTab);
        }
        let before = self.tabs.active();
        // A plain click on a tab during a comparison returns to one, that one (owner, 2026-11-08: comparison is by ⌘-click).
        let comparing = self.comparison.take().is_some();
        if self.tabs.active() == Some(id) && !comparing {
            if let Some(tab) = self.tabs.tab_mut(id) {
                tab.toc_open = !tab.toc_open;
            }
        } else {
            self.activate(id);
        }
        Ok(change::switched(before, self.tabs.active()))
    }

    pub(crate) fn relocate(&mut self, id: TabId, path: &Path) {
        // With no tab, or no file to move to, there'd be nothing in a new folder to let it go.
        if self.tabs.tab(id).is_none() || path.file_name().is_none() {
            return;
        }
        let dir = self.tabs.intern_dir(path.parent().unwrap_or(Path::new("")));
        let name = tab_name(path);
        let mut left = None;
        if let (Some(tab), Some(file_name)) = (self.tabs.tab_mut(id), path.file_name()) {
            left = Some(tab.file.dir).filter(|was| *was != dir);
            tab.file.dir = dir;
            tab.file.file_name = file_name.into();
            tab.name = name.into();
        }
        if let Some(was) = left {
            self.tabs.let_go_of_dir_if_unused(was);
        }
    }

    /// Closes a tab and releases what it held. The next tab along, or the previous one, becomes active, and the answer says which.
    /// Refuses while there are unsaved changes, or undone edits of the owner's, so nothing is lost by accident. [`Self::discard_edits`] is how to say it's meant, as the app does when told "Don't Save".
    pub fn close(&mut self, id: TabId) -> Result<Closed, SessionError> {
        let before = self.tabs.active();
        if self.saves.is_saving(id) {
            return Err(SessionError::Saving);
        }
        if self.lists.is_edited(id) || self.lists.has_undone_owner_steps(id) {
            return Err(SessionError::UnsavedChanges);
        }
        // History with nothing unsaved stays in its journal, for the PDF's next opening, as long as what's in the file is up to date: what hasn't gone out goes with it. Without any, or with no journal being kept, there's nothing to keep.
        // A list that won't read: nothing unsaved is at stake, or this wouldn't have got here. Its journal's left for a launch to look at.
        if !self.wake_list(id) {
            self.lists.forget_asleep(id);
            self.journals.forget_name(id);
        }
        let keeps_history = self.journals.journaling() && self.lists.get(id).is_some_and(PageEdits::has_history);
        if keeps_history {
            if let Some(append) = self.append_for(id) {
                self.journals.depart(JournalAppend { closing: true, ..append });
            }
            self.journals.forget_name(id);
        } else {
            self.let_go_of_list(id);
        }
        self.journals.drop_fresh(id);
        self.lists.remove(id);
        self.retire_sleep_file(id);
        let index = self.tabs.position(id).ok_or(SessionError::NoSuchTab)?;
        self.tabs.remove(index);
        self.mail.forget(id);
        self.generated.forget(id);
        self.warm.forget(id);
        // Out of a comparison too. With one left it's that one alone, and the reader goes to another pane rather than the next tab along.
        let mut next_pane = None;
        if let Some(comparison) = &mut self.comparison
            && comparison.contains(id)
        {
            comparison.remove(id);
            next_pane = comparison.tabs.first().copied();
            if comparison.tabs.len() < compare::MIN_PANES {
                self.comparison = None;
            }
        }
        if self.tabs.active() == Some(id)
            && let Some(next) = next_pane
        {
            self.tabs.set_active(None);
            self.activate(next);
        }
        if self.tabs.active() == Some(id) {
            self.tabs.set_active(None);
            let next = self.tabs.at(index).or_else(|| index.checked_sub(1).and_then(|i| self.tabs.at(i))).map(|t| t.id);
            if let Some(next) = next {
                self.activate(next);
            }
        }
        Ok(Closed::after(id, before, self.tabs.active()))
    }
}
