//! Saves under way, and what each save lately moved its tab on from.

use crate::{MAX_SAVED_OVER, TabId};
use recto_platform::FileStamp;

/// Tabs being saved, and saves remembered for the page view.
#[derive(Debug, Default)]
pub(crate) struct Saves {
    /// Tabs being saved now. Nothing changes their edits until the save's done: an edit made while the file was being written would read as saved and not be in it (review). Next to none.
    saving: Vec<TabId>,
    /// Tabs a save has moved to another version of their file, as `(tab, before, after)`: the pages the new version has are the ones that were shown, each as it was shown, so the page view keeps its layout, place and selection. At most [`MAX_SAVED_OVER`], the oldest out; one is only believed while its tab is still on `after`.
    saved_over: Vec<(TabId, FileStamp, FileStamp)>,
}

impl Saves {
    pub(crate) fn begin(&mut self, id: TabId) {
        self.saving.push(id);
    }

    /// A save of this tab is over. Says whether one was under way.
    pub(crate) fn end(&mut self, id: TabId) -> bool {
        let Some(at) = self.saving.iter().position(|saving| *saving == id) else {
            return false;
        };
        self.saving.remove(at);
        true
    }

    pub(crate) fn is_saving(&self, id: TabId) -> bool {
        self.saving.contains(&id)
    }

    pub(crate) fn any(&self) -> bool {
        !self.saving.is_empty()
    }

    /// A save moved this tab on to `now`, from `before` if its version was known.
    pub(crate) fn saved_over(&mut self, id: TabId, before: Option<FileStamp>, now: FileStamp) {
        self.saved_over.retain(|(tab, _, _)| *tab != id);
        if let Some(before) = before {
            if self.saved_over.len() >= MAX_SAVED_OVER {
                self.saved_over.remove(0);
            }
            self.saved_over.push((id, before, now));
        }
    }

    /// The version a save moved this tab on from, while it's on `now`, the version that save wrote.
    pub(crate) fn saved_from(&self, id: TabId, now: FileStamp) -> Option<FileStamp> {
        self.saved_over
            .iter()
            .rev()
            .find(|(tab, _, after)| *tab == id && *after == now)
            .map(|(_, before, _)| *before)
    }

    /// Whether a save of this tab is remembered.
    #[cfg(test)]
    pub(crate) fn remembers(&self, id: TabId) -> bool {
        self.saved_over.iter().any(|(tab, _, _)| *tab == id)
    }

    #[cfg(test)]
    pub(crate) fn remembered(&self) -> usize {
        self.saved_over.len()
    }
}
