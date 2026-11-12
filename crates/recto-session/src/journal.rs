//! Journals: what goes out of memory to the journals, fresh, retired and departing ones, and the history source.

use super::*;

impl<L: DocLoader> Session<L> {
    /// A tab's sleep file goes, as what's in it is done with: its tab's closed, or the file's been changed, and a redaction's what it would otherwise keep.
    pub(crate) fn retire_sleep_file(&mut self, id: TabId) {
        if let (Some(stamp), Some(path)) = (self.tab(id).and_then(|tab| tab.stamp), self.path_of(id)) {
            self.retired_journals.push(record::sleep_file_name(stamp, &path).into());
        }
    }

    pub(crate) fn retire_kept(&mut self, kept: Vec<Box<Path>>) {
        for path in kept {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()).filter(|name| record::is_kept_name(name)) {
                self.retired_journals.push(name.into());
            }
        }
    }

    /// What's in a tab's journal is done with, by a discard or a save, so its file goes: what's been removed mustn't stay recoverable.
    pub(crate) fn retire_journal(&mut self, id: TabId) {
        if let Some(name) = self.journals.remove(&id) {
            self.retired_journals.push(name);
        }
    }

    /// Whether edits go to a journal from now on. For whoever keeps the state record.
    pub fn set_journaling(&mut self, on: bool) {
        self.journaling = on;
    }

    /// An agent's next edits are at Full, where the owner's history can be pushed out, or not.
    pub fn set_agent_at_full(&mut self, full: bool) {
        self.agent_at_full = full;
    }

    /// What's waiting to go in journals: which to remove, and what to append to which. Copied, not taken: operations stay until [`Self::journal_outcome`] says they're in the file, so nothing's lost if the disk refuses, and nothing goes twice if one journal fails and another doesn't. A list's first time here gets its journal's name.
    pub fn journal_work(&mut self) -> JournalWork {
        let ids: Vec<TabId> = self
            .edits
            .iter()
            .filter(|(id, edits)| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()) || self.fresh_journals.iter().any(|(tab, _, _)| tab == *id))
            .map(|(id, _)| *id)
            .collect();
        let mut appends = self.departing.clone();
        for id in ids {
            if let Some(append) = self.append_for(id) {
                appends.push(append);
            }
        }
        JournalWork {
            retired: std::mem::take(&mut self.retired_journals).into_iter().map(String::from).collect(),
            appends,
        }
    }

    pub(crate) fn append_for(&mut self, id: TabId) -> Option<JournalAppend> {
        let name = self.journal_name_for(id);
        let (tab, path, edits) = (self.tab(id)?, self.path_of(id)?, self.edits.get(&id)?);
        let (through, ops) = edits.journal_waiting()?;
        let mut header = JournalHeader::new(id.0, &path, tab.stamp, tab.page_count, 0);
        header.saving = edits.saving().map(Into::into);
        // A journal starting again has all of it in its first line, whatever's come since.
        if let Some((_, _, replaces)) = self.fresh_journals.iter().find(|(tab, _, _)| *tab == id) {
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
            self.departing.retain(|append| !(append.tab == id && append.through == through));
            self.fresh_journals.retain(|(tab, seq, _)| !(*tab == id && *seq <= through));
            if let Some(edits) = self.edits.get_mut(&id) {
                edits.journal_written(through);
                self.journals.insert(id, name.into());
            }
        }
        self.retired_journals.extend(outcome.not_retired.into_iter().map(String::into_boxed_str));
    }

    /// A list as a whole for a journal's new checkpoint, with its header. Only if nothing's still waiting to go in the file, which the checkpoint would pass.
    pub fn checkpoint_if_caught_up(&self, id: TabId) -> Option<(JournalHeader, record::EditsRecord)> {
        let edits = self.edits.get(&id)?;
        let (seq, waiting) = edits.journal_waiting()?;
        if !waiting.is_empty() || self.fresh_journals.iter().any(|(tab, _, _)| *tab == id) {
            return None;
        }
        let tab = self.tab(id)?;
        let mut header = JournalHeader::new(id.0, &self.path_of(id)?, tab.stamp, tab.page_count, seq);
        // A save may be about to land, and a checkpoint mustn't lose that it said so.
        header.saving = edits.saving().map(Into::into);
        header.keeping = edits.keeping().map(edit::AddedSource::to_record);
        Some((header, edits.to_record()))
    }

    /// Every list as a whole, with its journal's name, for a session that keeps no journal as it goes and is told to save.
    pub fn checkpoints(&mut self) -> Vec<(String, JournalHeader, record::EditsRecord)> {
        let ids: Vec<TabId> = self.edits.keys().copied().collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let name = self.journal_name_for(id);
            let (Some(tab), Some(path), Some(list)) = (self.tab(id), self.path_of(id), self.edits_record(id)) else {
                continue;
            };
            out.push((name, JournalHeader::new(id.0, &path, tab.stamp, tab.page_count, 0), list));
        }
        out
    }

    /// Every list in full by its journal's name, for reading back.
    pub fn lists_by_journal(&self) -> HashMap<String, record::EditsRecord> {
        self.journals
            .iter()
            .filter_map(|(id, name)| Some((name.to_string(), self.edits.get(id)?.to_record())))
            .collect()
    }

    /// Whether there's anything for [`Self::journal_work`], which is asked often and costs a look per edited tab.
    pub fn has_journal_work(&self) -> bool {
        !self.retired_journals.is_empty()
            || !self.departing.is_empty()
            || !self.fresh_journals.is_empty()
            || self.edits.values().any(|edits| edits.journal_waiting().is_some_and(|(_, ops)| !ops.is_empty()))
    }

    /// Whether a journal has been retired, or a tab with one has closed, which wants writing at once.
    pub fn has_retired_journals(&self) -> bool {
        !self.retired_journals.is_empty() || !self.departing.is_empty() || !self.fresh_journals.is_empty()
    }

    /// Those operations are in the file.
    pub fn journal_written(&mut self, id: TabId, through: u64) {
        if let Some(edits) = self.edits.get_mut(&id) {
            edits.journal_written(through);
        }
    }

    /// A journal's name for this tab, from its PDF's identity.
    pub(crate) fn journal_name_for(&mut self, id: TabId) -> String {
        if let Some(name) = self.journals.get(&id) {
            return name.to_string();
        }
        let (stamp, path) = (self.tab(id).and_then(|tab| tab.stamp), self.path_of(id).unwrap_or_default());
        let name = record::journal_file_name(stamp, &path, None);
        self.journals.insert(id, name.as_str().into());
        name
    }

    /// Where a PDF's history is read from, for one being opened on any route.
    pub fn set_history_source(&mut self, source: Arc<dyn HistorySource>) {
        self.history = Some(source);
    }

    /// Reads what's left of a PDF's history, for [`Self::open_with_facts`], with the session free: hold this and call it outside. `None` if there's no such thing here.
    pub fn history_source(&self) -> Option<Arc<dyn HistorySource>> {
        self.history.clone()
    }

    /// A PDF opened again picks up the history it was left with. A tab with a list of its own keeps that, and a journal has one owner.
    pub fn take_up_history(&mut self, id: TabId, name: &str, mut edits: PageEdits, seq: u64) {
        if self.edits.contains_key(&id) || self.tab(id).is_none() || self.journals.values().any(|taken| **taken == *name) {
            return;
        }
        if self.journaling {
            edits.set_journal_seq(seq);
        } else {
            edits.no_journal();
        }
        self.edits.insert(id, edits);
        self.journals.insert(id, name.into());
    }

    /// The name of this tab's journal, if it has one, for checking what's reached the disk.
    pub fn journal_of(&self, id: TabId) -> Option<&str> {
        self.journals.get(&id).map(|name| &**name)
    }
}
