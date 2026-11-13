//! **Comparison view.** Owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view, right click menu can enable separate scroll (default) and synced scroll. Zoom is always synced."
//!
//! Several tabs side by side, each its own document in its own place. The rules are all here, with no page view in sight: which tabs, in which order, which one the reader's in, how synced scroll maps a page in one to a page in the others, and what's said when their lengths differ. The host draws a pane per tab and asks.
//!
//! **At most [`MAX_PANES`],** the same as the live documents' rule (SPEC §2: "Active plus at most 2 recent"), so a comparison of three is three live documents and costs nothing past what one reader and its two recent tabs already may. A fourth is refused with words for it.
//!
//! **Synced scroll goes page for page from where it was turned on.** Each pane's page then is its start, and a page `n` further on in one is `n` further on in each, at the same depth in its page. A pane past its end stays on its last page, and the panes come back into step on the way back. The reader is told once, beside the page number, when the documents differ in length.

use crate::TabId;

/// The most tabs side by side.
pub const MAX_PANES: usize = 3;
/// The fewest for there to be a comparison: one is reading.
pub const MIN_PANES: usize = 2;

/// Tabs side by side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comparison {
    /// In the order the panes are drawn, left to right.
    pub tabs: Vec<TabId>,
    /// Synced scroll is on. Separate is the default (owner).
    pub synced: bool,
    /// Each pane's page when synced scroll went on, by the order of `tabs`. Empty while separate.
    pub sync_from: Vec<u32>,
}

/// What a ⌘-click did, for the caller to say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareChange {
    /// A comparison began, or took another tab.
    Added,
    /// A tab left it, and the others are still side by side.
    Removed,
    /// Down to one, which is reading alone now.
    Ended,
    /// Nothing to do: a ⌘-click on the only tab showing.
    Nothing,
}

/// Why a comparison can't be as asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareError {
    TooMany,
    TooFew,
    NoSuchTab,
    SameTabTwice,
}

impl CompareError {
    pub fn code(self) -> &'static str {
        match self {
            Self::TooMany => "too_many_panes",
            Self::TooFew => "too_few_panes",
            Self::NoSuchTab => "no_such_tab",
            Self::SameTabTwice => "same_tab_twice",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::TooMany => "At most three PDFs can be side by side.",
            Self::TooFew => "A comparison needs two PDFs or more.",
            Self::NoSuchTab => "That tab isn't open.",
            Self::SameTabTwice => "A PDF can be in a comparison once.",
        }
    }
}

impl Comparison {
    /// Checks a list of tabs for a comparison: two to [`MAX_PANES`], each once.
    pub fn check(tabs: &[TabId]) -> Result<(), CompareError> {
        if tabs.len() < MIN_PANES {
            return Err(CompareError::TooFew);
        }
        if tabs.len() > MAX_PANES {
            return Err(CompareError::TooMany);
        }
        if tabs.iter().enumerate().any(|(at, tab)| tabs[..at].contains(tab)) {
            return Err(CompareError::SameTabTwice);
        }
        Ok(())
    }

    pub fn new(tabs: Vec<TabId>) -> Result<Self, CompareError> {
        Self::check(&tabs)?;
        Ok(Self {
            tabs,
            synced: false,
            sync_from: Vec::new(),
        })
    }

    pub fn contains(&self, id: TabId) -> bool {
        self.tabs.contains(&id)
    }

    /// Synced scroll on from these pages, one for each pane, or off.
    pub fn set_synced(&mut self, on: bool, pages_now: impl Fn(TabId) -> u32) {
        self.synced = on;
        self.sync_from = if on {
            self.tabs.iter().map(|tab| pages_now(*tab)).collect()
        } else {
            Vec::new()
        };
    }

    /// Takes a tab out, as closing it or a ⌘-click does. Its start goes with it.
    pub fn remove(&mut self, id: TabId) {
        if let Some(at) = self.tabs.iter().position(|tab| *tab == id) {
            self.tabs.remove(at);
            if at < self.sync_from.len() {
                self.sync_from.remove(at);
            }
        }
    }

    /// Adds a tab at the right, starting it in step from the page it's on.
    pub fn add(&mut self, id: TabId, page: u32) -> Result<(), CompareError> {
        if self.contains(id) {
            return Err(CompareError::SameTabTwice);
        }
        if self.tabs.len() >= MAX_PANES {
            return Err(CompareError::TooMany);
        }
        self.tabs.push(id);
        if self.synced {
            self.sync_from.push(page);
        }
        Ok(())
    }

    /// **Synced scroll's rule:** where each other pane goes when `leader` is on `page`, as `(tab, page)`, each held to its own length (`pages_of`). Nothing while separate.
    pub fn sync_targets(&self, leader: TabId, page: u32, pages_of: impl Fn(TabId) -> u32) -> Vec<(TabId, u32)> {
        if !self.synced || self.sync_from.len() != self.tabs.len() {
            return Vec::new();
        }
        let Some(from) = self.tabs.iter().position(|tab| *tab == leader) else {
            return Vec::new();
        };
        let moved = i64::from(page) - i64::from(self.sync_from[from]);
        self.tabs
            .iter()
            .zip(&self.sync_from)
            .filter(|(tab, _)| **tab != leader)
            .map(|(tab, start)| {
                let last = i64::from(pages_of(*tab).saturating_sub(1));
                let target = (i64::from(*start) + moved).clamp(0, last);
                (*tab, u32::try_from(target).unwrap_or(0))
            })
            .collect()
    }

    /// What's said with synced scroll on documents of different lengths, once, beside the page number. Nothing when they're the same length or it's separate.
    pub fn length_note(&self, pages_of: impl Fn(TabId) -> u32) -> Option<String> {
        if !self.synced {
            return None;
        }
        let lengths: Vec<u32> = self.tabs.iter().map(|tab| pages_of(*tab)).collect();
        if lengths.windows(2).all(|pair| pair[0] == pair[1]) {
            return None;
        }
        let list = match lengths.as_slice() {
            [one, two] => format!("{one} and {two}"),
            [rest @ .., last] => format!("{} and {last}", rest.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")),
            [] => String::new(),
        };
        Some(format!("Different lengths: {list} pages. Synced scroll waits at the end of a shorter one."))
    }
}

use super::*;

impl<L: DocLoader> Session<L> {
    /// Tabs side by side, if there are.
    pub fn comparison(&self) -> Option<&compare::Comparison> {
        self.comparison.as_ref()
    }

    /// What's on screen, a pane each, left to right: a comparison's tabs, or the active tab alone, or nothing.
    pub fn panes(&self) -> Vec<TabId> {
        match &self.comparison {
            Some(comparison) => comparison.tabs.clone(),
            None => self.tabs.active().into_iter().collect(),
        }
    }

    /// Whether a tab is on screen: active, or a pane of a comparison. Such a tab doesn't go dormant, whatever's asking.
    pub fn on_screen(&self, id: TabId) -> bool {
        self.tabs.active() == Some(id) || self.comparison.as_ref().is_some_and(|comparison| comparison.contains(id))
    }

    /// **Puts these tabs side by side,** in this order, separate scroll, with `reading` the pane being read, or the active tab if it's one of them, or the first. Their edits are woken, as a pane shows them. A tab that's cold is read here if nobody's read it ahead with the session free, as [`Self::wake_for_tap`] lets a caller do.
    pub fn compare(&mut self, tabs: &[TabId], reading: Option<TabId>) -> Result<bool, compare::CompareError> {
        if tabs.iter().any(|id| self.tabs.tab(*id).is_none()) {
            return Err(compare::CompareError::NoSuchTab);
        }
        let comparison = compare::Comparison::new(tabs.to_vec())?;
        let was = self.comparison.clone();
        for id in tabs {
            self.wake_list(*id);
        }
        let reading = reading
            .filter(|id| tabs.contains(id))
            .or(self.tabs.active().filter(|id| tabs.contains(id)))
            .unwrap_or(tabs[0]);
        // Synced scroll stays on through a change of tabs, starting again from where each is now.
        let synced = self.comparison.as_ref().is_some_and(|was| was.synced);
        self.comparison = Some(comparison);
        if synced {
            self.set_compare_synced(true);
        }
        // Every pane is a live document, so each is one of the warm tabs, the one read last and so first: a tab warm before that isn't a pane goes cold, and the live documents stay at [`MAX_WARM`] (SPEC §2), as `compare::MAX_PANES` is.
        // Those warm already first, so a pane coming in from cold pushes out a tab that isn't a pane, and never another pane, which would then be read with the session held.
        let mut others: Vec<TabId> = tabs.iter().copied().filter(|id| *id != reading).collect();
        others.sort_by_key(|id| !self.warm.contains(*id));
        for id in others {
            self.activate(id);
        }
        self.activate(reading);
        Ok(self.comparison != was)
    }

    /// What a ⌘-click on this tab would wake from cold, and its file, to read with the session free: the tab joining, or the pane read next if the one being read leaves. `None` if nothing needs reading.
    pub fn wake_for_compare_click(&mut self, id: TabId) -> Option<(TabId, PathBuf)> {
        let joining = !self.comparison.as_ref().is_some_and(|comparison| comparison.contains(id));
        let next = if joining {
            id
        } else if self.tabs.active() == Some(id) {
            self.pane_after(id)?
        } else {
            return None;
        };
        if self.tabs.active() == Some(next) {
            return None;
        }
        self.file_to_wake_now(next)
    }

    /// The panes a comparison of these tabs would wake from cold, with their files, to read with the session free before [`Self::compare`].
    pub fn wake_for_compare(&mut self, tabs: &[TabId]) -> Vec<(TabId, PathBuf)> {
        let active = self.tabs.active();
        tabs.iter()
            .filter(|id| active != Some(**id))
            .filter_map(|id| self.file_to_wake_now(*id))
            .collect()
    }

    /// **⌘-click and ⌃-click on a tab** (owner, 2026-11-08: "Ctrl clicking more then one pdf should enable comparison view"). A tab not in it joins at the right and is the one read; one in it leaves; down to one, that one's alone. On the only tab showing, nothing.
    pub fn toggle_compare(&mut self, id: TabId) -> Result<compare::CompareChange, compare::CompareError> {
        if self.tabs.tab(id).is_none() {
            return Err(compare::CompareError::NoSuchTab);
        }
        let Some(mut comparison) = self.comparison.clone() else {
            let Some(active) = self.tabs.active().filter(|active| *active != id) else {
                return Ok(compare::CompareChange::Nothing);
            };
            self.compare(&[active, id], Some(id))?;
            return Ok(compare::CompareChange::Added);
        };
        if comparison.contains(id) {
            comparison.remove(id);
            if comparison.tabs.len() < compare::MIN_PANES {
                let last = comparison.tabs.first().copied();
                self.comparison = None;
                if let Some(last) = last {
                    self.activate(last);
                }
                return Ok(compare::CompareChange::Ended);
            }
            let reading = comparison.tabs.first().copied();
            self.comparison = Some(comparison);
            if self.tabs.active() == Some(id)
                && let Some(reading) = reading
            {
                self.activate(reading);
            }
            return Ok(compare::CompareChange::Removed);
        }
        let page = self.tabs.tab(id).map_or(0, |tab| tab.page);
        comparison.add(id, page)?;
        self.wake_list(id);
        self.comparison = Some(comparison);
        self.activate(id);
        Ok(compare::CompareChange::Added)
    }

    /// Back to one tab: the one being read.
    pub fn end_compare(&mut self) -> bool {
        self.comparison.take().is_some()
    }

    /// Synced scroll on, from where each pane is now, or separate. Says nothing if there was no comparison to set, and otherwise whether it changed.
    pub fn set_compare_synced(&mut self, on: bool) -> Option<bool> {
        let pages: HashMap<TabId, u32> = self.tabs.all().iter().map(|tab| (tab.id, tab.page)).collect();
        self.comparison.as_mut().map(|comparison| {
            let was = comparison.clone();
            comparison.set_synced(on, |id| pages.get(&id).copied().unwrap_or(0));
            *comparison != was
        })
    }

    /// The reader's moved into another pane: it's the one read now, for the contents and the page number. Nothing changes on screen.
    pub fn focus_pane(&mut self, id: TabId) -> bool {
        if self.tabs.active() == Some(id) || !self.comparison.as_ref().is_some_and(|comparison| comparison.contains(id)) {
            return false;
        }
        self.activate(id);
        true
    }

    /// Where synced scroll sends the other panes when `leader` is on `page`: see [`compare::Comparison::sync_targets`].
    pub fn sync_targets(&self, leader: TabId, page: u32) -> Vec<(TabId, u32)> {
        self.comparison
            .as_ref()
            .map(|comparison| comparison.sync_targets(leader, page, |id| self.pages_in(id)))
            .unwrap_or_default()
    }

    /// What's said about the panes' lengths, if anything.
    pub fn compare_note(&self) -> Option<String> {
        self.comparison.as_ref()?.length_note(|id| self.pages_in(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(ids: &[u32]) -> Vec<TabId> {
        ids.iter().copied().map(TabId).collect()
    }

    /// Failure: a comparison of one, of four, or of the same tab twice gets through, and the host draws panes it has no budget for or two views of one document.
    #[test]
    fn a_comparison_is_two_to_three_different_tabs() {
        assert_eq!(Comparison::new(tabs(&[1])), Err(CompareError::TooFew));
        assert_eq!(Comparison::new(tabs(&[1, 2, 3, 4])), Err(CompareError::TooMany));
        assert_eq!(Comparison::new(tabs(&[1, 2, 1])), Err(CompareError::SameTabTwice));
        let mut three = Comparison::new(tabs(&[1, 2, 3])).unwrap();
        assert_eq!(three.add(TabId(4), 0), Err(CompareError::TooMany));
        assert!(!three.synced, "separate is the default");
        const { assert!(MAX_PANES <= crate::MAX_WARM, "panes are live documents, within that rule") };
    }

    /// Failure: synced scroll goes by page number rather than by where each pane was when it went on, so two editions a few pages apart line up wrongly; or a pane is sent past its end; or a step back from past the end doesn't bring them back into step.
    #[test]
    fn synced_scroll_keeps_page_for_page_from_where_it_went_on() {
        let mut comparison = Comparison::new(tabs(&[1, 2, 3])).unwrap();
        let start = |tab: TabId| [10, 3, 0][tab.0 as usize - 1];
        let length = |tab: TabId| [100, 20, 50][tab.0 as usize - 1];
        assert!(comparison.sync_targets(TabId(1), 12, length).is_empty(), "separate moves nothing");
        comparison.set_synced(true, start);
        assert_eq!(comparison.sync_targets(TabId(1), 12, length), vec![(TabId(2), 5), (TabId(3), 2)]);
        assert_eq!(
            comparison.sync_targets(TabId(1), 60, length),
            vec![(TabId(2), 19), (TabId(3), 49)],
            "each held to its own end"
        );
        assert_eq!(
            comparison.sync_targets(TabId(2), 0, length),
            vec![(TabId(1), 7), (TabId(3), 0)],
            "and its own start"
        );
        assert_eq!(
            comparison.sync_targets(TabId(1), 11, length),
            vec![(TabId(2), 4), (TabId(3), 1)],
            "back in step"
        );
        comparison.remove(TabId(2));
        assert_eq!(comparison.sync_targets(TabId(1), 12, length), vec![(TabId(3), 2)], "the rest keep their starts");
        comparison.add(TabId(2), 7).unwrap();
        assert_eq!(
            comparison.sync_targets(TabId(1), 12, length),
            vec![(TabId(3), 2), (TabId(2), 9)],
            "a tab added starts in step where it is"
        );
        comparison.set_synced(false, start);
        assert!(comparison.sync_from.is_empty());
    }

    /// Failure: documents of different lengths synced with nothing said, so the reader thinks a pane is stuck. It's said once, only when synced, and only when they differ.
    #[test]
    fn different_lengths_are_said_when_synced() {
        let mut comparison = Comparison::new(tabs(&[1, 2, 3])).unwrap();
        let length = |tab: TabId| [120, 96, 120][tab.0 as usize - 1];
        assert_eq!(comparison.length_note(length), None, "separate says nothing");
        comparison.set_synced(true, |_| 0);
        let note = comparison.length_note(length).unwrap();
        assert!(note.contains("120, 96 and 120"), "{note}");
        assert_eq!(comparison.length_note(|_| 40), None, "the same length says nothing");
    }
}
