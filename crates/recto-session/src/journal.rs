//! Journals: what goes out of memory to the journals, fresh, retired and departing ones, and the history source.

use super::*;

impl<L: DocLoader> Session<L> {
    /// A tab's sleep file goes, as what's in it is done with: its tab's closed, or the file's been changed, and a redaction's what it would otherwise keep.
    pub(crate) fn retire_sleep_file(&mut self, id: TabId) {
        if let (Some(stamp), Some(path)) = (self.tab(id).and_then(|tab| tab.file.stamp), self.path_of(id)) {
            self.journals.retire_name(record::sleep_file_name(stamp, &path).into());
        }
    }

    pub(crate) fn retire_kept(&mut self, kept: Vec<Box<Path>>) {
        for path in kept {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()).filter(|name| record::is_kept_name(name)) {
                self.journals.retire_name(name.into());
            }
        }
    }

    /// What's in a tab's journal is done with, by a discard or a save, so its file goes: what's been removed mustn't stay recoverable.
    pub(crate) fn retire_journal(&mut self, id: TabId) {
        self.journals.retire(id);
    }

    /// Whether edits go to a journal from now on. For whoever keeps the state record.
    pub fn set_journaling(&mut self, on: bool) {
        self.journals.set_journaling(on);
    }

    /// An agent's next edits are at Full, where the owner's history can be pushed out, or not.
    pub fn set_agent_at_full(&mut self, full: bool) {
        self.lists.set_agent_at_full(full);
    }

    /// What's waiting to go in journals: which to remove, and what to append to which. Copied, not taken: operations stay until [`Self::journal_outcome`] says they're in the file, so nothing's lost if the disk refuses, and nothing goes twice if one journal fails and another doesn't. A list's first time here gets its journal's name.
    pub fn journal_work(&mut self) -> JournalWork {
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
        let name = self.journal_name_for(id);
        let (tab, path, edits) = (self.tab(id)?, self.path_of(id)?, self.lists.get(id)?);
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

    /// What came of [`Self::journal_work`]: what's in the file can go from here.
    pub fn journal_outcome(&mut self, outcome: JournalOutcome) {
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

    /// A list as a whole for a journal's new checkpoint, with its header. Only if nothing's still waiting to go in the file, which the checkpoint would pass.
    pub fn checkpoint_if_caught_up(&self, id: TabId) -> Option<(JournalHeader, record::EditsRecord)> {
        let edits = self.lists.get(id)?;
        let (seq, waiting) = edits.journal_waiting()?;
        if !waiting.is_empty() || self.journals.is_fresh(id) {
            return None;
        }
        let tab = self.tab(id)?;
        let mut header = JournalHeader::new(id.0, &self.path_of(id)?, tab.file.stamp, tab.page_count, seq);
        // A save may be about to land, and a checkpoint mustn't lose that it said so.
        header.saving = edits.saving().map(Into::into);
        header.keeping = edits.keeping().map(edit::AddedSource::to_record);
        Some((header, edits.to_record()))
    }

    /// Every list as a whole, with its journal's name, for a session that keeps no journal as it goes and is told to save.
    pub fn checkpoints(&mut self) -> Vec<(String, JournalHeader, record::EditsRecord)> {
        let ids: Vec<TabId> = self.lists.ids().collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let name = self.journal_name_for(id);
            let (Some(tab), Some(path), Some(list)) = (self.tab(id), self.path_of(id), self.edits_record(id)) else {
                continue;
            };
            out.push((name, JournalHeader::new(id.0, &path, tab.file.stamp, tab.page_count, 0), list));
        }
        out
    }

    /// Every list in full by its journal's name, for reading back.
    pub fn lists_by_journal(&self) -> HashMap<String, record::EditsRecord> {
        self.journals
            .names()
            .filter_map(|(id, name)| Some((name.to_string(), self.lists.get(*id)?.to_record())))
            .collect()
    }

    /// Whether there's anything for [`Self::journal_work`], which is asked often and costs a look per edited tab.
    pub fn has_journal_work(&self) -> bool {
        self.journals.has_pending()
            || self
                .lists
                .iter()
                .any(|(_, edits)| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()))
    }

    /// Whether a journal has been retired, or a tab with one has closed, which wants writing at once.
    pub fn has_retired_journals(&self) -> bool {
        self.journals.has_pending()
    }

    /// Those operations are in the file.
    pub fn journal_written(&mut self, id: TabId, through: u64) {
        if let Some(edits) = self.lists.get_mut(id) {
            edits.journal_written(through);
        }
    }

    /// A journal's name for this tab, from its PDF's identity.
    pub(crate) fn journal_name_for(&mut self, id: TabId) -> String {
        if let Some(name) = self.journals.name_of(id) {
            return name.to_string();
        }
        let (stamp, path) = (self.tab(id).and_then(|tab| tab.file.stamp), self.path_of(id).unwrap_or_default());
        let name = record::journal_file_name(stamp, &path, None);
        self.journals.set_name(id, &name);
        name
    }

    /// Where a PDF's history is read from, for one being opened on any route.
    pub fn set_history_source(&mut self, source: Arc<dyn HistorySource>) {
        self.journals.set_history(source);
    }

    /// Reads what's left of a PDF's history, for [`Self::open_with_facts`], with the session free: hold this and call it outside. `None` if there's no such thing here.
    pub fn history_source(&self) -> Option<Arc<dyn HistorySource>> {
        self.journals.history().cloned()
    }

    /// A PDF opened again picks up the history it was left with. A tab with a list of its own keeps that, and a journal has one owner.
    pub fn take_up_history(&mut self, id: TabId, name: &str, mut edits: PageEdits, seq: u64) {
        if self.lists.contains(id) || self.tab(id).is_none() || self.journals.is_taken(name) {
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

    /// The name of this tab's journal, if it has one, for checking what's reached the disk.
    pub fn journal_of(&self, id: TabId) -> Option<&str> {
        self.journals.name_of(id)
    }
}
