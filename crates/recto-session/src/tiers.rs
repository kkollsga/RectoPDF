//! Tiers: the warm cache, activating a tab and waking cold ones.

use super::*;

impl<L: DocLoader> Session<L> {
    /// The tabs that keep their contents: the active one and the most recent others, at most [`MAX_WARM`].
    pub fn warm_tabs(&self) -> impl Iterator<Item = TabId> + '_ {
        self.warm.ids()
    }

    pub fn tier(&self, id: TabId) -> Tier {
        if self.tabs.active() == Some(id) {
            Tier::Hot
        } else if self.warm.contains(id) {
            Tier::Warm
        } else {
            Tier::Cold
        }
    }

    /// The tab tapping this one would wake from cold, and its file, so that can be read with this session free: a file on a dead volume mustn't hold up everything else that asks about tabs.
    pub fn wake_for_tap(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        let file = (self.tabs.active() != Some(id)).then(|| self.file_to_wake(id)).flatten()?;
        // A sleep file says all a read would, with nothing asked of the PDF.
        if self.wake_from_sleep_file(file.0) {
            return None;
        }
        Some(file)
    }

    /// The same, for closing this tab: what becomes active next.
    pub fn wake_for_close(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        if self.tabs.active() != Some(id) {
            return None;
        }
        let next = match self.pane_after(id) {
            Some(pane) => pane,
            None => {
                let index = self.tabs.position(id)?;
                self.tabs
                    .at(index + 1)
                    .or_else(|| index.checked_sub(1).and_then(|before| self.tabs.at(before)))?
                    .id
            }
        };
        self.file_to_wake_now(next)
    }

    /// The pane that's read next when the one being read leaves a comparison, by closing or a ⌘-click: the first of those left. `None` without a comparison, or one that ends with it.
    pub(crate) fn pane_after(&self, leaving: TabId) -> Option<TabId> {
        let comparison = self.comparison.as_ref()?;
        comparison
            .contains(leaving)
            .then(|| comparison.tabs.iter().copied().find(|tab| *tab != leaving))
            .flatten()
    }

    /// A cold tab's file to read ahead, unless its sleep file says all a read would.
    pub(crate) fn file_to_wake_now(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        let file = self.file_to_wake(id)?;
        if self.wake_from_sleep_file(file.0) {
            return None;
        }
        Some(file)
    }

    fn file_to_wake(&self, id: TabId) -> Option<(TabId, PathBuf)> {
        if self.warm.contains(id) {
            return None;
        }
        self.path_of(id).map(|path| (id, path))
    }

    /// Whether a read of this cold tab's file is held for its waking, so it needn't be read again.
    pub fn holds_read_of(&self, id: TabId) -> bool {
        self.warm.holds_read_of(id)
    }

    /// What was read for [`Self::wake_for_tap`] or [`Self::wake_for_close`], kept for that tab's waking. A tab closed or already woken while it was being read has no use for it. Several can be read at once, so they don't take each other's place, and the oldest goes past the cap.
    pub fn give_woken(&mut self, id: TabId, loaded: Result<DocFacts, OpenError>, stamp: Option<FileStamp>) {
        self.warm.drop_read(id);
        if self.file_to_wake(id).is_none() {
            return;
        }
        self.warm.keep_read(id, loaded, stamp);
    }

    /// A warm tab's contents and labels, shared: what the host needs to say a page by its label without asking the session again.
    pub fn warm_toc(&self, id: TabId) -> Option<Arc<Toc>> {
        self.warm.toc(id).map(Arc::clone)
    }

    /// Makes a restored active tab what it says it is, once its file has been read. No tap, so its fold isn't touched.
    pub fn wake_active(&mut self) {
        if let Some(id) = self.tabs.active() {
            self.activate(id);
        }
    }

    pub(crate) fn activate(&mut self, id: TabId) {
        // Going to a tab that isn't side by side, by opening it, an agent's `goto`, or closing what was showing, shows it alone.
        if self.comparison.as_ref().is_some_and(|comparison| !comparison.contains(id)) {
            self.comparison = None;
        }
        // Only the tab on screen holds a generated list in memory. The rest are on disk, and a cold tab stays a cold tab's size.
        self.generated.keep_ready_only_for(id);
        self.wake_list(id);
        // A moved or changed file with nothing riding on it has been said on the way past.
        if let Some(left) = self.tabs.active().filter(|left| *left != id) {
            let clean = !self.lists.has_list(left);
            self.touch(left);
            if let Some(tab) = self.tab_mut(left)
                && ((clean && matches!(tab.file.notice.as_deref(), Some(FileNotice::Moved { .. } | FileNotice::Changed)))
                    || matches!(tab.file.notice.as_deref(), Some(FileNotice::HistoryRolledOff)))
            {
                tab.file.notice = None;
            }
        }
        self.tabs.set_active(Some(id));
        if self.warm.bring_to_front(id) {
            return;
        }
        // Woken from cold: rebuild from disk, or from what was read ahead with the session free. If that fails, the tab stays and says why.
        let (loaded, stamp) = match self.warm.take_read(id) {
            Some(read) => read,
            None => {
                let stamp = self.path_of(id).and_then(|path| file_stamp(&path).ok());
                (self.path_of(id).ok_or(OpenError::NotFound).and_then(|path| self.loader.load(&path)), stamp)
            }
        };
        // The file at the list's own stamp is another length than the list says, so the list isn't about what was read: the file was replaced between its stamp and its
        // read, an edit landed between a change being noticed and the read, or a journal says what isn't so. **The edits and their journal stay**, as a file changing never
        // takes a person's edits. The stamp goes, so nothing takes this file as the one they were made on and saves them onto it, and the tab says its file changed.
        if let (Ok(facts), Some(known), Some(list)) = (&loaded, stamp, self.lists.get(id))
            && self.tab(id).is_some_and(|tab| tab.file.stamp == Some(known))
            && list.file_pages() != facts.page_count
            && let Some(tab) = self.tab_mut(id)
        {
            tab.file.stamp = None;
            tab.file.notice = Some(Box::new(FileNotice::Changed));
        }
        // Woken to another file, with only history: its steps don't apply to this, whichever way it's noticed. Unsaved edits stay.
        if let (Some(now), Some(list)) = (stamp, self.lists.get(id))
            && !list.is_edited()
            && self.tab(id).and_then(|tab| tab.file.stamp).is_some_and(|known| !known.same_contents(now))
        {
            self.clear_history(id, ClearReason::FileChanged);
            if let Some(tab) = self.tab_mut(id) {
                tab.file.notice = Some(Box::new(FileNotice::Changed));
            }
        }
        // An edited tab's length is its edit list's, not the file's: added pages must stay reachable.
        let pages_with_edits = self.lists.get(id).map(PageEdits::len);
        let mut replaced = None;
        let tab = self.tab_mut(id);
        match loaded {
            Ok(facts) => {
                if let Some(tab) = tab {
                    // With edits, the numbers in them go by the length the file had, as a changed file's isn't what they're about.
                    if pages_with_edits.is_none() {
                        tab.page_count = facts.page_count;
                    }
                    tab.page = tab.page.min(pages_with_edits.unwrap_or(facts.page_count).saturating_sub(1));
                    tab.problem = None;
                    tab.renderer_only = facts.renderer_only;
                    tab.file.encrypted = facts.encrypted;
                    tab.place_dirty = true;
                    if matches!(tab.file.notice.as_deref(), Some(FileNotice::Missing | FileNotice::NeedsPassword)) {
                        tab.file.notice = None;
                    }
                    // With edits, the numbers in them are about the version they were made on, and that must stay known.
                    if pages_with_edits.is_none() {
                        replaced = tab.file.stamp.filter(|known| stamp.is_some_and(|now| !known.same_contents(now)));
                        tab.file.stamp = stamp;
                    }
                }
                // Another version of the file than the sleep file's about, which won't be read again.
                if let (Some(known), Some(path)) = (replaced, self.path_of(id)) {
                    self.journals.retire_name(record::sleep_file_name(known, &path).into());
                }
                self.remember_warm(id, Arc::new(facts.toc));
            }
            Err(problem) => {
                if let Some(tab) = tab {
                    tab.problem = Some(problem);
                    // A password is never kept, so a protected file asks again, and says that's why.
                    if problem == OpenError::PasswordNeeded {
                        tab.file.notice = Some(Box::new(FileNotice::NeedsPassword));
                    }
                }
            }
        }
    }

    pub(crate) fn remember_warm(&mut self, id: TabId, toc: Arc<Toc>) {
        if let Some(tab) = self.tab_mut(id) {
            tab.contents_verdict = recto_core::verdict::judge(&toc, tab.page_count);
        }
        // The least recent's gone cold, so what its sleep file needs is kept long enough to write that.
        let dormancy = &mut self.dormancy;
        self.warm.push(id, toc, |left| dormancy.note_left(left));
    }

    /// Tabs just pushed out of the warm ones, and what their sleep files would say. There's a sleep file for any tab that's gone cold, by whatever way.
    pub fn take_left_warm(&mut self) -> Vec<SleepFacts> {
        let left = self.dormancy.take_left();
        left.into_iter().filter_map(|(id, toc)| self.sleep_facts_with(id, &toc)).collect()
    }
}
