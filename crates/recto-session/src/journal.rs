//! **The journal desk: what goes out of memory to the journals, and what comes back of them** (R3.5, the first of `Session`'s operation types). It
//! borrows exactly the components it changes, the journals and the lists, and reads the tabs, for one call: built by [`crate::Session::journal_desk`]
//! inside whatever already holds the session, with no state of its own. Its reads, which change nothing, are a look of their own
//! ([`crate::Session::journal_look`]), so a caller holding the session only to read needn't hold it to change.
//!
//! **What it owns:** a list's operations reach its journal once and in order, and a retired journal's file goes. What's waiting is copied out by
//! [`JournalDesk::work`], not taken, and goes from memory only when [`JournalDesk::outcome`] says it's in the file, so nothing's lost if the disk refuses
//! and nothing goes twice if one journal fails and another doesn't. Held by `tests/journal_desk.rs`, with the bytes of every write pinned.

use super::*;

/// What goes to the journals and what comes of it, borrowed from the session for one call: [`crate::Session::journal_desk`]. It hands out none of what
/// it borrows.
pub struct JournalDesk<'a> {
    pub(crate) journals: &'a mut Journals,
    pub(crate) lists: &'a mut Lists,
    pub(crate) tabs: &'a Tabs,
}

/// What the journals have to say, changing nothing, borrowed from the session for one look: [`crate::Session::journal_look`].
pub struct JournalLook<'a> {
    pub(crate) journals: &'a Journals,
    pub(crate) lists: &'a Lists,
    pub(crate) tabs: &'a Tabs,
}

/// The desk and the look are built for every call that goes to the journals, so they hold nothing but their borrows: no state, nothing allocated, and
/// `Session` no bigger for them. Failure caught: a field of its own added to either, which every call would then build.
const _: () = {
    assert!(std::mem::size_of::<JournalDesk<'static>>() == 3 * std::mem::size_of::<usize>());
    assert!(std::mem::size_of::<JournalLook<'static>>() == 3 * std::mem::size_of::<usize>());
};

impl JournalDesk<'_> {
    /// A tab's sleep file goes, as what's in it is done with: its tab's closed, or the file's been changed, and a redaction's what it would otherwise keep.
    pub(crate) fn retire_sleep_file(&mut self, id: TabId) {
        if let (Some(stamp), Some(path)) = (self.tabs.tab(id).and_then(|tab| tab.file.stamp), self.tabs.named_path(id)) {
            self.journals.retire_name(record::sleep_file_name(stamp, &path).into());
        }
    }

    /// What's waiting to go in journals: which to remove, and what to append to which. Copied, not taken: operations stay until [`Self::outcome`] says they're in the file, so nothing's lost if the disk refuses, and nothing goes twice if one journal fails and another doesn't. A list's first time here gets its journal's name.
    pub fn work(&mut self) -> JournalWork {
        let ids: Vec<TabId> = self
            .lists
            .iter()
            .filter(|(id, edits)| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()) || self.journals.is_fresh(**id))
            .map(|(id, _)| *id)
            .collect();
        let mut appends = self.journals.departing().to_vec();
        for id in ids {
            if let Some(append) = self.append_for(id) {
                appends.push(append);
            }
        }
        JournalWork {
            retired: self.journals.take_retired().into_iter().map(String::from).collect(),
            appends,
        }
    }

    pub(crate) fn append_for(&mut self, id: TabId) -> Option<JournalAppend> {
        let name = self.name_for(id);
        let (tab, path, edits) = (self.tabs.tab(id)?, self.tabs.named_path(id)?, self.lists.get(id)?);
        let (through, ops) = edits.journal_waiting()?;
        let mut header = JournalHeader::new(id.0, &path, tab.file.stamp, tab.page_count, 0);
        header.saving = edits.saving().map(Into::into);
        // A journal starting again has all of it in its first line, whatever's come since.
        if let Some(replaces) = self.journals.fresh_of(id) {
            header.keeping = edits.keeping().map(edit::AddedSource::to_record);
            return Some(JournalAppend {
                tab: id,
                name,
                header,
                through,
                ops: Vec::new(),
                closing: false,
                base: Some(edits.to_record()),
                replaces: replaces.clone(),
            });
        }
        Some(JournalAppend {
            tab: id,
            name,
            header,
            through,
            ops: ops.to_vec(),
            closing: false,
            base: None,
            replaces: None,
        })
    }

    /// What came of [`Self::work`]: what's in the file can go from here.
    pub fn outcome(&mut self, outcome: JournalOutcome) {
        for (id, through, name) in outcome.written {
            self.journals.written(id, through);
            if let Some(edits) = self.lists.get_mut(id) {
                edits.journal_written(through);
                self.journals.set_name(id, &name);
            }
        }
        for name in outcome.not_retired {
            self.journals.retire_name(name.into_boxed_str());
        }
    }

    /// Every list as a whole, with its journal's name, for a session that keeps no journal as it goes and is told to save.
    pub fn checkpoints(&mut self) -> Vec<(String, JournalHeader, record::EditsRecord)> {
        let ids: Vec<TabId> = self.lists.ids().collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let name = self.name_for(id);
            let (Some(tab), Some(path), Some(list)) = (self.tabs.tab(id), self.tabs.named_path(id), self.lists.edits_record(id)) else {
                continue;
            };
            out.push((name, JournalHeader::new(id.0, &path, tab.file.stamp, tab.page_count, 0), list));
        }
        out
    }

    /// A journal's name for this tab, from its PDF's identity.
    pub(crate) fn name_for(&mut self, id: TabId) -> String {
        if let Some(name) = self.journals.name_of(id) {
            return name.to_string();
        }
        let (stamp, path) = (self.tabs.tab(id).and_then(|tab| tab.file.stamp), self.tabs.named_path(id).unwrap_or_default());
        let name = record::journal_file_name(stamp, &path, None);
        self.journals.set_name(id, &name);
        name
    }

    /// A PDF opened again picks up the history it was left with. A tab with a list of its own keeps that, and a journal has one owner.
    pub fn take_up_history(&mut self, id: TabId, name: &str, mut edits: PageEdits, seq: u64) {
        if self.lists.contains(id) || self.tabs.tab(id).is_none() || self.journals.is_taken(name) {
            return;
        }
        if self.journals.journaling() {
            edits.set_journal_seq(seq);
        } else {
            edits.no_journal();
        }
        self.lists.insert(id, edits);
        self.journals.set_name(id, name);
    }
}

impl JournalLook<'_> {
    /// A list as a whole for a journal's new checkpoint, with its header. Only if nothing's still waiting to go in the file, which the checkpoint would pass.
    pub fn checkpoint_if_caught_up(&self, id: TabId) -> Option<(JournalHeader, record::EditsRecord)> {
        let edits = self.lists.get(id)?;
        let (seq, waiting) = edits.journal_waiting()?;
        if !waiting.is_empty() || self.journals.is_fresh(id) {
            return None;
        }
        let tab = self.tabs.tab(id)?;
        let mut header = JournalHeader::new(id.0, &self.tabs.named_path(id)?, tab.file.stamp, tab.page_count, seq);
        // A save may be about to land, and a checkpoint mustn't lose that it said so.
        header.saving = edits.saving().map(Into::into);
        header.keeping = edits.keeping().map(edit::AddedSource::to_record);
        Some((header, edits.to_record()))
    }

    /// Every list in full by its journal's name, for reading back.
    pub fn lists_by_journal(&self) -> HashMap<String, record::EditsRecord> {
        self.journals
            .names()
            .filter_map(|(id, name)| Some((name.to_string(), self.lists.get(*id)?.to_record())))
            .collect()
    }

    /// Whether there's anything for [`JournalDesk::work`], which is asked often and costs a look per edited tab.
    pub fn has_work(&self) -> bool {
        self.journals.has_pending()
            || self
                .lists
                .iter()
                .any(|(_, edits)| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()))
    }
}
