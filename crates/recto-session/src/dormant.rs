//! **Which tabs may go dormant, and when:** a look over the tabs, the warm ones, the lists, the holds and the comparison, changing none of them. Going
//! dormant itself changes several of them together, so it stays with the session ([`crate::Session::go_dormant`]).

use crate::TabId;
use crate::compare::Comparison;
use crate::dormancy::Dormancy;
use crate::lists::Lists;
use crate::registry::Tabs;
use crate::warm::Warm;

/// What going dormant goes by, borrowed from the session for one look.
pub struct Dormant<'a> {
    pub(crate) tabs: &'a Tabs,
    pub(crate) warm: &'a Warm,
    pub(crate) lists: &'a Lists,
    pub(crate) dormancy: &'a Dormancy,
    pub(crate) comparison: Option<&'a Comparison>,
}

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

    /// How long from now until the next tab comes due to go dormant, in seconds, by the session's own clock: [`Self::next_at`] less the time now.
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
}
