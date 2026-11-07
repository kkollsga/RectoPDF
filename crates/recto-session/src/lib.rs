//! Tabs, tiers and the sidebar: every rule the host needs, with no UI toolkit in sight.
//!
//! **Cost follows activity.** A cold tab is a [`TabRecord`] and nothing else: no handle, no contents, no pages. Opening
//! reads a PDF's facts and closes it. Only the active tab and the most recent others keep their contents, up to
//! [`MAX_WARM`], and anything evicted is rebuilt from disk when it's needed again.

pub mod edit;

use edit::{EditError, PageEdits};
use recto_core::{DocFacts, NONE, OpenError, Toc};
use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Tabs that may keep their contents in memory: the active one plus two recent ones.
pub const MAX_WARM: usize = 3;
/// A cap on tabs, because everything grown from outside has one.
pub const MAX_TABS: usize = 10_000;

/// Reads a document's facts. A seam so tests can stand in for the disk.
pub trait DocLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError>;
}

/// Reads from disk through the lazy reader.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiskLoader;

impl DocLoader for DiskLoader {
    fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
        recto_core::inspect(path)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u32);

/// How much of a tab is in memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The record only.
    Cold,
    /// The record plus contents.
    Warm,
    /// The tab on screen.
    Hot,
}

/// One tab. Everything else about it can be rebuilt from disk.
// size: asserted <= 256 by `tab_record_stays_small`. Held per tab, including cold ones.
#[derive(Debug)]
pub struct TabRecord {
    pub id: TabId,
    dir: u32,
    file_name: Box<OsStr>,
    /// The name shown in the sidebar: the file's name without its extension.
    pub name: Box<str>,
    /// Zero-based page the reader is on.
    pub page: u32,
    pub page_count: u32,
    pub toc_open: bool,
    /// Why this tab's file can't be read at the moment, if so.
    pub problem: Option<OpenError>,
}

/// A row in the sidebar. Borrows its text from the session, so building the model allocates little.
// size: asserted <= 64 by `sidebar_row_stays_small`. Held per visible row while a model is alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarRow<'a> {
    pub kind: RowKind,
    pub tab: TabId,
    pub text: &'a str,
    /// One-based page number, shown on hover.
    pub page: Option<u32>,
    /// Nesting within the contents.
    pub depth: u8,
    /// The active PDF, or the section being read.
    pub current: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Tab,
    Contents,
}

/// Everything the host draws in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarModel<'a> {
    pub rows: Vec<SidebarRow<'a>>,
    /// How many rows the rail fills: contents rows up to and including the section being read.
    pub rail_filled: usize,
    /// Whether the active PDF's contents are showing, which decides the chevron.
    pub toc_open: bool,
    /// The footer line, such as "32 of 96". Empty with no PDF open.
    pub place: String,
    /// Whether the active PDF has unsaved changes, which shows its dot and the Save buttons.
    pub edited: bool,
}

/// All open tabs and which one is active.
pub struct Session<L = DiskLoader> {
    loader: L,
    tabs: Vec<TabRecord>,
    active: Option<TabId>,
    next_id: u32,
    /// Most recently used first, at most [`MAX_WARM`].
    warm: VecDeque<(TabId, Arc<Toc>)>,
    dirs: Vec<Box<Path>>,
    dir_ids: HashMap<Box<Path>, u32>,
    /// Only for tabs with page edits. A tab with none costs nothing here.
    edits: HashMap<TabId, PageEdits>,
}

impl Default for Session<DiskLoader> {
    fn default() -> Self {
        Self::new(DiskLoader)
    }
}

impl<L: DocLoader> Session<L> {
    pub fn new(loader: L) -> Self {
        Self { loader, tabs: Vec::new(), active: None, next_id: 1, warm: VecDeque::new(), dirs: Vec::new(), dir_ids: HashMap::new(), edits: HashMap::new() }
    }

    pub fn tabs(&self) -> &[TabRecord] {
        &self.tabs
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    pub fn tab(&self, id: TabId) -> Option<&TabRecord> {
        self.tabs.iter().find(|t| t.id == id)
    }

    /// The tab's full path, rebuilt from its interned folder and name.
    pub fn path_of(&self, id: TabId) -> Option<PathBuf> {
        let tab = self.tab(id)?;
        Some(self.dirs.get(tab.dir as usize)?.join(&*tab.file_name))
    }

    pub fn tier(&self, id: TabId) -> Tier {
        if self.active == Some(id) {
            Tier::Hot
        } else if self.warm.iter().any(|(tab, _)| *tab == id) {
            Tier::Warm
        } else {
            Tier::Cold
        }
    }

    /// Opens a PDF as a new active tab. Opening a file that already has a tab goes to that tab instead.
    pub fn open(&mut self, path: &Path) -> Result<TabId, SessionError> {
        let dir_path = path.parent().unwrap_or(Path::new(""));
        let file_name = path.file_name().ok_or(SessionError::Open(OpenError::NotFound))?;
        if let Some(&dir) = self.dir_ids.get(dir_path)
            && let Some(id) = self.tabs.iter().find(|t| t.dir == dir && &*t.file_name == file_name).map(|t| t.id)
        {
            self.activate(id);
            return Ok(id);
        }
        if self.tabs.len() >= MAX_TABS {
            return Err(SessionError::TooManyTabs);
        }
        let facts = self.loader.load(path).map_err(SessionError::Open)?;
        let dir = self.intern_dir(dir_path);
        let id = TabId(self.next_id);
        self.next_id += 1;
        // The name comes from outside: a downloaded file can carry anything in it, and this reaches the sidebar and agents.
        let stem = Path::new(file_name).file_stem().unwrap_or(file_name).to_string_lossy();
        let name = match recto_core::clean_line(&stem) {
            name if name.is_empty() => "Untitled".to_string(),
            name => name,
        };
        self.tabs.push(TabRecord {
            id,
            dir,
            file_name: file_name.into(),
            name: name.into(),
            page: 0,
            page_count: facts.page_count,
            toc_open: true,
            problem: None,
        });
        self.remember_warm(id, Arc::new(facts.toc));
        self.active = Some(id);
        Ok(id)
    }

    /// **The sidebar's tap rule.** Tapping another PDF switches to it. Tapping the active PDF folds or opens its contents.
    pub fn tap(&mut self, id: TabId) -> Result<(), SessionError> {
        if self.tab(id).is_none() {
            return Err(SessionError::NoSuchTab);
        }
        if self.active == Some(id) {
            if let Some(tab) = self.tab_mut(id) {
                tab.toc_open = !tab.toc_open;
            }
        } else {
            self.activate(id);
        }
        Ok(())
    }

    /// Moves a tab to a zero-based page, clamped to the document.
    pub fn go_to(&mut self, id: TabId, page: u32) -> Result<(), SessionError> {
        let pages = self.pages_in(id);
        let tab = self.tab_mut(id).ok_or(SessionError::NoSuchTab)?;
        tab.page = page.min(pages.saturating_sub(1));
        Ok(())
    }

    /// How many pages a tab has now, counting unsaved edits.
    pub fn pages_in(&self, id: TabId) -> u32 {
        self.edits.get(&id).map(PageEdits::len).or_else(|| self.tab(id).map(|t| t.page_count)).unwrap_or(0)
    }

    /// Whether a tab has unsaved changes.
    pub fn is_edited(&self, id: TabId) -> bool {
        self.edits.get(&id).is_some_and(PageEdits::is_edited)
    }

    /// A tab's page edits, if it has any.
    pub fn page_edits(&self, id: TabId) -> Option<&PageEdits> {
        self.edits.get(&id)
    }

    /// Applies a page edit: rearranging, rotating or deleting. The list is made the first time a tab is edited.
    pub fn edit_pages(&mut self, id: TabId, change: impl FnOnce(&mut PageEdits) -> Result<(), EditError>) -> Result<(), SessionError> {
        let page_count = self.tab(id).ok_or(SessionError::NoSuchTab)?.page_count;
        change(self.edits.entry(id).or_insert_with(|| PageEdits::new(page_count))).map_err(SessionError::Edit)?;
        self.keep_page_in_range(id);
        Ok(())
    }

    /// Adds every page of another PDF at a position, as dropping it into the page view does. Only its facts are read.
    pub fn insert_pdf(&mut self, id: TabId, at: u32, path: &Path) -> Result<(), SessionError> {
        let facts = self.loader.load(path).map_err(SessionError::Open)?;
        self.edit_pages(id, |edits| edits.insert_pdf(at, path, facts.page_count))
    }

    /// Reverses the last page edit. Returns false with nothing to undo.
    pub fn undo_edit(&mut self, id: TabId) -> bool {
        let undone = self.edits.get_mut(&id).is_some_and(PageEdits::undo);
        self.keep_page_in_range(id);
        undone
    }

    /// Repeats the last undone page edit. Returns false with nothing to redo.
    pub fn redo_edit(&mut self, id: TabId) -> bool {
        let redone = self.edits.get_mut(&id).is_some_and(PageEdits::redo);
        self.keep_page_in_range(id);
        redone
    }

    /// Throws away a tab's unsaved page edits.
    pub fn discard_edits(&mut self, id: TabId) {
        self.edits.remove(&id);
        self.keep_page_in_range(id);
    }

    fn keep_page_in_range(&mut self, id: TabId) {
        let pages = self.pages_in(id);
        if let Some(tab) = self.tab_mut(id) {
            tab.page = tab.page.min(pages.saturating_sub(1));
        }
    }

    /// Closes a tab and releases what it held. The next tab along, or the previous one, becomes active.
    /// Refuses while there are unsaved changes, so nothing is lost by accident.
    pub fn close(&mut self, id: TabId) -> Result<(), SessionError> {
        if self.is_edited(id) {
            return Err(SessionError::UnsavedChanges);
        }
        self.edits.remove(&id);
        let index = self.tabs.iter().position(|t| t.id == id).ok_or(SessionError::NoSuchTab)?;
        self.tabs.remove(index);
        self.warm.retain(|(tab, _)| *tab != id);
        if self.active == Some(id) {
            self.active = None;
            let next = self.tabs.get(index).or_else(|| index.checked_sub(1).and_then(|i| self.tabs.get(i))).map(|t| t.id);
            if let Some(next) = next {
                self.activate(next);
            }
        }
        Ok(())
    }

    /// Builds the sidebar. Cost depends on the visible rows and the active tab's contents, never on idle tabs' PDFs.
    pub fn sidebar(&self) -> SidebarModel<'_> {
        let mut rows = Vec::with_capacity(self.tabs.len());
        let mut rail_filled = 0;
        let mut place = String::new();
        let mut toc_open = false;
        for tab in &self.tabs {
            let is_active = self.active == Some(tab.id);
            rows.push(SidebarRow { kind: RowKind::Tab, tab: tab.id, text: &tab.name, page: None, depth: 0, current: is_active });
            if !is_active {
                continue;
            }
            toc_open = tab.toc_open;
            place = format!("{} of {}", tab.page + 1, self.pages_in(tab.id));
            if tab.toc_open
                && let Some((_, toc)) = self.warm.iter().find(|(id, _)| *id == tab.id)
            {
                rail_filled = contents_rows(toc, tab, self.edits.get(&tab.id), &mut rows);
            }
        }
        let edited = self.active.is_some_and(|id| self.is_edited(id));
        SidebarModel { rows, rail_filled, toc_open, place, edited }
    }

    fn activate(&mut self, id: TabId) {
        self.active = Some(id);
        if let Some(position) = self.warm.iter().position(|(tab, _)| *tab == id) {
            if let Some(entry) = self.warm.remove(position) {
                self.warm.push_front(entry);
            }
            return;
        }
        // Woken from cold: rebuild from disk. If that fails, the tab stays and says why.
        let loaded = self.path_of(id).ok_or(OpenError::NotFound).and_then(|path| self.loader.load(&path));
        let tab = self.tab_mut(id);
        match loaded {
            Ok(facts) => {
                if let Some(tab) = tab {
                    tab.page_count = facts.page_count;
                    tab.page = tab.page.min(facts.page_count.saturating_sub(1));
                    tab.problem = None;
                }
                self.remember_warm(id, Arc::new(facts.toc));
            }
            Err(problem) => {
                if let Some(tab) = tab {
                    tab.problem = Some(problem);
                }
            }
        }
    }

    fn remember_warm(&mut self, id: TabId, toc: Arc<Toc>) {
        self.warm.push_front((id, toc));
        self.warm.truncate(MAX_WARM);
    }

    fn tab_mut(&mut self, id: TabId) -> Option<&mut TabRecord> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn intern_dir(&mut self, dir: &Path) -> u32 {
        if let Some(&id) = self.dir_ids.get(dir) {
            return id;
        }
        let id = u32::try_from(self.dirs.len()).unwrap_or(u32::MAX);
        self.dirs.push(dir.into());
        self.dir_ids.insert(dir.into(), id);
        id
    }
}

/// Adds the visible contents rows: every top-level entry, plus the chapter being read opened down to the current
/// section. Returns how many rows the rail fills.
///
/// With unsaved page edits, entries follow their pages to their new positions. An entry whose page was deleted stays
/// without a number, and pages added from another PDF belong to no section.
fn contents_rows<'a>(toc: &'a Toc, tab: &TabRecord, edits: Option<&PageEdits>, rows: &mut Vec<SidebarRow<'a>>) -> usize {
    let original_page = match edits {
        Some(edits) => edits.slots().get(tab.page as usize).filter(|s| s.source == 0).map(|s| s.page),
        None => Some(tab.page),
    };
    let here = original_page.and_then(|page| toc.section_at(page));
    let mut open = HashSet::new();
    let mut cursor = here.map_or(NONE, |i| u32::try_from(i).unwrap_or(NONE));
    while cursor != NONE && open.insert(cursor) {
        cursor = toc.entries().get(cursor as usize).map_or(NONE, |e| e.parent);
    }
    let first_row = rows.len();
    let mut filled = 0;
    for (index, entry) in toc.entries().iter().enumerate() {
        if entry.parent != NONE && !open.contains(&entry.parent) {
            continue;
        }
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
        });
        if current {
            filled = rows.len() - first_row;
        }
    }
    filled
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionError {
    Open(OpenError),
    NoSuchTab,
    TooManyTabs,
    UnsavedChanges,
    Edit(EditError),
}

impl SessionError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Open(error) => error.code(),
            Self::NoSuchTab => "no_such_tab",
            Self::TooManyTabs => "too_many_tabs",
            Self::UnsavedChanges => "unsaved_changes",
            Self::Edit(error) => error.code(),
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Open(error) => error.message(),
            Self::NoSuchTab => "That tab isn't open.",
            Self::TooManyTabs => "Too many PDFs are open. Close some and try again.",
            Self::UnsavedChanges => "This PDF has unsaved changes.",
            Self::Edit(error) => error.message(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FakeLoader {
        pages: u32,
        loads: Cell<u32>,
    }

    impl DocLoader for FakeLoader {
        fn load(&self, path: &Path) -> Result<DocFacts, OpenError> {
            self.loads.set(self.loads.get() + 1);
            if path.to_string_lossy().contains("missing") {
                return Err(OpenError::NotFound);
            }
            Ok(DocFacts { page_count: self.pages, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default() })
        }
    }

    fn session(pages: u32) -> Session<FakeLoader> {
        Session::new(FakeLoader { pages, loads: Cell::new(0) })
    }

    /// Failure: a downloaded file's name carries tag characters or a line break, and they reach the sidebar, `tabs` and error candidates: an instruction no eye can see. The path must
    /// stay exact, or the file couldn't be opened again.
    #[test]
    fn tab_names_are_cleaned_and_paths_stay_exact() {
        let mut session = Session::new(NameLoader);
        let hidden: String = "ignore".chars().map(|c| char::from_u32(0xE0000 + c as u32).unwrap()).collect();
        let raw = format!("/d/Report{hidden}\n2026\u{202E}.pdf");
        let id = session.open(Path::new(&raw)).unwrap();
        let tab = session.tab(id).unwrap();
        assert_eq!(&*tab.name, "Report 2026");
        assert_eq!(session.path_of(id).unwrap(), PathBuf::from(&raw));
        assert_eq!(session.sidebar().rows[0].text, "Report 2026");
        let invisible = session.open(Path::new("/d/\u{200B}.pdf")).unwrap();
        assert_eq!(&*session.tab(invisible).unwrap().name, "Untitled", "a name of nothing still needs a row");
    }

    struct NameLoader;

    impl DocLoader for NameLoader {
        fn load(&self, _path: &Path) -> Result<recto_core::DocFacts, recto_core::OpenError> {
            Ok(recto_core::DocFacts { page_count: 1, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: recto_core::Toc::default() })
        }
    }

    /// Failure: a field added carelessly raises the cost of every idle tab.
    #[test]
    fn tab_record_stays_small() {
        assert!(std::mem::size_of::<TabRecord>() <= 256);
    }

    /// Failure: a field added carelessly raises the cost of every visible row.
    #[test]
    fn sidebar_row_stays_small() {
        assert!(std::mem::size_of::<SidebarRow<'_>>() <= 64);
    }

    /// Failure: tapping the active PDF switches tabs or does nothing, instead of folding its contents.
    #[test]
    fn tapping_the_active_pdf_toggles_contents() {
        let mut s = session(10);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert!(s.tab(id).unwrap().toc_open);
        s.tap(id).unwrap();
        assert!(!s.tab(id).unwrap().toc_open);
        assert_eq!(s.active(), Some(id));
        s.tap(id).unwrap();
        assert!(s.tab(id).unwrap().toc_open);
    }

    /// Failure: tapping another PDF folds the current one instead of switching, or forgets its folded state.
    #[test]
    fn tapping_another_pdf_switches_and_keeps_each_fold_state() {
        let mut s = session(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.tap(a).unwrap();
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        s.tap(a).unwrap();
        assert_eq!(s.active(), Some(a));
        assert!(!s.tab(a).unwrap().toc_open);
        assert!(s.tab(b).unwrap().toc_open);
    }

    /// Failure: contents stay in memory for every tab, so many large PDFs cost memory while idle.
    #[test]
    fn only_a_few_tabs_stay_warm() {
        let mut s = session(10);
        let ids: Vec<_> = (0..20).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
        assert_eq!(s.tier(*ids.last().unwrap()), Tier::Hot);
        let warm = ids.iter().filter(|id| s.tier(**id) == Tier::Warm).count();
        assert!(warm < MAX_WARM);
        assert!(ids[..15].iter().all(|id| s.tier(*id) == Tier::Cold));
    }

    /// Failure: returning to a recent tab re-reads the file, or a cold tab isn't rebuilt from disk.
    #[test]
    fn switching_rebuilds_only_cold_tabs() {
        let mut s = session(10);
        let ids: Vec<_> = (0..5).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
        let before = s.loader.loads.get();
        s.tap(ids[3]).unwrap();
        assert_eq!(s.loader.loads.get(), before, "warm tab read the file again");
        s.tap(ids[0]).unwrap();
        assert_eq!(s.loader.loads.get(), before + 1, "cold tab wasn't rebuilt");
    }

    /// Failure: opening the same file twice makes a second tab rather than going to the first.
    #[test]
    fn opening_a_file_twice_reuses_its_tab() {
        let mut s = session(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.open(Path::new("/d/b.pdf")).unwrap();
        assert_eq!(s.open(Path::new("/d/a.pdf")).unwrap(), a);
        assert_eq!(s.tabs().len(), 2);
        assert_eq!(s.active(), Some(a));
    }

    /// Failure: a page past the end leaves the reader nowhere, or a zero-page file underflows.
    #[test]
    fn going_to_a_page_clamps_to_the_document() {
        let mut s = session(10);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.go_to(id, 500).unwrap();
        assert_eq!(s.tab(id).unwrap().page, 9);
        assert_eq!(s.sidebar().place, "10 of 10");
        let mut empty = session(0);
        let id = empty.open(Path::new("/d/empty.pdf")).unwrap();
        empty.go_to(id, 3).unwrap();
        assert_eq!(empty.tab(id).unwrap().page, 0);
    }

    /// Failure: closing the active tab leaves nothing active while others are open, or leaves its contents in memory.
    #[test]
    fn closing_the_active_tab_activates_a_neighbour() {
        let mut s = session(10);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let b = s.open(Path::new("/d/b.pdf")).unwrap();
        s.close(b).unwrap();
        assert_eq!(s.active(), Some(a));
        assert!(s.warm.iter().all(|(id, _)| *id != b));
        s.close(a).unwrap();
        assert_eq!(s.active(), None);
        assert_eq!(s.sidebar().place, "");
    }

    /// Failure: closing a tab loses page edits without warning.
    #[test]
    fn closing_refuses_while_there_are_unsaved_edits() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
        assert_eq!(s.close(id), Err(SessionError::UnsavedChanges));
        assert!(s.sidebar().edited);
        assert!(s.undo_edit(id));
        assert!(!s.sidebar().edited);
        assert!(s.close(id).is_ok());
    }

    /// Failure: the footer and page clamp ignore added or deleted pages, so the reader lands past the end.
    #[test]
    fn page_count_and_position_follow_edits() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.go_to(id, 4).unwrap();
        s.edit_pages(id, |e| e.delete(1, 3)).unwrap();
        assert_eq!(s.sidebar().place, "2 of 2");
        s.insert_pdf(id, 2, Path::new("/d/b.pdf")).unwrap();
        assert_eq!(s.pages_in(id), 7);
        assert_eq!(s.sidebar().place, "2 of 7");
        s.discard_edits(id);
        assert_eq!(s.sidebar().place, "2 of 5");
    }

    /// Failure: every open tab pays for page edits, so idle tabs cost more once one PDF is edited.
    #[test]
    fn only_edited_tabs_have_an_edit_list() {
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.open(Path::new("/d/b.pdf")).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        assert_eq!(s.edits.len(), 1);
    }

    /// Failure: a dropped PDF that can't be read changes the document anyway.
    #[test]
    fn a_dropped_pdf_that_cant_be_read_changes_nothing() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert_eq!(s.insert_pdf(id, 0, Path::new("/d/missing.pdf")), Err(SessionError::Open(OpenError::NotFound)));
        assert!(!s.is_edited(id));
        assert_eq!(s.pages_in(id), 5);
    }

    /// Failure: a PDF moved away after opening crashes or disappears, rather than staying open and saying why.
    #[test]
    fn a_tab_whose_file_vanished_stays_and_says_why() {
        let mut s = session(10);
        let gone = s.open(Path::new("/d/missing-later.pdf"));
        assert!(gone.is_err());
        let ids: Vec<_> = (0..5).map(|i| s.open(Path::new(&format!("/d/{i}.pdf"))).unwrap()).collect();
        // Rename the first tab's file underneath the session, then wake it from cold.
        s.tab_mut(ids[0]).unwrap().file_name = OsStr::new("missing.pdf").into();
        s.tap(ids[0]).unwrap();
        assert_eq!(s.tab(ids[0]).unwrap().problem, Some(OpenError::NotFound));
        assert_eq!(s.active(), Some(ids[0]));
    }
}
