//! The edit lists of tabs: those in memory, those gone to disk with only their summary kept, those that couldn't be read back, and whether an agent's at Full. What's saved of a tab, and whose an edit is, is decided from here (rule 5): the answers that need a list in memory and one asleep alike are these.

use crate::TabId;
use crate::edit::{Author, PageEdits};
use crate::record::EditsSummary;
use std::collections::HashMap;

/// Every tab's list of edits, wherever it is.
#[derive(Default)]
pub(crate) struct Lists {
    /// Only for tabs with page edits. A tab with none costs nothing here.
    edits: HashMap<TabId, PageEdits>,
    /// Lists that couldn't be read back, to try again, and whose tab says so meanwhile.
    wake_failed: Vec<TabId>,
    /// Lists that have gone to disk, SPEC decision 25: all there is of them in memory is what a look at the tab needs, a cold tab's cost. Woken by [`crate::Session::wake_list`] from their journal.
    asleep: HashMap<TabId, EditsSummary>,
    /// An agent is acting at Full, so losing the owner's history is the holder's call. Set for each command.
    agent_at_full: bool,
}

impl Lists {
    /// A tab's list, if it's in memory.
    pub(crate) fn get(&self, id: TabId) -> Option<&PageEdits> {
        self.edits.get(&id)
    }

    pub(crate) fn get_mut(&mut self, id: TabId) -> Option<&mut PageEdits> {
        self.edits.get_mut(&id)
    }

    /// Whether a tab's list is in memory.
    pub(crate) fn contains(&self, id: TabId) -> bool {
        self.edits.contains_key(&id)
    }

    pub(crate) fn insert(&mut self, id: TabId, list: PageEdits) {
        self.edits.insert(id, list);
    }

    pub(crate) fn remove(&mut self, id: TabId) -> Option<PageEdits> {
        self.edits.remove(&id)
    }

    /// A tab's list, made for a file of this many pages if it has none.
    pub(crate) fn get_or_new(&mut self, id: TabId, page_count: u32) -> &mut PageEdits {
        self.edits.entry(id).or_insert_with(|| PageEdits::new(page_count))
    }

    /// Every tab with a list in memory.
    pub(crate) fn ids(&self) -> impl Iterator<Item = TabId> + '_ {
        self.edits.keys().copied()
    }

    /// Every list in memory, by its tab.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&TabId, &PageEdits)> {
        self.edits.iter()
    }

    /// How many lists are in memory.
    #[cfg(test)]
    pub(crate) fn in_memory(&self) -> usize {
        self.edits.len()
    }

    /// Whether a tab has a list of edits, in memory or on disk.
    pub(crate) fn has_list(&self, id: TabId) -> bool {
        self.edits.contains_key(&id) || self.asleep.contains_key(&id)
    }

    /// Whether a tab has unsaved changes.
    pub(crate) fn is_edited(&self, id: TabId) -> bool {
        self.edits.get(&id).is_some_and(PageEdits::is_edited) || self.asleep.get(&id).is_some_and(|summary| summary.edited)
    }

    /// Whether saving this tab's edits writes the file anew.
    pub(crate) fn needs_rewrite(&self, id: TabId) -> bool {
        match self.edits.get(&id) {
            Some(list) => list.rewrites(),
            None => self.asleep.get(&id).is_some_and(|summary| summary.edited && summary.rewrites),
        }
    }

    /// Whether the owner's edits on this tab have been undone and can still be redone.
    pub(crate) fn has_undone_owner_steps(&self, id: TabId) -> bool {
        self.edits.get(&id).is_some_and(PageEdits::owner_steps_undone) || self.asleep.get(&id).is_some_and(|summary| summary.owner_steps_undone)
    }

    /// Whether there's a step to undo and one to redo, wherever its list is.
    pub(crate) fn undo_and_redo(&self, id: TabId) -> (bool, bool) {
        self.edits
            .get(&id)
            .map(|list| (list.can_undo(), list.can_redo()))
            .or_else(|| self.asleep.get(&id).map(|summary| (summary.undo > 0, summary.redo > 0)))
            .unwrap_or((false, false))
    }

    /// Whether any list holds the owner's unsaved edits or their history.
    pub(crate) fn holds_owner_edits(&self) -> bool {
        self.edits.values().any(PageEdits::holds_owner_edits) || self.asleep.values().any(|summary| summary.holds_owner_edits)
    }

    /// How many pages a tab's list has, wherever it is.
    pub(crate) fn pages(&self, id: TabId) -> Option<u32> {
        self.edits
            .get(&id)
            .map(PageEdits::len)
            .or_else(|| self.asleep.get(&id).map(|summary| summary.pages))
    }

    /// What's kept of a tab's list for a look without opening it, wherever it is.
    pub(crate) fn summary(&self, id: TabId) -> Option<EditsSummary> {
        self.edits.get(&id).map(PageEdits::summary).or_else(|| self.asleep.get(&id).copied())
    }

    /// Whether a tab's list is on disk now.
    pub(crate) fn is_asleep(&self, id: TabId) -> bool {
        self.asleep.contains_key(&id)
    }

    /// What's known of a list that's on disk.
    pub(crate) fn asleep_summary(&self, id: TabId) -> Option<EditsSummary> {
        self.asleep.get(&id).copied()
    }

    /// Every list on disk, by its tab.
    pub(crate) fn asleep(&self) -> impl Iterator<Item = (&TabId, &EditsSummary)> {
        self.asleep.iter()
    }

    /// A tab's list goes to disk, with only its summary kept here. Says whether it had one in memory.
    pub(crate) fn put_to_sleep(&mut self, id: TabId) -> bool {
        let Some(list) = self.edits.remove(&id) else { return false };
        self.asleep.insert(id, list.summary());
        true
    }

    /// A list on disk was read back as this. Says whether its tab had been told it couldn't be.
    pub(crate) fn woken(&mut self, id: TabId, list: PageEdits) -> bool {
        self.asleep.remove(&id);
        self.edits.insert(id, list);
        let said_unreadable = self.wake_failed.contains(&id);
        self.wake_failed.retain(|failed| *failed != id);
        said_unreadable
    }

    /// A list on disk couldn't be read back, to be tried again.
    pub(crate) fn wake_failed(&mut self, id: TabId) {
        if !self.wake_failed.contains(&id) {
            self.wake_failed.push(id);
        }
    }

    /// A list on disk is given up on, and that it couldn't be read with it.
    pub(crate) fn forget_asleep(&mut self, id: TabId) {
        self.asleep.remove(&id);
        self.wake_failed.retain(|failed| *failed != id);
    }

    /// A restore: no tab has a list till the record says. Who an agent's acting as stays, as it's set for each command.
    pub(crate) fn restored(&mut self) {
        self.edits.clear();
        self.asleep.clear();
        self.wake_failed.clear();
    }

    pub(crate) fn set_agent_at_full(&mut self, full: bool) {
        self.agent_at_full = full;
    }

    /// Whether an edit by this author may push the owner's steps out of the history: an agent's, at Full.
    pub(crate) fn may_push_owner(&self, by: Author) -> bool {
        by == Author::Agent && self.agent_at_full
    }

    /// Every part that holds something of this tab, by name.
    #[cfg(test)]
    pub(crate) fn holding(&self, id: TabId) -> [(&'static str, bool); 3] {
        [
            ("edits", self.edits.contains_key(&id)),
            ("asleep", self.asleep.contains_key(&id)),
            ("wake_failed", self.wake_failed.contains(&id)),
        ]
    }
}
