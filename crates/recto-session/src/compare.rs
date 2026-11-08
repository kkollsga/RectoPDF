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
        Ok(Self { tabs, synced: false, sync_from: Vec::new() })
    }

    pub fn contains(&self, id: TabId) -> bool {
        self.tabs.contains(&id)
    }

    /// Synced scroll on from these pages, one for each pane, or off.
    pub fn set_synced(&mut self, on: bool, pages_now: impl Fn(TabId) -> u32) {
        self.synced = on;
        self.sync_from = if on { self.tabs.iter().map(|tab| pages_now(*tab)).collect() } else { Vec::new() };
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
        let Some(from) = self.tabs.iter().position(|tab| *tab == leader) else { return Vec::new() };
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
        Some(format!("These PDFs have {list} pages. Synced scroll keeps them page for page, and a shorter one waits at its end."))
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
        assert_eq!(comparison.sync_targets(TabId(1), 60, length), vec![(TabId(2), 19), (TabId(3), 49)], "each held to its own end");
        assert_eq!(comparison.sync_targets(TabId(2), 0, length), vec![(TabId(1), 7), (TabId(3), 0)], "and its own start");
        assert_eq!(comparison.sync_targets(TabId(1), 11, length), vec![(TabId(2), 4), (TabId(3), 1)], "back in step");
        comparison.remove(TabId(2));
        assert_eq!(comparison.sync_targets(TabId(1), 12, length), vec![(TabId(3), 2)], "the rest keep their starts");
        comparison.add(TabId(2), 7).unwrap();
        assert_eq!(comparison.sync_targets(TabId(1), 12, length), vec![(TabId(3), 2), (TabId(2), 9)], "a tab added starts in step where it is");
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
