//! Tabs, tiers and the sidebar: every rule the host needs, with no UI toolkit in sight.
//!
//! **Cost follows activity.** A cold tab is a [`TabRecord`] and nothing else: no handle, no contents, no pages. Opening
//! reads a PDF's facts and closes it. Only the active tab and the most recent others keep their contents, up to
//! [`MAX_WARM`], and anything evicted is rebuilt from disk when it's needed again.

pub mod edit;

pub use edit::Author;
use edit::{EditError, PageEdits};
use recto_core::{DocFacts, NONE, OpenError, Toc};
use recto_platform::{FileStamp, file_stamp};
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
    /// Only the renderer could read this file. See [`DocFacts::renderer_only`].
    pub renderer_only: bool,
    /// Which version of the file this tab's facts and any edits are about. `None` if it couldn't be looked at.
    stamp: Option<FileStamp>,
}

/// Where a page of a tab comes from now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageSource {
    pub path: PathBuf,
    /// Zero-based page in that file.
    pub page: u32,
    /// Unsaved quarter turns on it.
    pub quarter_turns: u8,
    /// It came from another PDF added to this one, rather than the tab's own file.
    pub added: bool,
    /// What the file was when its page numbers were recorded, if this page's number depends on that: an added file, or the tab's own file once it's been edited.
    pub expected: Option<Option<FileStamp>>,
}

impl PageSource {
    /// Whether the file is still the one the page number is about. A file that's gone isn't. One that couldn't be looked at when it was recorded makes no claim.
    pub fn is_current(&self) -> bool {
        match self.expected {
            None | Some(None) => true,
            Some(Some(stamp)) => file_stamp(&self.path).ok() == Some(stamp),
        }
    }
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
    /// The owner's edits on the active PDF have been undone, by anyone, and can still be redone. The page matches the file, so `edited` is false, but closing would lose them, so the app has this to show.
    pub undone_owner_steps: bool,
}

/// The paper in dark mode. Light mode is always white. Owner, 2026-11-06: dark mode offers dark or white paper.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paper {
    #[default]
    Dark,
    White,
}

impl Paper {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::White => "white",
        }
    }
}

/// How pages are shown, which is the same for every tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewSettings {
    pub paper: Paper,
    /// Sidebar hidden, page number only, agent icon kept.
    pub focus: bool,
}

/// All open tabs and which one is active.
pub struct Session<L = DiskLoader> {
    loader: L,
    view: ViewSettings,
    tabs: Vec<TabRecord>,
    active: Option<TabId>,
    next_id: u32,
    /// Most recently used first, at most [`MAX_WARM`].
    warm: VecDeque<(TabId, Arc<Toc>)>,
    dirs: Vec<Box<Path>>,
    dir_ids: HashMap<Box<Path>, u32>,
    /// Only for tabs with page edits. A tab with none costs nothing here.
    edits: HashMap<TabId, PageEdits>,
    /// The latest pages drawn or tried, newest last, at most [`MAX_DRAWS_KEPT`]. Lets a waiter find out about a page finished a moment before it asked.
    draws: VecDeque<DrawNote>,
    /// The last edit removed the page a tab was on, so its place moved to the nearest one. Read once with [`Session::take_place_lost`].
    place_lost: bool,
}

/// Page results remembered across every tab, so nothing here grows with idle tabs.
pub const MAX_DRAWS_KEPT: usize = 256;

/// That a page was drawn, or couldn't be.
// size: asserted <= 32 by `draw_note_stays_small`. Held per recent draw, at most MAX_DRAWS_KEPT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrawNote {
    pub tab: TabId,
    /// Zero-based page.
    pub page: u32,
    /// The failure's stable code, or `None` if it was drawn.
    pub failure: Option<&'static str>,
}

impl Default for Session<DiskLoader> {
    fn default() -> Self {
        Self::new(DiskLoader)
    }
}

impl<L: DocLoader> Session<L> {
    pub fn new(loader: L) -> Self {
        Self { loader, view: ViewSettings::default(), tabs: Vec::new(), active: None, next_id: 1, warm: VecDeque::new(), dirs: Vec::new(), dir_ids: HashMap::new(), edits: HashMap::new(), draws: VecDeque::new(), place_lost: false }
    }

    pub fn view(&self) -> ViewSettings {
        self.view
    }

    pub fn set_view(&mut self, view: ViewSettings) {
        self.view = view;
    }

    /// How documents are read. Cheap to reach, so a caller can read a file without holding the session.
    pub fn loader(&self) -> &L {
        &self.loader
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

    /// The tabs that keep their contents: the active one and the most recent others, at most [`MAX_WARM`].
    pub fn warm_tabs(&self) -> impl Iterator<Item = TabId> + '_ {
        self.warm.iter().map(|(tab, _)| *tab)
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
        if let Some(id) = self.go_to_open_file(path) {
            return Ok(id);
        }
        // Looked at first, so a change during the read shows as a change.
        let stamp = file_stamp(path).ok();
        let facts = self.loader.load(path).map_err(SessionError::Open)?;
        self.open_with_facts(path, facts, stamp)
    }

    /// Activates the tab already showing this file, if there is one. The first half of [`Self::open`], for a caller that reads the file without holding the session.
    pub fn go_to_open_file(&mut self, path: &Path) -> Option<TabId> {
        let id = self.tab_for_path(path)?;
        self.activate(id);
        Some(id)
    }

    /// The tab already showing this file, if there is one.
    pub fn tab_for_path(&self, path: &Path) -> Option<TabId> {
        let dir = *self.dir_ids.get(path.parent().unwrap_or(Path::new("")))?;
        let file_name = path.file_name()?;
        self.tabs.iter().find(|t| t.dir == dir && &*t.file_name == file_name).map(|t| t.id)
    }

    /// The second half of [`Self::open`]: makes the tab from facts already read. Someone may have opened the same file meanwhile, and then that's the tab.
    pub fn open_with_facts(&mut self, path: &Path, facts: DocFacts, stamp: Option<FileStamp>) -> Result<TabId, SessionError> {
        if let Some(id) = self.go_to_open_file(path) {
            return Ok(id);
        }
        let dir_path = path.parent().unwrap_or(Path::new(""));
        let file_name = path.file_name().ok_or(SessionError::Open(OpenError::NotFound))?;
        if self.tabs.len() >= MAX_TABS {
            return Err(SessionError::TooManyTabs);
        }
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
            renderer_only: facts.renderer_only,
            stamp,
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

    /// Whether the owner's edits on this tab have been undone and can still be redone. Closing would lose them.
    pub fn has_undone_owner_steps(&self, id: TabId) -> bool {
        self.edits.get(&id).is_some_and(PageEdits::owner_steps_undone)
    }

    /// A tab's page edits, if it has any.
    pub fn page_edits(&self, id: TabId) -> Option<&PageEdits> {
        self.edits.get(&id)
    }

    /// Applies the owner's page edit: rearranging, rotating or deleting. This is what the app's own edits call, with nothing to say whose they are. Everything from the agent interface goes through [`Self::edit_pages_as`] with [`Author::Agent`].
    pub fn edit_pages(&mut self, id: TabId, change: impl FnOnce(&mut PageEdits) -> Result<(), EditError>) -> Result<(), SessionError> {
        self.edit_pages_as(id, Author::Owner, change)
    }

    /// Applies a page edit, noting who made it. The list is made the first time a tab is edited, and goes again if that edit was refused. The reader's place follows its page.
    pub fn edit_pages_as(&mut self, id: TabId, by: Author, change: impl FnOnce(&mut PageEdits) -> Result<(), EditError>) -> Result<(), SessionError> {
        let tab = self.tab(id).ok_or(SessionError::NoSuchTab)?;
        if tab.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        let page_count = tab.page_count;
        let place = self.place_of(id);
        let edits = self.edits.entry(id).or_insert_with(|| PageEdits::new(page_count));
        edits.act_as(by);
        let result = change(edits);
        edits.act_as(Author::Owner);
        if result.is_ok() {
            self.after_change(id, place);
        } else {
            self.drop_list_if_idle(id);
        }
        result.map_err(SessionError::Edit)
    }

    /// What the reader is looking at: which file, which page there, and where it is now.
    fn place_of(&self, id: TabId) -> Option<(u16, u32, u32)> {
        let position = self.tab(id)?.page;
        match self.edits.get(&id) {
            Some(edits) => edits.slots().get(position as usize).map(|slot| (slot.source, slot.page, position)),
            None => Some((0, position, position)),
        }
    }

    /// Everything a change to the page order means: a drawn page's number no longer names that page, and the reader stays on what they were reading.
    fn after_change(&mut self, id: TabId, place: Option<(u16, u32, u32)>) {
        self.draws.retain(|draw| draw.tab != id);
        self.drop_list_if_idle(id);
        let pages = self.pages_in(id);
        let found = place.and_then(|(source, page, near)| match self.edits.get(&id) {
            Some(edits) => edits.nearest_position(source, page, near),
            None => (source == 0 && page < pages).then_some(page),
        });
        let near = place.map_or(0, |(_, _, near)| near);
        self.place_lost = place.is_some() && found.is_none();
        let last = pages.saturating_sub(1);
        if let Some(tab) = self.tab_mut(id) {
            tab.page = found.unwrap_or(near).min(last);
        }
    }

    /// An edit list with nothing in it costs 8 bytes a page for nothing, and would hold the tab to the file's length as it was. One that's back at the file with steps to go back through stays until those are gone.
    fn drop_list_if_idle(&mut self, id: TabId) {
        if self.edits.get(&id).is_some_and(|edits| !edits.is_edited() && !edits.has_history()) {
            self.edits.remove(&id);
        }
    }

    /// Whether the last edit removed the page the reader was on, so its place moved to the nearest one. Asking clears it.
    pub fn take_place_lost(&mut self) -> bool {
        std::mem::take(&mut self.place_lost)
    }

    /// Adds every page of another PDF at a position, as dropping it into the page view does. Only its facts are read.
    pub fn insert_pdf(&mut self, id: TabId, at: u32, path: &Path) -> Result<(), SessionError> {
        let stamp = file_stamp(path).ok();
        let facts = self.loader.load(path).map_err(SessionError::Open)?;
        self.insert_pdf_with_facts(id, Author::Owner, at, path, &facts, stamp)
    }

    /// Adds another PDF's pages from facts already read, for a caller that reads the file without holding the session. A file only the renderer can read can't be added: Save couldn't write it.
    pub fn insert_pdf_with_facts(&mut self, id: TabId, by: Author, at: u32, path: &Path, facts: &DocFacts, stamp: Option<FileStamp>) -> Result<(), SessionError> {
        if facts.renderer_only {
            return Err(SessionError::RendererOnly);
        }
        if facts.page_count == 0 {
            return Err(SessionError::Open(OpenError::NotPdf));
        }
        self.edit_pages_as(id, by, |edits| edits.insert_pdf_stamped(at, path, facts.page_count, stamp))
    }

    /// Where a page of a tab comes from now, counting unsaved edits. Zero-based.
    pub fn page_source(&self, id: TabId, position: u32) -> Option<PageSource> {
        let tab = self.tab(id)?;
        let Some(edits) = self.edits.get(&id) else {
            return (position < tab.page_count).then(|| Some(PageSource { path: self.path_of(id)?, page: position, quarter_turns: 0, added: false, expected: None })).flatten();
        };
        let slot = *edits.slots().get(position as usize)?;
        match slot.source {
            0 => Some(PageSource { path: self.path_of(id)?, page: slot.page, quarter_turns: slot.quarter_turns, added: false, expected: Some(tab.stamp) }),
            source => {
                let added = edits.added_sources().get(usize::from(source) - 1)?;
                Some(PageSource { path: added.path.to_path_buf(), page: slot.page, quarter_turns: slot.quarter_turns, added: true, expected: Some(added.stamp) })
            }
        }
    }

    /// Whether a tab with edits still has the file those edits are about. A tab with none has no page numbers to get wrong: reading from the file as it is now is what's wanted.
    pub fn own_file_is_current(&self, id: TabId) -> bool {
        let Some(tab) = self.tab(id) else { return true };
        if !self.edits.contains_key(&id) {
            return true;
        }
        tab.stamp.is_none() || self.path_of(id).and_then(|path| file_stamp(&path).ok()) == tab.stamp
    }

    /// What a tab's own file was when its page numbers were recorded, for a caller listing pages by file.
    pub fn own_stamp(&self, id: TabId) -> Option<FileStamp> {
        self.tab(id).and_then(|tab| tab.stamp)
    }

    /// Notes that a page finished drawing, or couldn't be.
    pub fn note_drawn(&mut self, note: DrawNote) {
        self.draws.retain(|d| (d.tab, d.page) != (note.tab, note.page));
        self.draws.push_back(note);
        while self.draws.len() > MAX_DRAWS_KEPT {
            self.draws.pop_front();
        }
    }

    /// What's known about a page's latest draw, if it's still remembered.
    pub fn draw_note(&self, tab: TabId, page: u32) -> Option<DrawNote> {
        self.draws.iter().rev().find(|d| (d.tab, d.page) == (tab, page)).copied()
    }

    /// Reverses the last page edit. Returns false with nothing to undo.
    pub fn undo_edit(&mut self, id: TabId) -> bool {
        let place = self.place_of(id);
        let undone = self.edits.get_mut(&id).is_some_and(PageEdits::undo);
        if undone {
            self.after_change(id, place);
        }
        undone
    }

    /// Repeats the last undone page edit. Returns false with nothing to redo.
    pub fn redo_edit(&mut self, id: TabId) -> bool {
        let place = self.place_of(id);
        let redone = self.edits.get_mut(&id).is_some_and(PageEdits::redo);
        if redone {
            self.after_change(id, place);
        }
        redone
    }

    /// Throws away a tab's unsaved page edits.
    pub fn discard_edits(&mut self, id: TabId) {
        let place = self.place_of(id);
        if self.edits.remove(&id).is_some() {
            self.after_change(id, place);
        }
    }

    /// Closes a tab and releases what it held. The next tab along, or the previous one, becomes active.
    /// Refuses while there are unsaved changes, or undone edits of the owner's, so nothing is lost by accident. [`Self::discard_edits`] is how to say it's meant, as the app does when told "Don't Save".
    pub fn close(&mut self, id: TabId) -> Result<(), SessionError> {
        if self.is_edited(id) || self.has_undone_owner_steps(id) {
            return Err(SessionError::UnsavedChanges);
        }
        self.edits.remove(&id);
        let index = self.tabs.iter().position(|t| t.id == id).ok_or(SessionError::NoSuchTab)?;
        self.tabs.remove(index);
        self.draws.retain(|d| d.tab != id);
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
        let undone_owner_steps = self.active.is_some_and(|id| self.has_undone_owner_steps(id));
        SidebarModel { rows, rail_filled, toc_open, place, edited, undone_owner_steps }
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
        let stamp = self.path_of(id).and_then(|path| file_stamp(&path).ok());
        let loaded = self.path_of(id).ok_or(OpenError::NotFound).and_then(|path| self.loader.load(&path));
        // An edited tab's length is its edit list's, not the file's: added pages must stay reachable.
        let pages_with_edits = self.edits.get(&id).map(PageEdits::len);
        let tab = self.tab_mut(id);
        match loaded {
            Ok(facts) => {
                if let Some(tab) = tab {
                    tab.page_count = facts.page_count;
                    tab.page = tab.page.min(pages_with_edits.unwrap_or(facts.page_count).saturating_sub(1));
                    tab.problem = None;
                    tab.renderer_only = facts.renderer_only;
                    // With edits, the numbers in them are about the version they were made on, and that must stay known.
                    if pages_with_edits.is_none() {
                        tab.stamp = stamp;
                    }
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
    /// Only the renderer can read this file, so its pages can't be changed.
    RendererOnly,
}

impl SessionError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Open(error) => error.code(),
            Self::NoSuchTab => "no_such_tab",
            Self::TooManyTabs => "too_many_tabs",
            Self::UnsavedChanges => "unsaved_changes",
            Self::Edit(error) => error.code(),
            Self::RendererOnly => "renderer_only",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Open(error) => error.message(),
            Self::NoSuchTab => "That tab isn't open.",
            Self::TooManyTabs => "Too many PDFs are open. Close some and try again.",
            Self::UnsavedChanges => "This PDF has unsaved changes.",
            Self::Edit(error) => error.message(),
            Self::RendererOnly => "RectoPDF can draw this file but can't read its structure, so its pages can't be changed.",
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
            Ok(DocFacts { page_count: self.pages, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: Toc::default(), renderer_only: false })
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
            Ok(recto_core::DocFacts { page_count: 1, byte_len: 1, encrypted: false, version: "1.7".into(), title: None, toc: recto_core::Toc::default(), renderer_only: false })
        }
    }

    /// Failure: a field added carelessly raises the cost of every idle tab.
    #[test]
    fn tab_record_stays_small() {
        assert!(std::mem::size_of::<TabRecord>() <= 256);
    }

    /// Failure: a field added carelessly raises the cost of every remembered draw.
    #[test]
    fn draw_note_stays_small() {
        assert!(std::mem::size_of::<DrawNote>() <= 32);
    }

    /// Failure: after a move, the place stays at the same number and shows a different page, which no reader would do. It stays on the page, and if that page goes, it lands on the nearest and says so once.
    #[test]
    fn the_place_follows_the_page_being_read() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.go_to(id, 3).unwrap();
        s.edit_pages(id, |e| e.move_pages(3, 1, 0)).unwrap();
        assert_eq!(s.tab(id).unwrap().page, 0, "page 4 moved to the front, and the reader with it");
        assert!(!s.take_place_lost());
        s.edit_pages(id, |e| e.move_pages(1, 1, 3)).unwrap();
        assert_eq!(s.tab(id).unwrap().page, 0, "another page moved, so nothing changes");
        s.edit_pages(id, |e| e.delete(0, 1)).unwrap();
        assert!(s.take_place_lost(), "the page being read was deleted");
        assert!(!s.take_place_lost(), "it's only said once");
        assert_eq!(s.tab(id).unwrap().page, 0, "the nearest there is");
        s.go_to(id, 3).unwrap();
        s.edit_pages(id, |e| e.delete(3, 1)).unwrap();
        assert!(s.take_place_lost());
        assert_eq!(s.tab(id).unwrap().page, 2, "the last page, since that was the end");
        let before = s.tab(id).unwrap().page;
        assert!(s.undo_edit(id));
        assert_eq!(s.tab(id).unwrap().page, before, "undo keeps the page in view while one comes back behind it");
    }

    /// Failure: wait says a page is drawn when a different page now has that number, because notes are by position and outlive edits.
    #[test]
    fn draw_notes_go_when_the_pages_move() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        let note = DrawNote { tab: id, page: 0, failure: None };
        s.note_drawn(note);
        s.edit_pages(id, |e| e.move_pages(0, 1, 2)).unwrap();
        assert_eq!(s.draw_note(id, 0), None, "after an edit");
        s.note_drawn(note);
        assert!(s.undo_edit(id));
        assert_eq!(s.draw_note(id, 0), None, "after an undo");
        s.note_drawn(note);
        assert!(s.redo_edit(id));
        assert_eq!(s.draw_note(id, 0), None, "after a redo");
        s.note_drawn(note);
        s.discard_edits(id);
        assert_eq!(s.draw_note(id, 0), None, "after a discard");
        s.note_drawn(note);
        assert_eq!(s.edit_pages(id, |e| e.rotate(99, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
        assert_eq!(s.draw_note(id, 0), Some(note), "a refused edit changes nothing");
    }

    /// Failure: a tab back at its file through four quarter turns still reads as edited, and the sidebar's dot and the refusal to close stay with it. Its steps can still be undone, because the list isn't dropped while it has any. And a `page move` to where it already is must leave no list behind at all.
    #[test]
    fn a_tab_back_at_its_file_isnt_edited() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(id, |e| e.move_pages(2, 1, 2)).unwrap();
        assert!(s.page_edits(id).is_none() && !s.sidebar().edited, "nothing happened");
        for _ in 0..4 {
            s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
        }
        assert!(!s.is_edited(id) && !s.sidebar().edited);
        assert!(s.page_edits(id).unwrap().can_undo(), "it can still step back through them");
        assert!(s.undo_edit(id));
        assert!(s.is_edited(id));
        assert!(s.redo_edit(id));
        assert!(s.close(id).is_ok(), "it matches the file, and nothing's been undone");
    }

    /// Failure: a refused edit leaves an 8-byte-a-page list behind forever, over the budget for an idle tab, and the tab's length stops following its file.
    #[test]
    fn a_refused_first_edit_leaves_no_list() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        assert_eq!(s.edit_pages(id, |e| e.rotate(999, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
        assert!(s.page_edits(id).is_none(), "a tab with no edits must cost nothing here");
        s.edit_pages(id, |e| e.rotate(0, 1, 1)).unwrap();
        assert_eq!(s.edit_pages(id, |e| e.rotate(999, 1, 1)), Err(SessionError::Edit(EditError::OutOfRange)));
        assert!(s.page_edits(id).is_some(), "a real edit stays");
        assert!(s.undo_edit(id));
        assert!(s.page_edits(id).is_some(), "it can be redone");
        assert!(s.redo_edit(id));
        s.discard_edits(id);
        assert!(s.page_edits(id).is_none());
    }

    /// Failure: a file replaced behind an edited tab, or behind an added PDF, is read at the old page numbers as if nothing happened. A tab with no edits has nothing to get wrong, and reads the file as it is.
    #[test]
    fn a_page_knows_whether_its_file_is_the_one_it_was_numbered_from() {
        let dir = std::env::temp_dir().join(format!("recto-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (own, added) = (dir.join("own.pdf"), dir.join("added.pdf"));
        std::fs::write(&own, b"one").unwrap();
        std::fs::write(&added, b"two").unwrap();
        let mut s = session(3);
        let id = s.open(&own).unwrap();
        assert_eq!(s.page_source(id, 0).unwrap().expected, None, "no edits, so no claim");
        s.insert_pdf(id, 0, &added).unwrap();
        assert!(s.page_source(id, 0).unwrap().added);
        assert!(s.page_source(id, 0).unwrap().is_current());
        assert!(s.page_source(id, 3).unwrap().is_current());
        assert!(s.own_file_is_current(id));
        std::fs::write(&added, b"two, longer").unwrap();
        assert!(!s.page_source(id, 0).unwrap().is_current(), "the added file changed");
        assert!(s.page_source(id, 3).unwrap().is_current(), "the tab's own file hasn't");
        assert_eq!(s.insert_pdf(id, 0, &added), Err(SessionError::Edit(EditError::SourceChanged)), "two versions can't share page numbers");
        std::fs::remove_file(&added).unwrap();
        assert!(!s.page_source(id, 0).unwrap().is_current(), "the added file's gone");
        std::fs::write(&own, b"one, longer").unwrap();
        assert!(!s.page_source(id, 3).unwrap().is_current());
        assert!(!s.own_file_is_current(id));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Failure: a tab with added pages wakes from cold and lands back inside the file's own length, so the reader jumps away from the page they were on.
    #[test]
    fn waking_an_edited_tab_keeps_its_place_in_the_edited_length() {
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.insert_pdf(a, 5, Path::new("/d/added.pdf")).unwrap();
        s.go_to(a, 8).unwrap();
        for i in 0..MAX_WARM + 1 {
            s.open(Path::new(&format!("/d/other{i}.pdf"))).unwrap();
        }
        assert_eq!(s.tier(a), Tier::Cold);
        s.tap(a).unwrap();
        assert_eq!(s.tab(a).unwrap().page, 8);
    }

    /// Failure: after a move or a delete, reading "page 2" reads the file's second page rather than the one now in that place.
    #[test]
    fn a_pages_source_follows_edits() {
        let mut s = session(3);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        let three = s.page_source(a, 2).unwrap();
        assert_eq!((three.path.as_path(), three.page, three.quarter_turns, three.added), (Path::new("/d/a.pdf"), 2, 0, false));
        assert_eq!(s.page_source(a, 3), None);
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
        let first = s.page_source(a, 0).unwrap();
        assert_eq!((first.path.as_path(), first.page, first.quarter_turns, first.added), (Path::new("/d/a.pdf"), 1, 1, false));
        s.insert_pdf(a, 0, Path::new("/d/b.pdf")).unwrap();
        let second = s.page_source(a, 1).unwrap();
        assert_eq!((second.path.as_path(), second.page, second.added), (Path::new("/d/b.pdf"), 1, true));
    }

    struct RendererOnlyLoader;

    impl DocLoader for RendererOnlyLoader {
        fn load(&self, _path: &Path) -> Result<DocFacts, OpenError> {
            Ok(DocFacts { page_count: 4, byte_len: 1, encrypted: false, version: "".into(), title: None, toc: Toc::default(), renderer_only: true })
        }
    }

    /// Failure: a file only the renderer can read gets edited or added to another, though nothing could ever save the result. One writer: it must be readable and drawable, never editable.
    #[test]
    fn a_renderer_only_file_can_be_opened_but_not_edited_or_added() {
        let mut s = Session::new(RendererOnlyLoader);
        let id = s.open(Path::new("/d/damaged.pdf")).unwrap();
        assert_eq!(s.pages_in(id), 4);
        assert!(s.tab(id).unwrap().renderer_only);
        assert_eq!(s.edit_pages(id, |e| e.rotate(0, 1, 1)), Err(SessionError::RendererOnly));
        assert!(!s.is_edited(id));
        let mut other = session(3);
        let target = other.open(Path::new("/d/fine.pdf")).unwrap();
        let facts = RendererOnlyLoader.load(Path::new("/d/damaged.pdf")).unwrap();
        assert_eq!(other.insert_pdf_with_facts(target, Author::Owner, 0, Path::new("/d/damaged.pdf"), &facts, None), Err(SessionError::RendererOnly));
        assert!(!other.is_edited(target));
    }

    /// Failure: the draw notes grow with a long session, or an old result survives its tab, so a waiter is told a closed tab's page is drawn.
    #[test]
    fn draw_notes_are_capped_and_go_with_their_tab() {
        let mut s = session(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        for page in 0..(MAX_DRAWS_KEPT as u32 * 2) {
            s.note_drawn(DrawNote { tab: a, page, failure: None });
        }
        assert_eq!(s.draws.len(), MAX_DRAWS_KEPT);
        assert!(s.draw_note(a, 0).is_none());
        let last = MAX_DRAWS_KEPT as u32 * 2 - 1;
        assert!(s.draw_note(a, last).is_some());
        s.close(a).unwrap();
        assert!(s.draw_note(a, last).is_none());
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
        // Failure: an agent's `undo` takes the owner's edit off the page, so the dot goes and this close goes through with no prompt, and the edit is gone for good.
        assert!(s.sidebar().undone_owner_steps, "the app must be able to show it");
        assert_eq!(s.close(id), Err(SessionError::UnsavedChanges), "the owner's edit can still be redone, so closing would lose it");
        s.discard_edits(id);
        assert!(!s.sidebar().undone_owner_steps);
        assert!(s.close(id).is_ok());
    }

    /// Failure: undone steps of an agent's own hold up the app's close, or read as the owner's. They're an agent's to lose.
    #[test]
    fn an_agents_undone_steps_dont_stop_a_close() {
        let mut s = session(5);
        let id = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages_as(id, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
        assert!(s.undo_edit(id));
        assert!(!s.has_undone_owner_steps(id) && !s.sidebar().undone_owner_steps);
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
        assert_eq!(s.sidebar().place, "5 of 5", "the reader stays on the page they were reading");
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
