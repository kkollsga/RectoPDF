//! The open tabs: each tab's record in the sidebar's order, which one is active, the number the next one gets, and the folders their files are in.

use crate::confined::Confinement;
use crate::dirs::Dirs;
use crate::record::{self, FileNotice, PlaceRecord, Selection};
use crate::{TabId, TabRecord, trail};
use recto_core::OpenError;
use recto_platform::FileStamp;
use std::path::{Path, PathBuf};

/// Every open tab, which one's active, and their folders. Reads as the tabs in the sidebar's order. What's public here is about one tab, or reads; what
/// adds a tab, takes one away or hands out a tab's record to change stays the crate's, as the other components hold something of every tab and must change
/// with it.
#[derive(Debug)]
pub struct Tabs {
    /// In the sidebar's order.
    tabs: Vec<TabRecord>,
    active: Option<TabId>,
    /// The number the next tab opened gets.
    next_id: u32,
    /// The folders tabs' files are in.
    dirs: Dirs,
    /// **The session's confinement, its one cell:** kept here, by what hands a tab's path out, so a path from here meets the rule ([`Self::path_of`]),
    /// and the session asks it here too.
    confinement: Confinement,
}

impl Tabs {
    /// No tabs, and a confinement cell of its own, not yet set. The crate's: a `Tabs` made outside could take a session's place, and its cell with it
    /// (see [`crate::parts::PartMut`]).
    pub(crate) fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active: None,
            next_id: 1,
            dirs: Dirs::default(),
            confinement: Confinement::default(),
        }
    }
}

/// **How many tabs' records the searches for a tab looked at, on this thread,** in a debug build only: a release build has none of it. A walk over every tab
/// that looks each one up again by its number is quadratic in the tabs open, and costs nothing anyone would see until hundreds are (2026-11-13: the
/// sidebar did it for each tab's notice, and every action at 1,000 tabs took about 0.5 ms more). Per thread, as whatever reads the session reads it on the
/// thread that holds it. What the guards against that read: `tests/per_tab_work.rs` here, and the headless app's `per_tab_work`.
#[cfg(debug_assertions)]
pub mod searched {
    use std::cell::Cell;

    thread_local! {
        static RECORDS: Cell<u64> = const { Cell::new(0) };
    }

    /// The records looked at so far on this thread.
    pub fn records() -> u64 {
        RECORDS.with(Cell::get)
    }

    pub(crate) fn add(records: usize) {
        RECORDS.with(|count| count.set(count.get() + records as u64));
    }
}

/// The tabs in the sidebar's order, to read. There's no `DerefMut`: a tab's record isn't changed from outside the crate.
impl std::ops::Deref for Tabs {
    type Target = [TabRecord];

    fn deref(&self) -> &[TabRecord] {
        &self.tabs
    }
}

impl Tabs {
    /// Every tab, in the sidebar's order.
    pub(crate) fn all(&self) -> &[TabRecord] {
        &self.tabs
    }

    pub(crate) fn iter_mut(&mut self) -> std::slice::IterMut<'_, TabRecord> {
        self.tabs.iter_mut()
    }

    pub fn tab(&self, id: TabId) -> Option<&TabRecord> {
        self.position(id).map(|at| &self.tabs[at])
    }

    pub(crate) fn tab_mut(&mut self, id: TabId) -> Option<&mut TabRecord> {
        self.position(id).map(|at| &mut self.tabs[at])
    }

    /// Where a tab is in the sidebar's order: a search, each record looked at counted in a debug build ([`searched`]). Every lookup of a tab by its number
    /// comes here.
    pub(crate) fn position(&self, id: TabId) -> Option<usize> {
        let at = self.tabs.iter().position(|t| t.id == id);
        #[cfg(debug_assertions)]
        searched::add(at.map_or(self.tabs.len(), |at| at + 1));
        at
    }

    /// The tab at this place in the sidebar's order.
    pub(crate) fn at(&self, index: usize) -> Option<&TabRecord> {
        self.tabs.get(index)
    }

    pub(crate) fn push(&mut self, tab: TabRecord) {
        self.tabs.push(tab);
    }

    /// The tab at this place goes, its folder let go of if no other tab is in it.
    pub(crate) fn remove(&mut self, index: usize) -> TabRecord {
        let closed = self.tabs.remove(index);
        self.let_go_of_dir_if_unused(closed.file.dir);
        closed
    }

    pub fn active(&self) -> Option<TabId> {
        self.active
    }

    pub(crate) fn set_active(&mut self, id: Option<TabId>) {
        self.active = id;
    }

    /// The number the next tab gets.
    pub(crate) fn next_id(&self) -> u32 {
        self.next_id
    }

    /// The next tab's number, taken.
    pub(crate) fn take_id(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        id
    }

    #[cfg(test)]
    pub(crate) fn set_next_id(&mut self, next: u32) {
        self.next_id = next;
    }

    /// A folder's number, given it the first time it's asked for.
    pub(crate) fn intern_dir(&mut self, dir: &Path) -> u32 {
        self.dirs.intern(dir)
    }

    /// How many folders are kept.
    #[cfg(test)]
    pub(crate) fn dirs_kept(&self) -> usize {
        self.dirs.len()
    }

    /// The session's confinement: see [`crate::confined`].
    pub(crate) fn confinement(&self) -> &Confinement {
        &self.confinement
    }

    /// **The tab's full path, for whatever reads or writes its file,** rebuilt from its interned folder and name. In a confined session it's put to the
    /// rule again, as it resolves now, so a folder swapped for a link after the open leads nowhere: `None`, as for no tab, and
    /// `Session::apply_file_status` says its file is missing (2026-11-13). It's here, on the one way to a tab's path, so a caller reaching it through
    /// `Session::tabs` meets the rule as one through the session does. A free session asks nothing of the disk.
    pub fn path_of(&self, id: TabId) -> Option<PathBuf> {
        self.named_path(id).filter(|path| self.confinement.admits(path))
    }

    /// **The path the tab is named by,** with no rule: only for what names the tab and reads nothing of its file (its record, its journal's and sleep
    /// file's names, the look at whether its file is still there, which is what says a swapped one is missing).
    pub(crate) fn named_path(&self, id: TabId) -> Option<PathBuf> {
        self.named_path_of(self.tab(id)?)
    }

    /// [`Self::named_path`], for a tab whose record is in hand, with no search for it: what a walk over every tab calls, as the record does.
    pub(crate) fn named_path_of(&self, tab: &TabRecord) -> Option<PathBuf> {
        Some(self.dirs.get(tab.file.dir)?.join(&*tab.file.file_name))
    }

    /// The tab already showing this file, by its path.
    pub fn for_path(&self, path: &Path) -> Option<TabId> {
        let dir = self.dirs.id_of(path.parent().unwrap_or(Path::new("")))?;
        let file_name = path.file_name()?;
        let at = self.tabs.iter().position(|t| t.file.dir == dir && &*t.file.file_name == file_name);
        #[cfg(debug_assertions)]
        searched::add(at.map_or(self.tabs.len(), |at| at + 1));
        at.map(|at| self.tabs[at].id)
    }

    /// A folder no tab is in any more is let go of, so the folders kept are only those in use.
    pub(crate) fn let_go_of_dir_if_unused(&mut self, dir: u32) {
        if !self.tabs.iter().any(|tab| tab.file.dir == dir) {
            self.dirs.let_go(dir);
        }
    }

    /// The tab's notice, if there is one.
    pub fn tab_notice(&self, id: TabId) -> Option<&FileNotice> {
        self.tab(id)?.file.notice()
    }

    /// Whether a tab's file needs a password that hasn't been given, which the app asks for when it's showing.
    pub fn needs_password(&self, id: TabId) -> bool {
        self.tab(id).is_some_and(|tab| tab.problem == Some(OpenError::PasswordNeeded))
    }

    /// **Nothing of this tab's file goes to an agent:** it was opened with a password the person typed, or it needs one, and what the record has of it (its length, its place) was read with one. Reading a protected document needs the person's consent for that document (agent report §5.5), which isn't built, so it's refused outright. `encrypted` alone isn't this: a file anyone may open can be encrypted too.
    pub fn withheld_from_agents(&self, id: TabId) -> bool {
        self.tab(id).is_some_and(|tab| {
            tab.file.password || tab.problem == Some(OpenError::PasswordNeeded) || matches!(tab.file.notice(), Some(FileNotice::NeedsPassword))
        })
    }

    /// What a tab's own file was when its page numbers were recorded, for a caller listing pages by file.
    pub fn own_stamp(&self, id: TabId) -> Option<FileStamp> {
        self.tab(id).and_then(|tab| tab.file.stamp)
    }

    /// Every tab's own file's stamp, where known, in one walk: for a caller about all the files open, which asking [`Self::own_stamp`] of each tab would
    /// make a search a tab.
    pub fn own_stamps(&self) -> impl Iterator<Item = FileStamp> + '_ {
        self.tabs.iter().filter_map(|tab| tab.file.stamp)
    }

    /// How a tab's pages are laid out.
    pub fn spread_of(&self, id: TabId) -> Option<record::Spread> {
        self.tab(id).map(|tab| tab.spread)
    }

    /// A tab's trail, for QA and `recto where`.
    pub fn trail_of(&self, id: TabId) -> Option<&trail::Trail> {
        self.tab(id)?.reading.trail.as_deref()
    }

    /// Whether a tab can go back, and forward.
    pub fn can_go_back_and_forward(&self, id: TabId) -> (bool, bool) {
        self.trail_of(id)
            .map_or((false, false), |trail| (!trail.back().is_empty(), !trail.forward().is_empty()))
    }

    /// Where to bring a tab back to: only if it's still about the page the tab is on, as a `goto` while it was cold leaves it not.
    pub fn place_to_show(&self, id: TabId) -> Option<PlaceRecord> {
        let tab = self.tab(id)?;
        tab.reading.place.filter(|place| place.page == tab.page)
    }

    /// What's selected in a tab. Carried across a restart, and nothing else here reads it. The crate's: from outside, a selection is noted only by
    /// [`crate::parts::PartMut::note_selection`], which keeps the rule that only the tab showing holds one.
    pub(crate) fn set_selection(&mut self, id: TabId, selection: Option<Selection>) {
        if let Some(tab) = self.tab_mut(id) {
            tab.reading.selection = selection.map(Box::new);
        }
    }

    /// A tab's facts were, or are about to be, read with a password the person typed. Marked before they're taken in, and kept while the tab's open and in the record, so there's no moment an agent could read them.
    pub(crate) fn note_password_used(&mut self, id: TabId) {
        if let Some(tab) = self.tab_mut(id) {
            tab.file.password = true;
        }
    }

    /// [`crate::Session::touch`], at a given time.
    pub(crate) fn touch_at(&mut self, id: TabId, now: u32) {
        if let Some(tab) = self.tab_mut(id) {
            tab.touched = now;
        }
    }

    /// Shows a tab's generated contents, or its own. The choice is kept in the record, and the file is never changed. What's folded goes, as it was about the other list's entries.
    pub(crate) fn choose_generated(&mut self, id: TabId, generated: bool) {
        if let Some(tab) = self.tab_mut(id)
            && tab.generated_contents != generated
        {
            tab.generated_contents = generated;
            tab.folds = None;
        }
    }

    /// A restore: no tabs, none active, every folder's number gone, and the next number the record's. Room for this many tabs.
    pub(crate) fn restored(&mut self, next_id: u32, tabs: usize) {
        self.tabs.clear();
        // Every tab's going, and with them every folder's number.
        self.dirs = Dirs::default();
        self.active = None;
        self.next_id = next_id;
        self.tabs.reserve_exact(tabs);
    }

    /// Every part that holds something of this tab, by name.
    #[cfg(test)]
    pub(crate) fn holding(&self, id: TabId) -> [(&'static str, bool); 2] {
        [("tabs", self.tab(id).is_some()), ("active", self.active == Some(id))]
    }
}

/// What a caller outside the crate may change of the tabs: a tab's layout, its view's turn, and that its file was opened with a password. Adding,
/// removing or reordering tabs stays the session's, as every other component holds something of each tab.
impl crate::parts::PartMut<'_, Tabs> {
    /// Turns the view of a tab, by quarter turns.
    pub fn set_view_rotation(&mut self, id: TabId, quarter_turns: u8) {
        if let Some(tab) = self.0.tab_mut(id) {
            tab.view_rotation = quarter_turns % 4;
        }
    }

    /// Lays a tab's pages out another way. Says whether that changed anything.
    pub fn set_spread(&mut self, id: TabId, spread: record::Spread) -> bool {
        match self.0.tab_mut(id) {
            Some(tab) if tab.spread != spread => {
                tab.spread = spread;
                true
            }
            _ => false,
        }
    }

    /// **The host's confinement cell, put back if a step it ran with the session in hand swapped another session in** (R3.5a): the host's cell is the
    /// authority, as it's what the host reads with no lock. Says whether it was the session's already, which is always so unless something replaced the
    /// whole session. Never widens: a cell that would let the session use more than it may now isn't taken (`Confinement::may_replace`), and the
    /// session keeps its own.
    pub fn keep_confinement(&mut self, host: &Confinement) -> bool {
        if self.0.confinement.is(host) {
            return true;
        }
        if host.may_replace(&self.0.confinement) {
            self.0.confinement = host.shared();
        }
        false
    }

    /// What's selected in the tab showing, as the page view says. Only that one holds a selection: one that's left has lost its.
    pub fn note_selection(&mut self, showing: TabId, selection: Option<Selection>) {
        for tab in self.0.tabs.iter_mut() {
            if tab.reading.selection.is_some() && tab.id != showing {
                tab.reading.selection = None;
            }
        }
        self.0.set_selection(showing, selection);
    }

    /// Shows a tab's generated contents, or its own: the tabs' own `choose_generated`, which the session's operations call too.
    pub fn choose_generated(&mut self, id: TabId, generated: bool) {
        self.0.choose_generated(id, generated);
    }

    /// A tab's facts were, or are about to be, read with a password the person typed: the tabs' own `note_password_used`, which the session's
    /// unlocking calls too.
    pub fn note_password_used(&mut self, id: TabId) {
        self.0.note_password_used(id);
    }
}
