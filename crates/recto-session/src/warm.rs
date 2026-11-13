//! The warm tabs: those that keep their contents, at most [`MAX_WARM`], most recently used first, and the reads of cold tabs' files made ahead of waking them, at most [`MAX_READ_AHEAD`].

use crate::{MAX_READ_AHEAD, MAX_WARM, TabId};
use recto_core::{DocFacts, OpenError, Toc};
use recto_platform::FileStamp;
use std::collections::VecDeque;
use std::sync::Arc;

/// A read of a cold tab's file: what it found, and the version it found.
pub(crate) type Read = (Result<DocFacts, OpenError>, Option<FileStamp>);

/// The tabs that keep their contents, and reads of cold tabs' files made ahead of waking them. What's public here only reads; what's warm changes with
/// activating and waking, which go with the tab and its list.
#[derive(Debug, Default)]
pub struct Warm {
    /// Most recently used first, at most [`MAX_WARM`].
    warm: VecDeque<(TabId, Arc<Toc>)>,
    /// Cold tabs' files, read ahead of waking them with the session free. Each is taken by the next activation of that tab, and goes if the tab stops being cold. At most [`MAX_READ_AHEAD`].
    woken: Vec<(TabId, Result<DocFacts, OpenError>, Option<FileStamp>)>,
}

impl Warm {
    /// Most recently used first.
    pub fn ids(&self) -> impl Iterator<Item = TabId> + '_ {
        self.warm.iter().map(|(tab, _)| *tab)
    }

    pub(crate) fn contains(&self, id: TabId) -> bool {
        self.warm.iter().any(|(tab, _)| *tab == id)
    }

    /// A warm tab's contents.
    pub fn toc(&self, id: TabId) -> Option<&Arc<Toc>> {
        self.warm.iter().find(|(tab, _)| *tab == id).map(|(_, toc)| toc)
    }

    /// A warm tab is the most recent now. Says whether it was warm.
    pub(crate) fn bring_to_front(&mut self, id: TabId) -> bool {
        let Some(position) = self.warm.iter().position(|(tab, _)| *tab == id) else {
            return false;
        };
        if let Some(entry) = self.warm.remove(position) {
            self.warm.push_front(entry);
        }
        true
    }

    /// A tab's contents, as the most recent. Each pushed out past [`MAX_WARM`], the least recent, goes to `left`.
    pub(crate) fn push(&mut self, id: TabId, toc: Arc<Toc>, mut left: impl FnMut((TabId, Arc<Toc>))) {
        self.warm.push_front((id, toc));
        while self.warm.len() > MAX_WARM {
            if let Some(gone) = self.warm.pop_back() {
                left(gone);
            }
        }
    }

    /// A tab's contents go, and any read of its file.
    pub(crate) fn forget(&mut self, id: TabId) {
        self.warm.retain(|(tab, _)| *tab != id);
        self.woken.retain(|(tab, _, _)| *tab != id);
    }

    /// Whether a read of this tab's file is held for its waking.
    pub fn holds_read_of(&self, id: TabId) -> bool {
        self.woken.iter().any(|(tab, _, _)| *tab == id)
    }

    /// Any read of this tab's file goes.
    pub(crate) fn drop_read(&mut self, id: TabId) {
        self.woken.retain(|(tab, _, _)| *tab != id);
    }

    /// A read of a cold tab's file, kept for its waking. The oldest goes past [`MAX_READ_AHEAD`].
    pub(crate) fn keep_read(&mut self, id: TabId, loaded: Result<DocFacts, OpenError>, stamp: Option<FileStamp>) {
        if self.woken.len() >= MAX_READ_AHEAD {
            drop(self.woken.remove(0));
        }
        self.woken.push((id, loaded, stamp));
    }

    /// The read held for this tab's waking, taken.
    pub(crate) fn take_read(&mut self, id: TabId) -> Option<Read> {
        let place = self.woken.iter().position(|(tab, _, _)| *tab == id)?;
        let (_, loaded, stamp) = self.woken.remove(place);
        Some((loaded, stamp))
    }

    #[cfg(test)]
    pub(crate) fn reads_held(&self) -> usize {
        self.woken.len()
    }

    /// Lets go of every tab's contents, as the system taking memory back would.
    #[cfg(test)]
    pub(crate) fn clear_contents(&mut self) {
        self.warm.clear();
    }

    /// Lets go of a tab's contents alone.
    #[cfg(test)]
    pub(crate) fn drop_contents(&mut self, id: TabId) {
        self.warm.retain(|(tab, _)| *tab != id);
    }
}
