//! Tiers: the warm cache, activating a tab and waking cold ones.

use super::*;

impl<L: DocLoader> Session<L> {
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
        self.tabs.path_of(id).map(|path| (id, path))
    }

    /// What was read for [`Self::wake_for_tap`] or [`Self::wake_for_close`], kept for that tab's waking. A tab closed or already woken while it was being read has no use for it. Several can be read at once, so they don't take each other's place, and the oldest goes past the cap.
    pub fn give_woken(&mut self, id: TabId, loaded: Result<DocFacts, OpenError>, stamp: Option<FileStamp>) {
        self.warm.drop_read(id);
        if self.file_to_wake(id).is_none() {
            return;
        }
        self.warm.keep_read(id, loaded, stamp);
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
                && ((clean && matches!(tab.file.notice(), Some(FileNotice::Moved { .. } | FileNotice::Changed)))
                    || matches!(tab.file.notice(), Some(FileNotice::HistoryRolledOff)))
            {
                tab.file.set_notice(None);
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
        self.compare_read_with_edits(id, loaded.as_ref().ok().map(|facts| facts.page_count), stamp);
        // An edited tab's length is its edit list's, not the file's: added pages must stay reachable. A list still asleep, as one that couldn't be woken just
        // now, is no less the tab's edits: "not in memory" is never "none".
        let pages_with_edits = self.lists.pages(id);
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
                    if matches!(tab.file.notice(), Some(FileNotice::Missing | FileNotice::NeedsPassword)) {
                        tab.file.set_notice(None);
                    }
                    // With edits, the numbers in them are about the version they were made on, and that must stay known.
                    if pages_with_edits.is_none() {
                        replaced = tab.file.stamp.filter(|known| stamp.is_some_and(|now| !known.same_contents(now)));
                        tab.file.take_version(stamp);
                    }
                }
                // Another version of the file than the sleep file's about, which won't be read again.
                if let (Some(known), Some(path)) = (replaced, self.tabs.named_path(id)) {
                    self.journals.retire_name(record::sleep_file_name(known, &path).into());
                }
                self.remember_warm(id, Arc::new(facts.toc));
            }
            Err(problem) => {
                if let Some(tab) = tab {
                    tab.problem = Some(problem);
                    // A password is never kept, so a protected file asks again, and says that's why.
                    if problem == OpenError::PasswordNeeded {
                        tab.file.set_notice(Some(FileNotice::NeedsPassword));
                    }
                }
            }
        }
    }

    /// **What a wake read, against what the tab's list is about** (`pages` and `read` are the file's length and stamp as read). Only history, made on another
    /// version than what was read, doesn't apply to it and goes. Unsaved edits are never let go for it, as a file changing never takes a person's edits: the tab
    /// knows they don't fit ([`Self::edits_fit`]), and nothing is written. Where the version they were made on is still known it stays their stamp, so putting it
    /// back makes them fit again; where it isn't, the stamp goes, so nothing takes this file as theirs. A list still asleep (its journal couldn't be read just
    /// now) is compared by what's kept of it: the length the file had isn't, so only another version is found, never another length.
    fn compare_read_with_edits(&mut self, id: TabId, pages: Option<u32>, read: Option<FileStamp>) {
        let (Some(pages), Some(read), Some(summary)) = (pages, read, self.lists.summary(id)) else {
            return;
        };
        let (edited, list_pages) = (summary.edited, self.lists.get(id).map(PageEdits::file_pages));
        let Some(own) = self.tabs.tab(id).map(|tab| tab.file.stamp) else { return };
        let other_version = own.is_some_and(|own| !own.same_contents(read));
        // The file at the list's own stamp is another length than the list says, so the list isn't about what was read: the file was replaced between its stamp
        // and its read, an edit landed between a change being noticed and the read, or a journal says what isn't so.
        let other_length = own == Some(read) && list_pages.is_some_and(|list_pages| list_pages != pages);
        if !edited {
            // Only history, about another version than what was read or another length: its steps don't apply to this, whichever way it's noticed, and it
            // goes. Nothing unsaved was there, so nothing says any was kept: the file changed, if a look says so, or else undo can't go back as far.
            if other_version || other_length {
                let replaced = other_version
                    || self
                        .tabs
                        .path_of(id)
                        .and_then(|path| file_stamp(&path).ok())
                        .is_some_and(|now| !now.same_contents(read));
                self.clear_history(id, ClearReason::FileChanged);
                if let Some(tab) = self.tab_mut(id) {
                    tab.file
                        .set_notice(Some(if replaced { FileNotice::Changed } else { FileNotice::HistoryRolledOff }));
                }
            }
            return;
        }
        let Some(tab) = self.tabs.tab(id) else { return };
        let heard_changed = matches!(tab.file.notice(), Some(FileNotice::Changed)) || tab.file.elsewhere().is_some_and(|elsewhere| elsewhere.file_changed);
        let length_differs = list_pages.is_some_and(|list_pages| list_pages != pages);
        let elsewhere = if other_version || own.is_none() {
            // Another version than the edits' own, or edits that name none: what's held is of this one.
            Some((own, read, Some(pages), heard_changed || other_version, length_differs))
        } else if other_length {
            // Replaced mid-read, the stamp is still the version the edits were made on, and a stat, in this one rare case, says so. Otherwise the stamp is the
            // file's as it is, which the edits aren't about, and it goes. The file's said to have changed only where that's known.
            match self
                .tabs
                .path_of(id)
                .and_then(|path| file_stamp(&path).ok())
                .filter(|now| !now.same_contents(read))
            {
                Some(now) => Some((own, now, None, true, true)),
                None => Some((None, read, Some(pages), heard_changed, true)),
            }
        } else {
            None
        };
        let Some(tab) = self.tab_mut(id) else { return };
        match elsewhere {
            Some((stamp, seen, seen_pages, file_changed, other_length)) => {
                tab.file.stamp = stamp;
                tab.file.set_elsewhere(Some(fit::Elsewhere {
                    seen: Some(seen),
                    seen_pages,
                    file_changed,
                    other_length,
                }));
                if file_changed {
                    tab.file.set_notice(Some(FileNotice::Changed));
                }
            }
            // The version the edits were made on, at their length, as when it's been put back since a look: they fit it.
            None if tab.file.elsewhere().is_some() => {
                tab.file.set_elsewhere(None);
                if matches!(tab.file.notice(), Some(FileNotice::Changed)) {
                    tab.file.set_notice(None);
                }
            }
            None => {}
        }
    }

    pub(crate) fn remember_warm(&mut self, id: TabId, toc: Arc<Toc>) {
        if let Some(tab) = self.tabs.tab_mut(id) {
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
