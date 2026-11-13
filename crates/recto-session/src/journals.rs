//! Journals: each edited tab's journal by name, those that have to start again from a whole list, those to remove, those of tabs that have closed, and where a PDF's history is read from.

use crate::{HistorySource, JournalAppend, TabId};
use std::collections::HashMap;
use std::sync::Arc;

/// What's kept of the journals, and what's waiting to go out to them.
#[derive(Default)]
pub(crate) struct Journals {
    /// Whether edits go to a journal. A session with a state record has this, and one without has nothing writing them out.
    journaling: bool,
    /// Each edited tab's journal, by name. Nothing for a tab with no edits.
    journals: HashMap<TabId, Box<str>>,
    /// Lists whose journal has to start again from the whole list: which tab, as of which count, and the journal it takes over from. Next to none, and only until they're written.
    fresh_journals: Vec<(TabId, u64, Option<String>)>,
    /// Journals to remove, as what's in them has been discarded or saved.
    retired_journals: Vec<Box<str>>,
    /// Journals of tabs that have closed, with what hadn't gone out yet. Nothing's lost by closing.
    departing: Vec<JournalAppend>,
    /// Where a PDF's history is, for one being opened.
    history: Option<Arc<dyn HistorySource>>,
}

impl Journals {
    pub(crate) fn journaling(&self) -> bool {
        self.journaling
    }

    pub(crate) fn set_journaling(&mut self, on: bool) {
        self.journaling = on;
    }

    pub(crate) fn history(&self) -> Option<&Arc<dyn HistorySource>> {
        self.history.as_ref()
    }

    pub(crate) fn set_history(&mut self, source: Arc<dyn HistorySource>) {
        self.history = Some(source);
    }

    /// Journals are kept as edits come, and there's somewhere to read them back from: a list can go to disk.
    pub(crate) fn kept_and_readable(&self) -> bool {
        self.journaling && self.history.is_some()
    }

    /// A tab's journal's name, if it has one.
    pub(crate) fn name_of(&self, id: TabId) -> Option<&str> {
        self.journals.get(&id).map(|name| &**name)
    }

    pub(crate) fn set_name(&mut self, id: TabId, name: &str) {
        self.journals.insert(id, name.into());
    }

    /// A tab no longer has this journal, which isn't removed: it's left for someone else to read.
    pub(crate) fn forget_name(&mut self, id: TabId) -> Option<Box<str>> {
        self.journals.remove(&id)
    }

    /// Whether some tab has this journal, which has one owner.
    pub(crate) fn is_taken(&self, name: &str) -> bool {
        self.journals.values().any(|taken| **taken == *name)
    }

    /// Every journal by its tab.
    pub(crate) fn names(&self) -> impl Iterator<Item = (&TabId, &Box<str>)> {
        self.journals.iter()
    }

    /// What's in a tab's journal is done with, so its file goes.
    pub(crate) fn retire(&mut self, id: TabId) {
        if let Some(name) = self.journals.remove(&id) {
            self.retired_journals.push(name);
        }
    }

    /// A file of this name goes: a journal, a sleep file or a file kept for a history.
    pub(crate) fn retire_name(&mut self, name: Box<str>) {
        self.retired_journals.push(name);
    }

    /// Everything to remove, taken.
    pub(crate) fn take_retired(&mut self) -> Vec<Box<str>> {
        std::mem::take(&mut self.retired_journals)
    }

    /// A tab's journal starts again from its whole list, as of `seq`, taking over from `replaces`.
    pub(crate) fn start_fresh(&mut self, id: TabId, seq: u64, replaces: Option<String>) {
        self.fresh_journals.retain(|(tab, _, _)| *tab != id);
        self.fresh_journals.push((id, seq, replaces));
    }

    /// A tab's journal no longer starts again.
    pub(crate) fn drop_fresh(&mut self, id: TabId) {
        self.fresh_journals.retain(|(tab, _, _)| *tab != id);
    }

    /// A tab's journal starts again no more, and the one it was to take over from goes too.
    pub(crate) fn let_go_of_fresh(&mut self, id: TabId) {
        self.fresh_journals.retain(|(tab, _, replaces)| {
            if *tab == id {
                if let Some(old) = replaces {
                    self.retired_journals.push(old.as_str().into());
                }
                return false;
            }
            true
        });
    }

    /// If a tab's journal starts again, the one it takes over from, if any.
    pub(crate) fn fresh_of(&self, id: TabId) -> Option<&Option<String>> {
        self.fresh_journals.iter().find(|(tab, _, _)| *tab == id).map(|(_, _, replaces)| replaces)
    }

    pub(crate) fn is_fresh(&self, id: TabId) -> bool {
        self.fresh_journals.iter().any(|(tab, _, _)| *tab == id)
    }

    /// A closed tab's journal, with what hadn't gone out yet.
    pub(crate) fn depart(&mut self, append: JournalAppend) {
        self.departing.push(append);
    }

    pub(crate) fn departing(&self) -> &[JournalAppend] {
        &self.departing
    }

    /// A tab's journal is in its file up to `through`: what was waiting to go there of it is done.
    pub(crate) fn written(&mut self, id: TabId, through: u64) {
        self.departing.retain(|append| !(append.tab == id && append.through == through));
        self.fresh_journals.retain(|(tab, seq, _)| !(*tab == id && *seq <= through));
    }

    /// Whether a journal's to be removed, a closed tab's to be written, or one's to start again, which wants writing at once.
    pub(crate) fn has_pending(&self) -> bool {
        !self.retired_journals.is_empty() || !self.departing.is_empty() || !self.fresh_journals.is_empty()
    }

    /// A restore: no tab has a journal till the record says, and none starts again. What's to be removed or written stays: it's about files.
    pub(crate) fn restored(&mut self) {
        self.journals.clear();
        self.fresh_journals.clear();
    }

    /// Every part that holds something of this tab, by name.
    #[cfg(test)]
    pub(crate) fn holding(&self, id: TabId) -> [(&'static str, bool); 3] {
        [
            ("journals", self.journals.contains_key(&id)),
            ("fresh_journals", self.is_fresh(id)),
            ("departing", self.departing.iter().any(|append| append.tab == id)),
        ]
    }
}
