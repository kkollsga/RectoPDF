//! **The sleeper: a tab going dormant and waking** (SPEC decision 25; R3.5, the second of `Session`'s operation types). Owner, 2026-11-07: "as it goes
//! dormant we store to disk", and "When it's on deep sleep, we want its state to be stored on disk, not on memory. But it needs to be stored in a way
//! that's quick to recover without any delay." It borrows exactly the components going dormant and waking change, for one call: built by
//! [`crate::Session::sleeper`] inside whatever already holds the session, with no state of its own. What it reads, which changes nothing, is the
//! dormancy look ([`crate::Dormant`], [`crate::Session::dormant`]): which tabs may go and when, what a sleep file would say, what's on hand to wake one.
//!
//! **What it owns:** a tab is warm, cold with a sleep file on its way, or cold, and never two of them at once; a list goes to sleep only when all of it
//! is in its journal, so a list asleep reads back from its journal as what's kept of it; and one that can't be read back stays asleep, under its name,
//! with the tab saying so, till a discard or a confirmed save gives up on it. Held by `tests/sleeper.rs`, with what each step says and leaves pinned.

use super::*;
use crate::compare::Comparison;
use crate::dormant::Dormant;

/// A tab going dormant and waking, borrowed from the session for one call: [`crate::Session::sleeper`]. It hands out none of what it borrows.
pub struct Sleeper<'a> {
    /// A tab's notice, its `touched`, `from_sleep` and facts as a wake gives them.
    pub(crate) tabs: &'a mut registry::Tabs,
    /// Lists put to sleep, woken, or given up on.
    pub(crate) lists: &'a mut lists::Lists,
    /// A woken list's journal starting again, a list given up on, and a sleep file retired.
    pub(crate) journals: &'a mut journals::Journals,
    /// A dormant tab's contents and any read of its file go, and a read for a wake is kept.
    pub(crate) warm: &'a mut warm::Warm,
    /// Holds let go of, what's leaving the warm ones taken, and a wake from a sleep file noted.
    pub(crate) dormancy: &'a mut dormancy::Dormancy,
    /// A dormant tab holds no generated contents.
    pub(crate) generated: &'a mut generated::Generated,
    /// What the page view drew of a dormant tab, and what it was told, go.
    pub(crate) mail: &'a mut mail::PageViewMail,
    /// Read only, for whether a tab's on screen and so may not go.
    pub(crate) comparison: Option<&'a Comparison>,
}

/// Built for every call that sends a tab to sleep or wakes one, so it holds nothing but its borrows: no state, nothing allocated, and `Session` no
/// bigger for it. Failure caught: a field of its own added, which every call would then build.
const _: () = assert!(std::mem::size_of::<Sleeper<'static>>() == 8 * std::mem::size_of::<usize>());

impl Sleeper<'_> {
    /// The dormancy look, over what this borrows.
    fn look(&self) -> Dormant<'_> {
        Dormant {
            tabs: &*self.tabs,
            warm: &*self.warm,
            lists: &*self.lists,
            dormancy: &*self.dormancy,
            journals: &*self.journals,
            comparison: self.comparison,
        }
    }

    /// Somebody's using this tab, so it's not to go dormant for a while. Agents' commands say so too.
    pub fn touch(&mut self, id: TabId) {
        let now = self.dormancy.seconds_now();
        self.tabs.touch_at(id, now);
    }

    pub fn let_go(&mut self, id: TabId) {
        self.dormancy.let_go(id);
        self.touch(id);
    }

    /// **A tab goes dormant, SPEC decision 25: it costs a cold tab's memory from here on.** Its contents go, and its list goes if [`Dormant::list_to_sleep`] says it can, as all of it's on disk. Said once for each, and read back only if it's touched again. Says whether the list went, or there wasn't one.
    pub fn go_dormant(&mut self, id: TabId) -> bool {
        // A dormant tab holds no list: its generated contents are on disk, and found again when it's shown.
        self.generated.forget(id);
        if !self.look().may_go(id) {
            return false;
        }
        self.warm.forget(id);
        self.mail.forget_draws(id);
        self.mail.forget_view_word(id);
        !self.lists.contains(id) || self.put_list_to_sleep(id)
    }

    pub(crate) fn put_list_to_sleep(&mut self, id: TabId) -> bool {
        if self.look().list_to_sleep(id).is_none() {
            return false;
        }
        self.lists.put_to_sleep(id)
    }

    /// **Brings a tab's list back from its journal, if it's on disk.** Everything about the tab comes here first, so nothing can act on a list it can't see. **If it can't be read just now, it stays asleep, its name kept, to be tried again by whatever's next,** and says false. Nothing goes on that hasn't got a list to go on: a new edit laid on a journal it never read would land on the wrong pages. The tab says so till then. Only a discard or a confirmed save gives up on it.
    pub fn wake_list(&mut self, id: TabId) -> bool {
        let Some(summary) = self.lists.asleep_summary(id) else { return true };
        let restored = self.journals.name_of(id).and_then(|name| self.journals.history()?.read_list(name));
        match restored {
            Some(restored) if (restored.edits.summary() == summary || restored.rolled_off) && self.tabs.confinement().admits_edits(&restored.edits) => {
                let mut edits = restored.edits;
                edits.set_journal_seq(restored.seq);
                let said_unreadable = self.lists.woken(id, edits);
                if let Some(tab) = self.tabs.tab_mut(id) {
                    if said_unreadable && matches!(tab.file.notice(), Some(FileNotice::EditsUnreadable)) {
                        tab.file.set_notice(None);
                    }
                    if restored.rolled_off {
                        tab.file.set_notice(Some(FileNotice::HistoryRolledOff));
                    }
                }
                // Steps have gone that the journal has, so it has to start again from the list, or it wouldn't read back as this.
                if restored.rolled_off {
                    self.journals.start_fresh(id, restored.seq, None);
                }
                true
            }
            _ => {
                self.lists.wake_failed(id);
                if let Some(tab) = self.tabs.tab_mut(id) {
                    tab.file.set_notice(Some(FileNotice::EditsUnreadable));
                }
                false
            }
        }
    }

    /// A list that couldn't be read is given up on, as what's being done says it's done with: its journal goes.
    pub(crate) fn give_up_on_asleep(&mut self, id: TabId) {
        if self.lists.is_asleep(id) {
            self.lists.forget_asleep(id);
            self.journals.retire(id);
        }
    }

    /// **Wakes a cold tab from its sleep file, if it has one about its file as last known.** Says whether it did. Every route to a cold tab's through here, or through a read of its PDF if not.
    pub fn wake_from_sleep_file(&mut self, id: TabId) -> bool {
        if !self.dormancy.sleep_files() {
            return false;
        }
        let Some((stamp, path)) = self.look().sleep_to_read(id) else { return false };
        let Some(sleep) = self.journals.history().and_then(|source| source.read_sleep(stamp, &path)) else {
            return false;
        };
        self.wake_from_sleep(id, &sleep);
        self.dormancy.woke_from_file(id, sleep);
        true
    }

    pub(crate) fn wake_from_sleep(&mut self, id: TabId, sleep: &sleep::SleepFile) {
        let facts = DocFacts {
            page_count: sleep.file_pages,
            byte_len: sleep.file.size,
            encrypted: false,
            version: "".into(),
            title: None,
            toc: sleep.toc.clone(),
            renderer_only: sleep.renderer_only,
        };
        self.give_woken(id, Ok(facts), Some(sleep.file));
        if let Some(tab) = self.tabs.tab_mut(id) {
            tab.from_sleep = Some(sleep.version.unwrap_or(0));
        }
    }

    /// What was read for [`crate::Session::wake_for_tap`] or [`crate::Session::wake_for_close`], kept for that tab's waking. A tab closed or already woken while it was being read has no use for it. Several can be read at once, so they don't take each other's place, and the oldest goes past the cap.
    pub fn give_woken(&mut self, id: TabId, loaded: Result<DocFacts, OpenError>, stamp: Option<FileStamp>) {
        self.warm.drop_read(id);
        if self.look().file_to_wake(id).is_none() {
            return;
        }
        self.warm.keep_read(id, loaded, stamp);
    }

    /// **The worker has the file open, and says how long it is.** A sleep file's word goes only until now. If it's said something else, it's gone, and the file's read: says so, for that to be done. A list's own length is compared with what that read finds, on its way in. A file read in the usual way isn't looked at, as the two readers of a damaged PDF can differ, and neither is wrong.
    pub fn file_length_is(&mut self, id: TabId, pages: u32, version: u64) -> bool {
        let Some(tab) = self.tabs.tab_mut(id) else { return false };
        let Some(said) = tab.from_sleep.take() else { return false };
        // The same length and, if it said, the same version: it was right. A sleep file that didn't say is right about the length only, which is all it's been used for, its pictures having gone if they weren't.
        if tab.page_count == pages && (said == 0 || said == version) {
            return false;
        }
        journal::JournalDesk {
            journals: &mut *self.journals,
            lists: &mut *self.lists,
            tabs: &*self.tabs,
        }
        .retire_sleep_file(id);
        self.forget_read(id);
        true
    }

    /// What was read of a tab goes, so it's read again: its file's turned out to be something else.
    pub fn forget_read(&mut self, id: TabId) {
        self.generated.forget(id);
        // What it left the warm ones with is of what was read, so it goes too, or it'd be written into a sleep file under the same stamp.
        self.dormancy.not_leaving(id);
        if let Some(tab) = self.tabs.tab_mut(id) {
            tab.from_sleep = None;
        }
        self.warm.forget(id);
    }

    /// Tabs just pushed out of the warm ones, what their sleep files would say, and whose command pushed each out. There's a sleep file for any tab that's gone
    /// cold, by whatever way.
    pub fn take_left_warm(&mut self) -> Vec<(SleepFacts, crate::Author)> {
        let left = self.dormancy.take_left();
        left.into_iter()
            .filter_map(|(id, toc, by)| Some((self.look().sleep_facts_with(id, &toc)?, by)))
            .collect()
    }
}
