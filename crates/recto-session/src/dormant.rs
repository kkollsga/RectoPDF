//! **Which tabs may go dormant, and when, and what goes to disk and comes back:** a look over the tabs, the warm ones, the lists, the holds, the journals
//! and the comparison, changing none of them. Going dormant and waking change several of them together, which is the sleeper's
//! ([`crate::Session::sleeper`]); this is what it and anyone else reads first.

use crate::compare::Comparison;
use crate::dormancy::Dormancy;
use crate::journals::Journals;
use crate::lists::Lists;
use crate::registry::Tabs;
use crate::warm::Warm;
use crate::{FileNotice, PageEdits, SleepFacts, TabId, fit, journal, record, sleep};
use recto_core::Toc;
use recto_platform::FileStamp;
use std::collections::HashMap;
use std::path::PathBuf;

/// What going dormant and waking go by, borrowed from the session for one look.
pub struct Dormant<'a> {
    pub(crate) tabs: &'a Tabs,
    pub(crate) warm: &'a Warm,
    pub(crate) lists: &'a Lists,
    pub(crate) dormancy: &'a Dormancy,
    pub(crate) journals: &'a Journals,
    pub(crate) comparison: Option<&'a Comparison>,
}

/// The look is built on every look at dormancy, as each command and each wait does, so it holds nothing but its borrows: no state, nothing allocated.
/// Failure caught: a field of its own added to it, which every look would then build.
const _: () = assert!(std::mem::size_of::<Dormant<'static>>() == 6 * std::mem::size_of::<usize>());

/// Whether a tab is on screen: the active one, or a pane of the comparison. One rule, for the session and for the look.
pub(crate) fn on_screen(tabs: &Tabs, comparison: Option<&Comparison>, id: TabId) -> bool {
    tabs.active() == Some(id) || comparison.is_some_and(|comparison| comparison.contains(id))
}

impl Dormant<'_> {
    fn on_screen(&self, id: TabId) -> bool {
        on_screen(self.tabs, self.comparison, id)
    }

    /// The tabs that could go dormant, as they hold something: not the one on screen, and not one a save's under way on. Next to none however many tabs there are: the warm ones, and those with a list in memory.
    pub(crate) fn candidates(&self) -> impl Iterator<Item = TabId> + '_ {
        self.warm.ids().chain(self.lists.ids()).filter(move |id| self.may_go(*id))
    }

    /// How long from now until the next tab comes due to go dormant, in seconds, by the session's own clock.
    pub fn next_in(&self, after: u32) -> Option<u32> {
        self.next_at(after).map(|at| at.saturating_sub(self.dormancy.seconds_now()))
    }

    /// The tabs due to go dormant now, by the session's own clock.
    pub fn due(&self, after: u32) -> Vec<TabId> {
        self.due_at(self.dormancy.seconds_now(), after)
    }

    /// When the next tab comes due to go dormant, in seconds of the session, this long after it was last touched. One deadline for all of them, kept by whoever's waiting anyway. There's no timer per tab.
    pub(crate) fn next_at(&self, after: u32) -> Option<u32> {
        self.candidates()
            .filter_map(|id| self.tabs.tab(id))
            .map(|tab| tab.touched.saturating_add(after))
            .min()
    }

    /// The tabs due to go dormant at `now`, in seconds of the session.
    pub(crate) fn due_at(&self, now: u32, after: u32) -> Vec<TabId> {
        let mut due: Vec<TabId> = self
            .candidates()
            .filter(|id| self.tabs.tab(*id).is_some_and(|tab| tab.touched.saturating_add(after) <= now))
            .collect();
        due.sort_unstable_by_key(|id| id.0);
        due.dedup();
        due
    }

    /// Every tab that could go dormant, oldest touched first, for when a budget or the system won't wait.
    pub fn oldest_first(&self) -> Vec<TabId> {
        let mut all: Vec<TabId> = self.candidates().collect();
        all.sort_unstable_by_key(|id| (self.tabs.tab(*id).map_or(0, |tab| tab.touched), id.0));
        all.dedup();
        all
    }

    /// Whether a tab may go dormant, whatever's asking: not the one on screen, one being worked on, or one being saved.
    pub fn may_go(&self, id: TabId) -> bool {
        !self.on_screen(id) && !self.dormancy.is_held(id) && self.lists.get(id).is_none_or(|list| list.saving().is_none())
    }

    /// What a dormant tab's journal has to say for its list to go to disk: the whole list, to compact to and compare with. `None` if there's no list, if it's not all in the file, or if nobody keeps journals here.
    pub fn list_to_sleep(&self, id: TabId) -> Option<(String, record::JournalHeader, record::EditsRecord)> {
        if !self.journals.kept_and_readable() {
            return None;
        }
        let look = journal::JournalLook {
            journals: self.journals,
            lists: self.lists,
            tabs: self.tabs,
        };
        let (header, base) = look.checkpoint_if_caught_up(id)?;
        Some((self.journals.name_of(id)?.to_string(), header, base))
    }

    /// What's asleep, by journal, for reading back: each has to read as what's known of it here.
    pub fn asleep_by_journal(&self) -> HashMap<String, record::EditsSummary> {
        self.lists
            .asleep()
            .filter_map(|(id, summary)| Some((self.journals.name_of(*id)?.to_string(), *summary)))
            .collect()
    }

    /// **Whether a password was needed to open a tab's file:** it was opened with one the person typed, or it needs one that hasn't been given. What keeps a
    /// document's pages out of a sleep file, and the same as what keeps them from an agent ([`Tabs::withheld_from_agents`]), stated once there.
    fn password_was_needed(&self, id: TabId) -> bool {
        self.tabs.withheld_from_agents(id)
    }

    /// What a tab's sleep file would say of it, apart from what the page view knows, if it can have one: its file's been read this session, so its contents are here, and it isn't protected, as this would be those in the clear.
    ///
    /// **Protected here means a password was needed to open it** ([`Self::password_was_needed`]), not that the file is encrypted on disk: a file anyone can open
    /// with no password (permissions only, as many government and UN reports are) has nothing a sleep file could give away, and its sleep file is private and
    /// out of backups like any other. Until 2026-11-13 the rule read `encrypted`, so such a document never had a sleep file and woke bare every time.
    pub fn sleep_facts(&self, id: TabId) -> Option<SleepFacts> {
        let toc = self.warm.toc(id)?;
        self.sleep_facts_with(id, toc)
    }

    pub(crate) fn sleep_facts_with(&self, id: TabId, toc: &Toc) -> Option<SleepFacts> {
        let tab = self.tabs.tab(id)?;
        let stamp = tab.file.stamp?;
        // What's come from a sleep file and hasn't been checked against the PDF isn't written again as if it had. Nor anything of a file that's not what it was,
        // or one its edits don't fit: what's held would be of another version than the one the sleep file is named for.
        let notices = fit::Notices {
            tabs: self.tabs,
            lists: self.lists,
        };
        if self.password_was_needed(id)
            || tab.from_sleep.is_some()
            || self.lists.is_asleep(id)
            || matches!(tab.file.notice(), Some(FileNotice::Changed | FileNotice::Missing | FileNotice::NeedsPassword))
            || !notices.fit(id).fits()
        {
            return None;
        }
        let list = self.lists.get(id);
        let (pages, list_seq) = list.map_or((tab.page_count, 0), |list| (list.len(), list.order_mark()));
        let sleep = sleep::SleepFile {
            file: stamp,
            file_pages: tab.page_count,
            pages,
            list_seq,
            renderer_only: tab.renderer_only,
            version: None,
            toc: toc.clone(),
            usual: None,
            sizes: Vec::new(),
            pictures: Vec::new(),
        };
        // A page from a protected file on any page means pictures of none, and it's the same for each of them.
        Some(SleepFacts {
            id,
            path: self.tabs.named_path(id)?,
            sleep,
            pictures_allowed: !list.is_some_and(PageEdits::has_protected_pages),
        })
    }

    /// The same for a tab that's just been woken some other way, for a picture for its first frame.
    pub fn sleep_to_read_awake(&self, id: TabId) -> Option<(FileStamp, PathBuf)> {
        let tab = self.tabs.tab(id)?;
        if !self.dormancy.sleep_files() || self.password_was_needed(id) {
            return None;
        }
        Some((tab.file.stamp?, self.tabs.named_path(id)?))
    }

    /// What's on hand to wake a cold tab from its sleep file: which file, as far as anything's known, without looking at it. `None` for a tab that isn't cold.
    pub fn sleep_to_read(&self, id: TabId) -> Option<(FileStamp, PathBuf)> {
        if self.warm.contains(id) {
            return None;
        }
        let tab = self.tabs.tab(id)?;
        if self.password_was_needed(id) {
            return None;
        }
        Some((tab.file.stamp?, self.tabs.named_path(id)?))
    }

    /// A cold tab's file to read for its waking, and the tab: `None` for a warm one, or one with no file to read.
    pub(crate) fn file_to_wake(&self, id: TabId) -> Option<(TabId, PathBuf)> {
        if self.warm.contains(id) {
            return None;
        }
        self.tabs.path_of(id).map(|path| (id, path))
    }
}
