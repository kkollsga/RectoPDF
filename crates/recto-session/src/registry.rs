//! The open tabs: each tab's record in the sidebar's order, which one is active, the number the next one gets, and the folders their files are in.

use crate::dirs::Dirs;
use crate::{TabId, TabRecord};
use std::path::{Path, PathBuf};

/// Every open tab, which one's active, and their folders.
#[derive(Debug)]
pub(crate) struct Tabs {
    /// In the sidebar's order.
    tabs: Vec<TabRecord>,
    active: Option<TabId>,
    /// The number the next tab opened gets.
    next_id: u32,
    /// The folders tabs' files are in.
    dirs: Dirs,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: None,
            next_id: 1,
            dirs: Dirs::default(),
        }
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

    pub(crate) fn len(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) fn get(&self, id: TabId) -> Option<&TabRecord> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: TabId) -> Option<&mut TabRecord> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Where a tab is in the sidebar's order.
    pub(crate) fn position(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
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

    pub(crate) fn active(&self) -> Option<TabId> {
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

    /// The tab's full path, rebuilt from its interned folder and name.
    pub(crate) fn path_of(&self, id: TabId) -> Option<PathBuf> {
        let tab = self.get(id)?;
        Some(self.dirs.get(tab.file.dir)?.join(&*tab.file.file_name))
    }

    /// The tab already showing this file, by its path.
    pub(crate) fn for_path(&self, path: &Path) -> Option<TabId> {
        let dir = self.dirs.id_of(path.parent().unwrap_or(Path::new("")))?;
        let file_name = path.file_name()?;
        self.tabs.iter().find(|t| t.file.dir == dir && &*t.file.file_name == file_name).map(|t| t.id)
    }

    /// A folder no tab is in any more is let go of, so the folders kept are only those in use.
    pub(crate) fn let_go_of_dir_if_unused(&mut self, dir: u32) {
        if !self.tabs.iter().any(|tab| tab.file.dir == dir) {
            self.dirs.let_go(dir);
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
        [("tabs", self.get(id).is_some()), ("active", self.active == Some(id))]
    }
}
