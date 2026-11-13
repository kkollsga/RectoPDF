//! **The session's components, as callers reach them.** A caller that needs one concern takes that component, not the whole session. What's `pub` on a
//! component can't leave it out of step with another one: what has to change two together stays `pub(crate)`, called only by the session's own
//! operations. So a `&mut` here goes out only for a component a caller outside the crate changes today.

use super::*;
use crate::record::FindRecord;

impl<L: DocLoader> Session<L> {
    /// What a tab's pages are called and how many it has now, counting unsaved edits: a look over the tabs, the lists and the warm tabs' labels.
    pub fn pages(&self) -> pages::Pages<'_> {
        pages::Pages {
            tabs: &self.tabs,
            lists: &self.lists,
            warm: &self.warm,
        }
    }

    /// Files opened lately, for File > Open Recent and ⌘K.
    pub fn recent(&self) -> &recent::Recent {
        &self.recent
    }

    pub fn recent_mut(&mut self) -> &mut recent::Recent {
        &mut self.recent
    }

    /// Generated contents the sidebar knows of, for tabs someone asked about.
    pub fn generated(&self) -> &generated::Generated {
        &self.generated
    }

    pub fn generated_mut(&mut self) -> &mut generated::Generated {
        &mut self.generated
    }

    /// The tabs that keep their contents, most recent first, and the reads of cold tabs' files made ahead of waking them.
    pub fn warm(&self) -> &warm::Warm {
        &self.warm
    }

    /// The session's clock, the holds on tabs, and what's on its way to and from the sleep files.
    pub fn dormancy(&self) -> &dormancy::Dormancy {
        &self.dormancy
    }

    pub fn dormancy_mut(&mut self) -> &mut dormancy::Dormancy {
        &mut self.dormancy
    }

    /// Tabs being saved, and what saves lately moved their tabs on from.
    pub fn saves(&self) -> &saves::Saves {
        &self.saves
    }

    /// Each edited tab's journal, what's waiting to go out to the journals, and where a PDF's history is read from.
    pub fn journals(&self) -> &journals::Journals {
        &self.journals
    }

    pub fn journals_mut(&mut self) -> &mut journals::Journals {
        &mut self.journals
    }

    /// What passes between the session and the page view: the pages lately drawn, the jumps it hasn't taken, and a place lost to an edit.
    pub fn mail(&self) -> &mail::PageViewMail {
        &self.mail
    }

    pub fn mail_mut(&mut self) -> &mut mail::PageViewMail {
        &mut self.mail
    }

    /// Every tab's list of edits, in memory or gone to disk, and whether an agent's at Full.
    pub fn lists(&self) -> &lists::Lists {
        &self.lists
    }

    pub fn lists_mut(&mut self) -> &mut lists::Lists {
        &mut self.lists
    }

    /// Every open tab in the sidebar's order, which one's active, and their folders.
    pub fn tabs(&self) -> &registry::Tabs {
        &self.tabs
    }

    pub fn tabs_mut(&mut self) -> &mut registry::Tabs {
        &mut self.tabs
    }

    /// The view's settings, Find and edit mode.
    pub fn viewing(&self) -> &viewing::Viewing {
        &self.viewing
    }

    pub fn viewing_mut(&mut self) -> &mut viewing::Viewing {
        &mut self.viewing
    }
}

/// Session's methods kept only for callers in files another branch holds (R3.5's bounds). Each goes once that branch has merged, its callers taking the
/// component the comment names.
impl<L: DocLoader> Session<L> {
    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `tabs().withheld_from_agents`.
    pub fn withheld_from_agents(&self, id: TabId) -> bool {
        self.tabs.withheld_from_agents(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `tabs().path_of`.
    pub fn path_of(&self, id: TabId) -> Option<PathBuf> {
        self.tabs.path_of(id)
    }
    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `tabs().active`.
    pub fn active(&self) -> Option<TabId> {
        self.tabs.active()
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `tabs_mut().set_view_rotation`.
    pub fn set_view_rotation(&mut self, id: TabId, quarter_turns: u8) {
        self.tabs.set_view_rotation(id, quarter_turns);
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `tabs().own_stamp`.
    pub fn own_stamp(&self, id: TabId) -> Option<FileStamp> {
        self.tabs.own_stamp(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `tabs().tab`.
    pub fn tab(&self, id: TabId) -> Option<&TabRecord> {
        self.tabs.tab(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `viewing().find`.
    pub fn find(&self) -> &FindRecord {
        self.viewing.find()
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `viewing_mut().set_find`.
    pub fn set_find(&mut self, open: bool, query: &str, draft: &str) {
        self.viewing.set_find(open, query, draft);
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `lists().is_edited`.
    pub fn is_edited(&self, id: TabId) -> bool {
        self.lists.is_edited(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `lists().get`.
    pub fn page_edits(&self, id: TabId) -> Option<&PageEdits> {
        self.lists.get(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `lists().own_pages_needed`.
    pub fn own_pages_needed(&self, id: TabId) -> Vec<u32> {
        self.lists.own_pages_needed(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `lists().undo_and_redo`.
    pub fn undo_and_redo(&self, id: TabId) -> (bool, bool) {
        self.lists.undo_and_redo(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `lists().is_asleep`.
    pub fn list_is_asleep(&self, id: TabId) -> bool {
        self.lists.is_asleep(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `journals_mut().set_journaling`.
    pub fn set_journaling(&mut self, on: bool) {
        self.journals.set_journaling(on);
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `dormancy_mut().hold`.
    pub fn hold(&mut self, id: TabId) {
        self.dormancy.hold(id);
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `pages().count`.
    pub fn pages_in(&self, id: TabId) -> u32 {
        self.pages().count(id)
    }
}

/// Session's methods kept only for the bodies of its own operations that another branch is changing (R3.5's bounds: `apply_file_status`, `activate`,
/// `let_go_of_list` and `saving.rs`), so they're left as they are on `main`. Each goes once that branch has merged, its callers taking the component the
/// comment names.
impl<L: DocLoader> Session<L> {
    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `self.tabs.tab_mut`.
    pub(crate) fn tab_mut(&mut self, id: TabId) -> Option<&mut TabRecord> {
        self.tabs.tab_mut(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `self.tabs.for_path`.
    pub(crate) fn tab_for_path(&self, path: &Path) -> Option<TabId> {
        self.tabs.for_path(path)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `self.lists.has_undone_owner_steps`.
    pub(crate) fn has_undone_owner_steps(&self, id: TabId) -> bool {
        self.lists.has_undone_owner_steps(id)
    }

    // R3.5: kept for a caller in a file another branch holds; goes once that branch has merged. Callers: `self.journals.retire_kept`.
    pub(crate) fn retire_kept(&mut self, kept: Vec<Box<Path>>) {
        self.journals.retire_kept(kept);
    }
}
