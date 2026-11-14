//! Contents: the sidebar model, generated contents, folds and the verdict.

use super::*;
use crate::lists::Order;
use recto_core::thousandths;

impl<L: DocLoader> Session<L> {
    /// Builds the sidebar. Cost depends on the visible rows and the active tab's contents, never on idle tabs' PDFs.
    pub fn sidebar(&self) -> SidebarModel<'_> {
        let mut rows = Vec::with_capacity(self.tabs.len());
        let mut rail_filled = 0;
        let mut place = String::new();
        let mut toc_open = false;
        let notices = self.notices();
        for tab in self.tabs.all() {
            let is_active = self.tabs.active() == Some(tab.id);
            rows.push(SidebarRow {
                kind: RowKind::Tab,
                tab: tab.id,
                text: &tab.name,
                page: None,
                depth: 0,
                current: is_active,
                notice: notices.said_of(tab).map(|said| said.message),
                action: None,
                entry: NONE,
                top: 0,
                expandable: false,
                expanded: false,
            });
            if !is_active {
                continue;
            }
            toc_open = tab.toc_open;
            place = self.pages().place_words(tab.id, tab.page);
            if tab.toc_open
                && let Some(toc) = self.warm.toc(tab.id)
            {
                let view = self.generated.view(tab.id);
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
                        top: 0,
                        expandable: false,
                        expanded: false,
                    });
                }
                let shown = match view {
                    Some(GeneratedView::Ready(generated)) if tab.generated_contents => generated,
                    _ => toc,
                };
                // The note isn't an entry: the host draws it over the contents, and the rail counts entries.
                rail_filled = contents_rows(shown, tab, self.lists.order(tab.id), reading_line(self, tab), &mut rows);
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

    /// Notes where a tab's generated contents stand. The oldest note gives way past [`MAX_GENERATED_VIEWS`], but never the active tab's.
    pub fn set_generated_view(&mut self, id: TabId, view: GeneratedView) {
        if self.tabs.tab(id).is_none() {
            return;
        }
        self.generated.set(id, view, self.tabs.active());
    }

    /// The same, from a look at the file as it was at `version`: dropped if the tab's file has changed since, as a look that comes back late is about the old one.
    pub fn set_generated_view_for(&mut self, id: TabId, view: GeneratedView, version: Option<FileStamp>) {
        if version.is_none() || self.tabs.own_stamp(id) == version {
            self.set_generated_view(id, view);
        }
    }

    /// **Generated contents stopped:** the tab shows its file's own, and where generated ones stood is forgotten, so the note offers them again. One
    /// operation, as the tab's choice and the generated contents it's about change together.
    pub fn stop_generated_contents(&mut self, id: TabId) {
        self.tabs.choose_generated(id, false);
        self.generated.forget(id);
    }

    /// Opens a contents section that's closed, or closes one that's open, as its chevron does. Owner, 2026-11-07: "there should be a collapse toggle button for sections with sub chapters on the left side of the headers". What following the place opens is the default, and only what differs from it is kept, at most [`record::MAX_CONTENTS_FOLDS`], in the record.
    ///
    /// Says whether the section's open now, or `None` if it isn't one: the entry has nothing under it, or the tab's contents aren't read.
    pub fn toggle_contents_section(&mut self, id: TabId, entry: u32) -> Option<bool> {
        let toc = self.shown_contents(id)?;
        let tab = self.tabs.tab(id)?;
        let order = self.lists.order(id);
        let chain = following_chain(&toc, order, reading_line(self, tab), &Placed::of(tab, order));
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
        shown_contents_of(self, self.tabs.tab(id)?)
    }

    /// What's thought of a tab's own contents, once they've been read.
    pub fn contents_verdict(&self, id: TabId) -> Option<ContentsVerdict> {
        let known = self.warm.contains(id);
        self.tabs.tab(id).filter(|_| known).map(|tab| tab.contents_verdict)
    }
}

/// The contents the sidebar shows for a tab in hand: its generated ones if they're chosen and ready, or its own. A tab in hand, so a walk over every tab
/// needn't look each up again.
fn shown_contents_of<L: DocLoader>(session: &Session<L>, tab: &TabRecord) -> Option<Arc<Toc>> {
    let own = session.warm.toc(tab.id)?;
    Some(match (tab.generated_contents, session.generated.view(tab.id)) {
        (true, Some(GeneratedView::Ready(generated))) => Arc::clone(generated),
        _ => Arc::clone(own),
    })
}

/// **The line the contents' current entry is found by:** the page at the top of the reader's view, as the session counts pages, and how far down it, in
/// thousandths. An entry is current once its top is at or above this, so a heading becomes current as it reaches the top of the view, and an entry jumped to
/// (whose top the jump puts on the view's top) stays current. It is where a jump the page view hasn't taken yet is going, else the place the page view last
/// told of for the page the tab is on (its anchor, which is the top of the view and may be the page before the one the page number says, at the foot of it),
/// else the top of the tab's page, as before the view has said.
fn reading_line<L: DocLoader>(session: &Session<L>, tab: &TabRecord) -> (u32, u16) {
    if let Some(jump) = crate::trail::jump_here(session, tab) {
        return (jump.page, thousandths(f64::from(jump.within)));
    }
    tab.reading
        .place
        .filter(|place| place.page == tab.page)
        .map_or((tab.page, 0), |place| (place.anchor_page, thousandths(place.within)))
}

/// Which pages of the file have entries whose places mean something: not those with an unsaved turn (or any page, with the view turned), whose frame isn't the
/// file's, where every entry reads as at its page's top.
struct Placed {
    all_turned: bool,
    turned: HashSet<u32>,
}

impl Placed {
    fn of(tab: &TabRecord, order: Order<'_>) -> Self {
        let turned = match order {
            Order::Edited(edits) => edits
                .slots()
                .iter()
                .filter(|slot| slot.source == 0 && slot.quarter_turns != 0)
                .map(|slot| slot.page)
                .collect(),
            _ => HashSet::new(),
        };
        Self {
            all_turned: tab.view_rotation != 0,
            turned,
        }
    }

    fn page(&self, page: u32) -> bool {
        !self.all_turned && !self.turned.contains(&page)
    }
}

/// **Where the entry being read changes,** for the tab being read: the stretch of the page at the top of the view in which the same entry stays current, in
/// the page's thousandths, for the page view to tell the core when the top of the view leaves it, and no oftener. `None` where nothing can be said: no
/// contents shown, or the page isn't the file's own. The page is the session's (edits counted), as the page view counts.
#[must_use]
pub fn reading_band<L: DocLoader>(session: &Session<L>, tab: &TabRecord) -> Option<recto_core::Band> {
    if !tab.toc_open {
        return None;
    }
    let toc = shown_contents_of(session, tab)?;
    let order = session.lists.order(tab.id);
    let (at_page, top) = reading_line(session, tab);
    let page = original_page(at_page, order)?;
    let placed = Placed::of(tab, order);
    let band = toc.band_at(page, top, |at| placed.page(at));
    Some(recto_core::Band { page: at_page, ..band })
}

/// Adds the visible contents rows: every top-level entry, plus the chapter being read opened down to the current
/// section. Returns how many rows the rail fills.
///
/// With unsaved page edits, entries follow their pages to their new positions. An entry whose page was deleted stays
/// without a number, and pages added from another PDF belong to no section. While the tab's list is asleep, where any entry's page is now isn't known:
/// none has a number, and none is the section being read.
fn contents_rows<'a>(toc: &'a Toc, tab: &TabRecord, order: Order<'_>, line: (u32, u16), rows: &mut Vec<SidebarRow<'a>>) -> usize {
    let placed = Placed::of(tab, order);
    let here = section_here(toc, order, line, &placed);
    let chain = following_chain(toc, order, line, &placed);
    let folds = tab.folds.as_deref();
    // Whether each entry's own children show: it's shown, and open. Entries come in document order, so a parent is always decided first.
    let mut children_show = vec![false; toc.entries().len()];
    let first_row = rows.len();
    // Where each entry's row is, for those shown.
    let mut row_of: Vec<Option<u32>> = vec![None; toc.entries().len()];
    for (index, entry) in toc.entries().iter().enumerate() {
        if entry.parent != NONE && !children_show.get(entry.parent as usize).copied().unwrap_or(false) {
            continue;
        }
        let at = u32::try_from(index).unwrap_or(NONE);
        let expandable = toc.entries().get(index + 1).is_some_and(|next| next.parent == at);
        let expanded = expandable && section_open(folds, &chain, at);
        children_show[index] = expanded;
        row_of[index] = u32::try_from(rows.len()).ok();
        rows.push(SidebarRow {
            kind: RowKind::Contents,
            tab: tab.id,
            text: toc.title(entry),
            page: (entry.page != NONE)
                .then(|| match order {
                    Order::File => Some(entry.page),
                    Order::Edited(edits) => edits.position_of(entry.page),
                    Order::Unknown => None,
                })
                .flatten()
                .map(|page| page + 1),
            depth: entry.depth,
            current: false,
            notice: None,
            action: None,
            entry: at,
            top: if entry.page != NONE && placed.page(entry.page) { entry.top } else { 0 },
            expandable,
            expanded,
        });
    }
    // The entry being read, or when a fold hides it the nearest section above it that shows: the usual outline convention.
    let mut cursor = here.map_or(NONE, |i| u32::try_from(i).unwrap_or(NONE));
    // An entry's parent comes before it in a list this crate builds, but a list is only trusted to the length of the walk.
    for _ in 0..toc.entries().len() {
        if cursor == NONE {
            break;
        }
        if let Some(at) = row_of.get(cursor as usize).copied().flatten().map(|at| at as usize) {
            rows[at].current = true;
            return at + 1 - first_row;
        }
        cursor = toc.entries().get(cursor as usize).map_or(NONE, |e| e.parent);
    }
    0
}

/// A page of the tab as the session counts it, in the file's own numbers, as its contents are in: none for a page added from another PDF, or while its list
/// is asleep.
fn original_page(page: u32, order: Order<'_>) -> Option<u32> {
    match order {
        Order::Edited(edits) => edits.slots().get(page as usize).filter(|s| s.source == 0).map(|s| s.page),
        Order::File => Some(page),
        Order::Unknown => None,
    }
}

/// The entry for the section being read, in the file's own page numbers: the one with the greatest place at or above the top of the reader's view, own
/// contents and generated alike ([`Toc::current_at`]).
fn section_here(toc: &Toc, order: Order<'_>, (page, top): (u32, u16), placed: &Placed) -> Option<usize> {
    original_page(page, order).and_then(|page| toc.current_at(page, top, |at| placed.page(at)))
}

/// The section being read and every section it's in, which following the place keeps open.
fn following_chain(toc: &Toc, order: Order<'_>, line: (u32, u16), placed: &Placed) -> HashSet<u32> {
    let mut open = HashSet::new();
    let mut cursor = section_here(toc, order, line, placed).map_or(NONE, |i| u32::try_from(i).unwrap_or(NONE));
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
