//! Saving: the set of tabs being saved, and what a save leaves of the history.

use super::*;

impl<L: DocLoader> Session<L> {
    /// **The one method a confirmed Save or Save As calls, and it marks a saved point.** Owner, 2026-11-07: "after a save you might still have regrets and want to make changes. But you definitely don't want history to leak into the pdf file". What was written is the file now, so the document reads as saved, and the history goes on: undoing past this makes it unsaved again, and back to here reads as saved. A failed save calls nothing, and everything stays. `saved_as` moves the tab if it went to another file, and the history goes with it. `kept` is what [`Self::save_prepared`] was told, and a Save As has the old file itself for that. What a step can't get pages from rolls off: see [`PageEdits::rebase_on_saved`], and the tab says so. The steps around it, and what a crash at each does, are in `docs/engineering/state-record.md`. Says how many steps rolled off.
    pub fn saved(&mut self, id: TabId, now: FileStamp, pages: u32, saved_as: Option<&Path>, kept: Option<edit::AddedSource>) -> usize {
        // A save of the session a restore replaced says nothing of the tab that has its number now.
        if self.saves.doubted(id) {
            return 0;
        }
        // Another version of the file now: the old one's generated contents and folds aren't about it.
        self.generated.forget(id);
        if let Some(tab) = self.tab_mut(id) {
            tab.folds = None;
        }
        if !self.wake_list(id) {
            self.give_up_on_asleep(id);
        }
        let kept = kept.or_else(|| {
            saved_as?;
            let tab = self.tab(id)?;
            Some(edit::AddedSource {
                path: self.path_of(id)?.into_boxed_path(),
                page_count: tab.page_count,
                stamp: tab.file.stamp,
                kept: false,
                protected: tab.file.encrypted,
                holds: None,
            })
        });
        self.retire_sleep_file(id);
        let rebased = self.lists.get_mut(id).map(|list| list.rebase_on_saved(pages, kept.clone()));
        let mut rolled_off = 0;
        match rebased {
            // There's no list, so no history. A file kept for it isn't needed.
            None => self.retire_kept(kept.filter(|kept| kept.kept).map(|kept| kept.path).into_iter().collect()),
            // Something else was written, so nothing in the history's about that file.
            Some(None) => {
                rolled_off = self.lists.get(id).map_or(0, |list| list.summary().undo as usize + list.summary().redo as usize);
                self.retire_kept(kept.filter(|kept| kept.kept).map(|kept| kept.path).into_iter().collect());
                self.let_go_of_list(id);
            }
            Some(Some(rebased)) => {
                rolled_off = rebased.rolled_off;
                self.retire_kept(rebased.let_go);
            }
        }
        if let Some(path) = saved_as {
            self.relocate(id, path);
        }
        // What's in the new version is what was shown, page for page, so the page view goes on from where it was.
        let before = self.tab(id).and_then(|tab| tab.file.stamp);
        self.saves.saved_over(id, before, now);
        if let Some(tab) = self.tab_mut(id) {
            tab.file.take_version(Some(now));
            tab.page_count = pages;
            tab.page = tab.page.min(pages.saturating_sub(1));
            tab.file.set_notice((rolled_off > 0).then_some(FileNotice::HistoryRolledOff));
            tab.problem = None;
            tab.place_dirty = true;
        }
        self.mail.forget_draws(id);
        if self.lists.get(id).is_some_and(|list| !list.has_history()) {
            self.let_go_of_list(id);
        } else if self.lists.contains(id) {
            // A journal goes by its file's identity, which this has changed. The old one goes only once the new one's there, so a crash in between has one of them.
            let replaces = self.journals.forget_name(id).map(String::from);
            self.journals.drop_fresh(id);
            if self.journals.journaling() {
                let seq = self.lists.get(id).map_or(0, PageEdits::journal_seq);
                self.journal_name_for(id);
                self.journals.start_fresh(id, seq, replaces);
            } else if let Some(old) = replaces {
                self.journals.retire_name(old.into());
            }
        }
        rolled_off
    }

    /// **Starts a save of this tab:** what's to be written, copied out, and the tab held so that nothing changes its edits until [`Self::end_save`]. `None` if there's nothing unsaved. Refused while another save of it runs, for a file only the renderer can read, and for edits that can't be read back.
    pub fn begin_save(&mut self, id: TabId, even_unedited: bool) -> Result<Option<SaveSnapshot>, SessionError> {
        if self.is_saving(id) || self.saves.must_wait(id) {
            return Err(SessionError::Saving);
        }
        let tab = self.tab(id).ok_or(SessionError::NoSuchTab)?;
        if tab.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        if !self.wake_list(id) {
            return Err(SessionError::EditsUnreadable);
        }
        // Edits that don't fit the file name no version of it to save onto.
        let fits = self.notices().fit(id).fits();
        let tab = self.tab(id).ok_or(SessionError::NoSuchTab)?;
        let path = self.path_of(id).ok_or(SessionError::NoSuchTab)?;
        let snapshot = match self.lists.get(id).filter(|list| list.is_edited()) {
            Some(list) => SaveSnapshot {
                path,
                stamp: tab.file.stamp.filter(|_| fits),
                file_pages: list.file_pages(),
                slots: list.slots().to_vec(),
                added: list.added_sources().to_vec(),
                holds_owner_edits: list.holds_owner_edits(),
                needs_own_pages: list.needs_own_pages(),
            },
            // A Save As of the document as it is: every page in its place.
            None if even_unedited => {
                let pages = tab.page_count;
                SaveSnapshot {
                    path,
                    stamp: tab.file.stamp,
                    file_pages: pages,
                    slots: (0..pages)
                        .map(|page| edit::PageSlot {
                            source: 0,
                            page,
                            quarter_turns: 0,
                        })
                        .collect(),
                    added: Vec::new(),
                    holds_owner_edits: false,
                    needs_own_pages: false,
                }
            }
            None => return Ok(None),
        };
        self.saves.begin(id);
        self.hold(id);
        Ok(Some(snapshot))
    }

    /// A save of this tab has finished, however it went. One that didn't put a file in place calls this without [`Self::saved`], and what [`Self::save_prepared`] noted goes: the edits stand, as they were.
    pub fn end_save(&mut self, id: TabId) {
        if self.saves.end_abandoned(id) {
            return;
        }
        if self.saves.end(id) {
            self.let_go(id);
            if let Some(list) = self.lists.get_mut(id)
                && list.saving().is_some()
            {
                let kept = list.save_abandoned();
                self.retire_kept(kept.into_iter().collect());
            }
        }
    }

    /// **The close a prompt's Save asked for, once the save is done.** If an edit or an undo came in since, the file doesn't have it, so the tab stays open and this says `None`: nothing is lost unasked. Otherwise it closes, letting go of undone steps, as the prompt said they would go.
    pub fn close_after_save(&mut self, id: TabId) -> Result<Option<Closed>, SessionError> {
        if self.is_edited(id) {
            return Ok(None);
        }
        if self.has_undone_owner_steps(id) {
            self.discard_edits(id);
        }
        self.close(id).map(Some)
    }

    /// Whether a save of this tab is under way.
    pub fn is_saving(&self, id: TabId) -> bool {
        self.saves.is_saving(id)
    }

    /// Whether any save is under way, which a hand-off waits for.
    pub fn any_saving(&self) -> bool {
        self.saves.any()
    }

    /// The first step of a save, before anything's put in place: notes what the file will be if it works, and what's been kept for undoing past it, so that a crash between that and [`Self::saved`] can tell. Goes in the journal. **A save that removes something for good, as a redaction does, keeps nothing:** a copy of the file would hold what it's removing.
    pub fn save_prepared(&mut self, id: TabId, target: FileStamp, keeping: Option<&edit::AddedSource>) {
        if self.saves.doubted(id) {
            return;
        }
        self.wake_list(id);
        if let Some(edits) = self.lists.get_mut(id) {
            edits.note_saving(target, keeping);
        }
    }

    /// **Clears a tab's history, and removes its files rather than only forgetting them.** With unsaved edits, the page order stays, as only the way back goes, unless it's a discard, which is all of it. With none, there's nothing left to hold, so the list goes too. Says whether there was any.
    pub fn clear_history(&mut self, id: TabId, reason: ClearReason) -> bool {
        // Nothing's taken from under a save: what it writes is what it was given, and its `saved` rebases this list.
        if self.is_saving(id) {
            return false;
        }
        if !self.wake_list(id) {
            self.give_up_on_asleep(id);
            if let Some(tab) = self.tab_mut(id) {
                tab.file.set_notice(None);
            }
            return true;
        }
        // What's gone mustn't stay in a picture of it.
        if matches!(reason, ClearReason::Redaction | ClearReason::HiddenDataRemoval) {
            self.retire_sleep_file(id);
        }
        let Some(list) = self.lists.get_mut(id) else { return false };
        if reason != ClearReason::Discard && list.is_edited() {
            let unconfirmed = list.keeping().filter(|kept| kept.kept).map(|kept| kept.path.clone());
            let let_go: Vec<Box<Path>> = list.drop_history().into_iter().chain(unconfirmed).collect();
            let seq = list.journal_seq();
            let journaled = self.journals.journaling();
            self.retire_kept(let_go);
            if journaled {
                // The same name as it has now, as the file's the same, replaced in one step.
                self.journals.start_fresh(id, seq, None);
            }
            return true;
        }
        let place = self.place_of(id);
        self.let_go_of_list(id);
        if let Some(tab) = self.tab_mut(id)
            && matches!(
                tab.file.notice(),
                Some(FileNotice::Changed | FileNotice::EditsUnreadable | FileNotice::HistoryRolledOff)
            )
        {
            tab.file.set_notice(None);
        }
        self.after_change(id, place);
        true
    }
}
