//! **What a session method changed,** said by the method, so the events that follow are built from it and no caller compares state before and after (review C6, step R2.1). Small values, never lists of everything.

use crate::TabId;

/// A tab closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed {
    pub id: TabId,
    /// The tab showing now because of it, if another one is than before.
    pub activated: Option<TabId>,
}

impl Closed {
    /// A tab closed, with the tab showing before and after.
    pub fn after(id: TabId, before: Option<TabId>, now: Option<TabId>) -> Self {
        Self {
            id,
            activated: switched(before, now),
        }
    }
}

/// The tab showing now, if it isn't the one that was.
pub fn switched(before: Option<TabId>, now: Option<TabId>) -> Option<TabId> {
    now.filter(|now| Some(*now) != before)
}
