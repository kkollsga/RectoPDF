//! Contents: the sidebar model, generated contents, folds and the verdict.

use super::*;

impl<L: DocLoader> Session<L> {
    /// Builds the sidebar. Cost depends on the visible rows and the active tab's contents, never on idle tabs' PDFs.
    pub fn sidebar(&self) -> SidebarModel<'_> {
        let mut rows = Vec::with_capacity(self.tabs.len());
        let mut rail_filled = 0;
        let mut place = String::new();
        let mut toc_open = false;
        for tab in self.tabs.all() {
            let is_active = self.tabs.active() == Some(tab.id);
            rows.push(SidebarRow {
                kind: RowKind::Tab,
                tab: tab.id,
                text: &tab.name,
                page: None,
                depth: 0,
                current: is_active,
                notice: tab_notice_words(tab, self.lists.has_list(tab.id)),
                action: None,
                entry: NONE,
                expandable: false,
                expanded: false,
            });
            if !is_active {
                continue;
            }
            toc_open = tab.toc_open;
            place = self.place_words(tab.id, tab.page);
            if tab.toc_open
                && let Some(toc) = self.warm.toc(tab.id)
            {
                let view = self.generated_view(tab.id);
                let note = contents_note(tab.contents_verdict, view, tab.generated_contents, toc.entries().len());
                if let Some((text, hover, action)) = note {
                    rows.push(SidebarRow {
                        kind: RowKind::Note,
                        tab: tab.id,
                        text,
                        page: None,
                        depth: 0,
                        current: false,
                        notice: Some(hover),
                        action,
                        entry: NONE,
                        expandable: false,
                        expanded: false,
                    });
                }
                let shown = match view {
                    Some(GeneratedView::Ready(generated)) if tab.generated_contents => generated,
                    _ => toc,
                };
                // The note isn't an entry: the host draws it over the contents, and the rail counts entries.
                rail_filled = contents_rows(shown, tab, self.lists.get(tab.id), &mut rows);
            }
        }
        let edited = self.tabs.active().is_some_and(|id| self.lists.is_edited(id));
        let undone_owner_steps = self.tabs.active().is_some_and(|id| self.lists.has_undone_owner_steps(id));
        SidebarModel {
            rows,
            rail_filled,
            toc_open,
            place,
            edited,
            undone_owner_steps,
        }
    }

    /// Where this tab's generated contents stand, if anyone's asked.
    pub fn generated_view(&self, id: TabId) -> Option<&GeneratedView> {
        self.generated.view(id)
    }

    /// Notes where a tab's generated contents stand. The oldest note gives way past [`MAX_GENERATED_VIEWS`], but never the active tab's.
    pub fn set_generated_view(&mut self, id: TabId, view: GeneratedView) {
        if self.tabs.tab(id).is_none() {
            return;
        }
        self.generated.set(id, view, self.tabs.active());
    }

    /// Forgets where a tab's generated contents stand, so it offers them again.
    pub fn forget_generated(&mut self, id: TabId) {
        self.generated.forget(id);
    }

    /// The same, from a look at the file as it was at `version`: dropped if the tab's file has changed since, as a look that comes back late is about the old one.
    pub fn set_generated_view_for(&mut self, id: TabId, view: GeneratedView, version: Option<FileStamp>) {
        if version.is_none() || self.tabs.own_stamp(id) == version {
            self.set_generated_view(id, view);
        }
    }

    /// Tabs whose generated contents are being made.
    pub fn generated_making(&self) -> Vec<TabId> {
        self.generated.making()
    }

    /// Shows a tab's generated contents, or its own. The choice is kept in the record, and the file is never changed. What's folded goes, as it was about the other list's entries.
    pub fn choose_generated(&mut self, id: TabId, generated: bool) {
        if let Some(tab) = self.tabs.tab_mut(id)
            && tab.generated_contents != generated
        {
            tab.generated_contents = generated;
            tab.folds = None;
        }
    }

    /// Opens a contents section that's closed, or closes one that's open, as its chevron does. Owner, 2026-11-07: "there should be a collapse toggle button for sections with sub chapters on the left side of the headers". What following the place opens is the default, and only what differs from it is kept, at most [`record::MAX_CONTENTS_FOLDS`], in the record.
    ///
    /// Says whether the section's open now, or `None` if it isn't one: the entry has nothing under it, or the tab's contents aren't read.
    pub fn toggle_contents_section(&mut self, id: TabId, entry: u32) -> Option<bool> {
        let toc = self.shown_contents(id)?;
        let tab = self.tabs.tab(id)?;
        let chain = following_chain(&toc, tab, self.lists.get(id));
        let has_children = toc.entries().get(entry as usize + 1).is_some_and(|next| next.parent == entry);
        if !has_children {
            return None;
        }
        let tab = self.tabs.tab_mut(id)?;
        let open_now = section_open(tab.folds.as_deref(), &chain, entry);
        let mut folds: Vec<(u32, bool)> = tab.folds.take().map(Vec::from).unwrap_or_default();
        folds.retain(|(at, _)| *at != entry);
        // Back to what following does is no fold at all.
        if chain.contains(&entry) == open_now {
            folds.push((entry, !open_now));
        }
        let over = folds.len().saturating_sub(record::MAX_CONTENTS_FOLDS);
        folds.drain(..over);
        tab.folds = (!folds.is_empty()).then(|| folds.into_boxed_slice());
        tab.place_dirty = true;
        Some(!open_now)
    }

    /// The contents the sidebar shows for a tab: its generated ones if they're chosen and ready, or its own.
    fn shown_contents(&self, id: TabId) -> Option<Arc<Toc>> {
        let own = self.warm.toc(id)?;
        Some(match (self.tabs.tab(id).is_some_and(|tab| tab.generated_contents), self.generated_view(id)) {
            (true, Some(GeneratedView::Ready(generated))) => Arc::clone(generated),
            _ => Arc::clone(own),
        })
    }

    /// What's thought of a tab's own contents, once they've been read.
    pub fn contents_verdict(&self, id: TabId) -> Option<ContentsVerdict> {
        let known = self.warm.contains(id);
        self.tabs.tab(id).filter(|_| known).map(|tab| tab.contents_verdict)
    }
}

/// Adds the visible contents rows: every top-level entry, plus the chapter being read opened down to the current
/// section. Returns how many rows the rail fills.
///
/// With unsaved page edits, entries follow their pages to their new positions. An entry whose page was deleted stays
/// without a number, and pages added from another PDF belong to no section.
fn contents_rows<'a>(toc: &'a Toc, tab: &TabRecord, edits: Option<&PageEdits>, rows: &mut Vec<SidebarRow<'a>>) -> usize {
    let here = section_here(toc, tab, edits);
    let chain = following_chain(toc, tab, edits);
    let folds = tab.folds.as_deref();
    // Whether each entry's own children show: it's shown, and open. Entries come in document order, so a parent is always decided first.
    let mut children_show = vec![false; toc.entries().len()];
    let first_row = rows.len();
    let mut filled = 0;
    for (index, entry) in toc.entries().iter().enumerate() {
        if entry.parent != NONE && !children_show.get(entry.parent as usize).copied().unwrap_or(false) {
            continue;
        }
        let at = u32::try_from(index).unwrap_or(NONE);
        let expandable = toc.entries().get(index + 1).is_some_and(|next| next.parent == at);
        let expanded = expandable && section_open(folds, &chain, at);
        children_show[index] = expanded;
        let current = here == Some(index);
        rows.push(SidebarRow {
            kind: RowKind::Contents,
            tab: tab.id,
            text: toc.title(entry),
            page: (entry.page != NONE)
                .then(|| edits.map_or(Some(entry.page), |e| e.position_of(entry.page)))
                .flatten()
                .map(|page| page + 1),
            depth: entry.depth,
            current,
            notice: None,
            action: None,
            entry: at,
            expandable,
            expanded,
        });
        if current {
            filled = rows.len() - first_row;
        }
    }
    filled
}

/// The entry for the section being read, in the file's own page numbers.
fn section_here(toc: &Toc, tab: &TabRecord, edits: Option<&PageEdits>) -> Option<usize> {
    let original_page = match edits {
        Some(edits) => edits.slots().get(tab.page as usize).filter(|s| s.source == 0).map(|s| s.page),
        None => Some(tab.page),
    };
    original_page.and_then(|page| toc.section_at(page))
}

/// The section being read and every section it's in, which following the place keeps open.
fn following_chain(toc: &Toc, tab: &TabRecord, edits: Option<&PageEdits>) -> HashSet<u32> {
    let mut open = HashSet::new();
    let mut cursor = section_here(toc, tab, edits).map_or(NONE, |i| u32::try_from(i).unwrap_or(NONE));
    while cursor != NONE && open.insert(cursor) {
        cursor = toc.entries().get(cursor as usize).map_or(NONE, |e| e.parent);
    }
    open
}

/// Whether a section's children show: as the reader folded it, or else as following the place has it.
fn section_open(folds: Option<&[(u32, bool)]>, chain: &HashSet<u32>, entry: u32) -> bool {
    folds
        .and_then(|folds| folds.iter().rev().find(|(at, _)| *at == entry))
        .map_or_else(|| chain.contains(&entry), |(_, open)| *open)
}
