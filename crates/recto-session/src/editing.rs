//! Editing: page edits, undo and redo, and what the edits say of the file.

use super::*;

/// **What a caller's page edit may do to a tab's list: move, turn or delete pages, add or take away a highlight, and nothing else** (R3.5b). The closure handed to
/// [`Session::edit_pages_as`] gets this, not `&mut PageEdits`, so it can't put a new list in the tab's place, which would take the owner's edits past
/// the close and discard rules and drop the author marking and journaling set on the list it replaced, and can't add another PDF's pages, which go only
/// through [`Session::insert_pdf_with_facts`] and its check that a quiet run's file is in its folders. These are what the executor's commands do.
impl crate::parts::PartMut<'_, PageEdits> {
    /// [`PageEdits::rotate`].
    pub fn rotate(&mut self, start: u32, len: u32, quarter_turns: i32) -> Result<(), EditError> {
        self.0.rotate(start, len, quarter_turns)
    }

    /// [`PageEdits::move_pages`].
    pub fn move_pages(&mut self, start: u32, len: u32, to: u32) -> Result<(), EditError> {
        self.0.move_pages(start, len, to)
    }

    /// [`PageEdits::delete`].
    pub fn delete(&mut self, start: u32, len: u32) -> Result<(), EditError> {
        self.0.delete(start, len)
    }

    /// [`PageEdits::mark`]: a highlight added, or one made as an edit taken away.
    pub fn mark(&mut self, change: crate::edit::marks::MarkChange) -> Result<(), EditError> {
        self.0.mark(change)
    }

    /// How many pages the tab has now, for an edit of all of them. Never none, as a document can't lose its last page, so there's no `is_empty`.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> u32 {
        self.0.len()
    }
}

impl<L: DocLoader> Session<L> {
    /// Applies the owner's page edit: rearranging, rotating or deleting. This is what the app's own edits call, with nothing to say whose they are. Everything from the agent interface goes through [`Self::edit_pages_as`] with [`Author::Agent`].
    pub fn edit_pages(
        &mut self,
        id: TabId,
        change: impl FnOnce(&mut crate::parts::PartMut<'_, PageEdits>) -> Result<(), EditError>,
    ) -> Result<(), SessionError> {
        self.edit_pages_as(id, Author::Owner, change)
    }

    /// Applies a page edit, noting who made it. The list is made the first time a tab is edited, and goes again if that edit was refused. The reader's place follows its page.
    pub fn edit_pages_as(
        &mut self,
        id: TabId,
        by: Author,
        change: impl FnOnce(&mut crate::parts::PartMut<'_, PageEdits>) -> Result<(), EditError>,
    ) -> Result<(), SessionError> {
        if self.saves.is_saving(id) {
            return Err(SessionError::Saving);
        }
        if !self.sleeper().wake_list(id) {
            return Err(SessionError::EditsUnreadable);
        }
        self.sleeper().touch(id);
        let tab = self.tabs.tab(id).ok_or(SessionError::NoSuchTab)?;
        if tab.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        let page_count = tab.page_count;
        // Its list is 8 bytes a page, so a file or a record claiming billions has to be stopped here.
        if page_count as usize > edit::MAX_PAGES && !self.lists.contains(id) {
            return Err(SessionError::Edit(EditError::TooManyPages));
        }
        let place = self.place_of(id);
        let journaling = self.journals.journaling();
        let may_push_owner = self.lists.may_push_owner(by);
        let edits = self.lists.get_or_new(id, page_count);
        if journaling {
            edits.journal_from_now();
        }
        let result = change(&mut crate::parts::PartMut(&mut edits.acting(by, may_push_owner)));
        if result.is_ok() {
            self.after_change(id, place);
        } else {
            self.drop_list_if_idle(id);
        }
        result.map_err(SessionError::Edit)
    }

    /// What the reader is looking at: which file, which page there, and where it is now.
    pub(crate) fn place_of(&self, id: TabId) -> Before {
        let source_of = |position: u32| match self.lists.get(id) {
            Some(edits) => edits.slots().get(position as usize).map(|slot| (slot.source, slot.page)),
            None => Some((0, position)),
        };
        let place = self
            .tabs
            .tab(id)
            .and_then(|tab| source_of(tab.page).map(|(source, page)| (source, page, tab.page)));
        // Where each stop of the trail's page comes from, so it can follow it through the edit.
        let stops = self.tabs.trail_of(id).map(|trail| {
            trail
                .back()
                .iter()
                .chain(trail.forward())
                .map(|stop| (stop.page, source_of(stop.page)))
                .collect()
        });
        Before {
            place,
            stops,
            fit: self.notices().fit(id),
        }
    }

    /// Everything a change to the page order means: a drawn page's number no longer names that page, and the reader stays on what they were reading.
    pub(crate) fn after_change(&mut self, id: TabId, before: Before) {
        let Before { place, stops, fit } = before;
        // Edits that didn't fit their file, undone or discarded back to nothing unsaved: what's left is history about another version, which redo mustn't bring
        // back, so it goes, and the tab is the file as last read, said once, as only history is when its file's found changed (`compare_read_with_edits`).
        // Not for edits whose version is only unknown: their history is about the file they read, whichever version that was.
        if !fit.fits() && fit != EditsFit::VersionUnknown && !self.lists.is_edited(id) {
            self.let_go_of_list(id);
            let own = self.tabs.tab(id).and_then(|tab| tab.file.stamp);
            let now = self.tabs.path_of(id).and_then(|path| file_stamp(&path).ok());
            let replaced = fit != EditsFit::NotThisFile || own.zip(now).is_some_and(|(own, now)| !own.same_contents(now));
            let elsewhere = self.tabs.tab(id).and_then(|tab| tab.file.elsewhere());
            let seen = elsewhere.and_then(|elsewhere| elsewhere.seen).or(now);
            if replaced && seen != own {
                self.journal_desk().retire_sleep_file(id);
            }
            match elsewhere.and_then(|elsewhere| elsewhere.seen_pages) {
                Some(pages) => {
                    if let Some(tab) = self.tabs.tab_mut(id) {
                        tab.page_count = pages;
                    }
                }
                // Looked at and not read yet: what's held isn't of it, and it's read when it's next shown.
                None => self.sleeper().forget_read(id),
            }
            if let Some(tab) = self.tabs.tab_mut(id) {
                tab.file.take_version(seen);
                // A file gone, a password asked for or another tab's edits kept elsewhere is still so, and said over this.
                if !matches!(
                    tab.file.notice(),
                    Some(FileNotice::Missing | FileNotice::NeedsPassword | FileNotice::EditsNotMerged)
                ) {
                    tab.file
                        .set_notice(Some(if replaced { FileNotice::Changed } else { FileNotice::HistoryRolledOff }));
                }
            }
        }
        // The trail's stops go where their pages went, and a stop whose page went with the edit goes too.
        if let Some(stops) = stops {
            let pages = self.pages().count(id);
            let now: HashMap<u32, Option<u32>> = stops
                .into_iter()
                .map(|(was, source)| {
                    let found = source.and_then(|(source, page)| match self.lists.get(id) {
                        Some(edits) => edits
                            .slots()
                            .iter()
                            .position(|slot| slot.source == source && slot.page == page)
                            .and_then(|at| u32::try_from(at).ok()),
                        None => (source == 0 && page < pages).then_some(page),
                    });
                    (was, found)
                })
                .collect();
            if let Some(trail) = self.tabs.tab_mut(id).and_then(|tab| tab.reading.trail.as_deref_mut()) {
                trail.follow_pages(|page| now.get(&page).copied().flatten());
            }
        }
        self.mail.forget_draws(id);
        self.drop_list_if_idle(id);
        let pages = self.pages().count(id);
        let found = place.and_then(|(source, page, near)| match self.lists.get(id) {
            Some(edits) => edits.nearest_position(source, page, near),
            None => (source == 0 && page < pages).then_some(page),
        });
        let near = place.map_or(0, |(_, _, near)| near);
        self.mail.set_place_lost(place.is_some() && found.is_none());
        let last = pages.saturating_sub(1);
        if let Some(tab) = self.tabs.tab_mut(id) {
            tab.page = found.unwrap_or(near).min(last);
            tab.place_dirty = true;
        }
    }

    /// An edit list with nothing in it costs 8 bytes a page for nothing, and would hold the tab to the file's length as it was. One that's back at the file with steps to go back through stays until those are gone.
    fn drop_list_if_idle(&mut self, id: TabId) {
        if self.lists.get(id).is_some_and(|edits| !edits.is_edited() && !edits.has_history()) {
            self.let_go_of_list(id);
        }
    }

    /// A tab's list is done with: its journal goes, and any files kept for it.
    pub(crate) fn let_go_of_list(&mut self, id: TabId) {
        if let Some(list) = self.lists.remove(id) {
            // With one that a save said it kept and that hasn't been confirmed, which nothing else names.
            let kept: Vec<Box<Path>> = list
                .kept_sources()
                .map(|(_, added)| added.path.clone())
                .chain(list.keeping().filter(|kept| kept.kept).map(|kept| kept.path.clone()))
                .collect();
            self.journals.retire_kept(kept);
        }
        self.journals.let_go_of_fresh(id);
        self.journals.retire(id);
    }

    /// A tab's list whole, wherever it is, for QA and a look: one on disk is read for this and stays there.
    pub fn list_record_anywhere(&self, id: TabId) -> Option<record::EditsRecord> {
        self.lists.edits_record(id).or_else(|| {
            self.lists.is_asleep(id).then_some(())?;
            Some(self.journals.history()?.read_list(self.journals.name_of(id)?)?.edits.to_record())
        })
    }

    /// Adds every page of another PDF at a position, as dropping it into the page view does. Only its facts are read.
    pub fn insert_pdf(&mut self, id: TabId, at: u32, path: &Path) -> Result<(), SessionError> {
        let to_read = self.tabs.confinement().to_read(path).ok_or(SessionError::OutsideQuietRun)?;
        let stamp = file_stamp(&to_read).ok();
        let facts = self.loader.load(&to_read).map_err(SessionError::Open)?;
        self.insert_pdf_with_facts(id, Author::Owner, at, path, &facts, stamp)
    }

    /// Adds another PDF's pages from facts already read, for a caller that reads the file without holding the session. A file only the renderer can read can't be added: Save couldn't write it.
    pub fn insert_pdf_with_facts(
        &mut self,
        id: TabId,
        by: Author,
        at: u32,
        path: &Path,
        facts: &DocFacts,
        stamp: Option<FileStamp>,
    ) -> Result<(), SessionError> {
        if !self.tabs.confinement().admits(path) {
            return Err(SessionError::OutsideQuietRun);
        }
        if facts.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        if facts.page_count == 0 {
            return Err(SessionError::Open(OpenError::NotPdf));
        }
        // Not protected, encrypted or not: a file that needs a password can't be added at all (the loader that read `facts` is given none, and refuses
        // it), so what's added opens for anyone, and its pages are no more private than the file (2026-11-13; before, an encrypted one cost the document
        // its sleep file's pictures). A journal written before keeps its flag, and its document has no pictures till those pages are gone.
        self.edit_pages_as(id, by, |edits| edits.0.insert_pdf_noting(at, path, facts.page_count, stamp, false))
    }

    /// Where a page of a tab comes from now, counting unsaved edits. Zero-based.
    pub fn page_source(&self, id: TabId, position: u32) -> Option<PageSource> {
        let tab = self.tabs.tab(id)?;
        if self.lists.is_asleep(id) {
            return None;
        }
        let Some(edits) = self.lists.get(id) else {
            return (position < tab.page_count)
                .then(|| {
                    Some(PageSource {
                        path: self.tabs.path_of(id)?,
                        page: position,
                        quarter_turns: 0,
                        added: false,
                        expected: None,
                    })
                })
                .flatten();
        };
        let slot = *edits.slots().get(position as usize)?;
        match slot.source {
            0 => Some(PageSource {
                path: self.tabs.path_of(id)?,
                page: slot.page,
                quarter_turns: slot.quarter_turns,
                added: false,
                // Edits that don't fit the file name no version of it that can be current.
                expected: Some(tab.file.stamp.filter(|_| self.notices().fit(id).fits())),
            }),
            source => {
                let added = edits.added_sources().get(usize::from(source) - 1)?;
                Some(PageSource {
                    path: added.path.to_path_buf(),
                    page: slot.page,
                    quarter_turns: slot.quarter_turns,
                    added: true,
                    expected: Some(added.stamp),
                })
            }
        }
    }

    /// **The version of its file a save moved this tab on from,** while it's still on the version that save wrote: every page of that is a page that was shown before, as it was shown, in the same place. `None` once anything else has happened to the file.
    pub fn saved_from(&self, id: TabId) -> Option<FileStamp> {
        let now = self.tabs.own_stamp(id)?;
        self.saves.saved_from(id, now)
    }

    /// Reverses the last page edit. Returns false with nothing to undo.
    pub fn undo_edit(&mut self, id: TabId) -> bool {
        if self.saves.is_saving(id) || !self.sleeper().wake_list(id) {
            return false;
        }
        self.sleeper().touch(id);
        let place = self.place_of(id);
        let undone = self.lists.get_mut(id).is_some_and(PageEdits::undo);
        if undone {
            self.after_change(id, place);
        }
        undone
    }

    /// [`Self::undo_edit`] for whoever's asking: an agent's can be refused, if it would push the owner's steps out of the history.
    pub fn undo_edit_as(&mut self, id: TabId, by: Author) -> Result<bool, SessionError> {
        self.step_back_or_on(id, by, PageEdits::try_undo)
    }

    /// Whether a tab's pages can be edited now: it's there, more than the renderer can read its file, and its edits can be read. What greys out the panel's Rotate. Cheap: no list is woken for it.
    pub fn can_edit_pages(&self, id: TabId) -> bool {
        self.tabs
            .tab(id)
            .is_some_and(|tab| !tab.renderer_only && !matches!(tab.file.notice(), Some(FileNotice::EditsUnreadable)))
    }

    /// The panel's Rotate, by the owner. Outside edit mode it turns every page together, as one step that one undo takes back; in edit mode, the page being read. Owner, 2026-11-07: "When not in edit mode rotate page will rotate all pages together (it still enables save to commit the adjustment). Usefull when there is a scan issue or similar, for a quick fix."
    pub fn rotate_from_panel(&mut self, id: TabId, quarter_turns: i32) -> Result<(), SessionError> {
        let page = self.tabs.tab(id).ok_or(SessionError::NoSuchTab)?.page;
        let one_page = self.viewing.edit_mode();
        self.edit_pages_as(id, Author::Owner, |edits| {
            if one_page {
                edits.rotate(page, 1, quarter_turns)
            } else {
                edits.rotate(0, edits.len(), quarter_turns)
            }
        })
    }

    /// [`Self::redo_edit`] for whoever's asking.
    pub fn redo_edit_as(&mut self, id: TabId, by: Author) -> Result<bool, SessionError> {
        self.step_back_or_on(id, by, PageEdits::try_redo)
    }

    fn step_back_or_on(&mut self, id: TabId, by: Author, step: fn(&mut PageEdits) -> Result<bool, EditError>) -> Result<bool, SessionError> {
        if self.saves.is_saving(id) {
            return Err(SessionError::Saving);
        }
        if !self.sleeper().wake_list(id) {
            return Err(SessionError::EditsUnreadable);
        }
        self.sleeper().touch(id);
        let place = self.place_of(id);
        let may_push_owner = self.lists.may_push_owner(by);
        let Some(edits) = self.lists.get_mut(id) else { return Ok(false) };
        let result = step(&mut edits.acting(by, may_push_owner));
        let stepped = result.map_err(SessionError::Edit)?;
        if stepped {
            self.after_change(id, place);
        }
        Ok(stepped)
    }

    /// Repeats the last undone page edit. Returns false with nothing to redo.
    pub fn redo_edit(&mut self, id: TabId) -> bool {
        if self.saves.is_saving(id) || !self.sleeper().wake_list(id) {
            return false;
        }
        self.sleeper().touch(id);
        let place = self.place_of(id);
        let redone = self.lists.get_mut(id).is_some_and(PageEdits::redo);
        if redone {
            self.after_change(id, place);
        }
        redone
    }

    /// Throws away a tab's unsaved page edits, and its history with them.
    pub fn discard_edits(&mut self, id: TabId) {
        self.clear_history(id, ClearReason::Discard);
    }
}
