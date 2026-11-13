//! Saves under way, and what each save lately moved its tab on from.

use crate::{MAX_SAVED_OVER, TabId};
use recto_platform::FileStamp;

/// Saves left under way by a restore that are kept by name. Saves run one a tab and end in moments, so a restore meets next to none; past this many, every save not begun since is doubted (see [`Saves::beyond`]).
pub(crate) const MAX_ABANDONED_SAVES: usize = 64;

/// Tabs being saved, and what saves lately moved their tabs on from. What's public here only reads; a save begins and ends with the session, as it goes
/// with the tab's list and file.
#[derive(Debug, Default)]
pub struct Saves {
    /// Tabs being saved now. Nothing changes their edits until the save's done: an edit made while the file was being written would read as saved and not be in it (review). Next to none.
    saving: Vec<TabId>,
    /// Tabs a save has moved to another version of their file, as `(tab, before, after)`: the pages the new version has are the ones that were shown, each as it was shown, so the page view keeps its layout, place and selection. At most [`MAX_SAVED_OVER`], the oldest out; one is only believed while its tab is still on `after`.
    saved_over: Vec<(TabId, FileStamp, FileStamp)>,
    /// **Saves a restore left under way: they belong to the session it replaced.** A restored tab can have one of their tabs' numbers, and the callers pass back only that number, so what such a save says on finishing (`save_prepared`, `saved`) is not believed of the restored tab: it changes nothing of it. Its end takes it off this list, and until then no new save of that number begins, so an old save's word can't land on a new one. At most [`MAX_ABANDONED_SAVES`], the oldest kept.
    ///
    /// What this doesn't cover: the old save still writes the file it was saving, which is the old document's own file, and that's right. And the app saver's own steps for that number (`before_writing`, `keep_pages`, `note_on_disk`, `after_save`) still run, outside this crate.
    abandoned: Vec<TabId>,
    /// A restore left more saves under way than are kept by name, so the rest aren't known: from then on, for the rest of this session, no save is believed of a tab that has no save begun since, and no new save begins. Edits stay in their journals for the next launch. When in doubt a save isn't believed, and no person or agent reaches this many saves at once.
    beyond: bool,
}

impl Saves {
    /// A restore: saves under way belong to the session it replaces, and what was remembered of saves is about its tabs.
    pub(crate) fn restored(&mut self) {
        for id in std::mem::take(&mut self.saving) {
            if self.abandoned.contains(&id) {
                continue;
            }
            if self.abandoned.len() >= MAX_ABANDONED_SAVES {
                self.beyond = true;
            } else {
                self.abandoned.push(id);
            }
        }
        self.saved_over.clear();
    }

    /// Whether what a save of this tab says on finishing is to be doubted: it was left under way by a restore.
    pub(crate) fn doubted(&self, id: TabId) -> bool {
        self.abandoned.contains(&id) || (self.beyond && !self.saving.contains(&id))
    }

    /// Whether a new save of this tab must wait: an old one, left by a restore, may still be finishing.
    pub(crate) fn must_wait(&self, id: TabId) -> bool {
        self.beyond || self.abandoned.contains(&id)
    }

    /// The end of a save left under way by a restore: it's let go of, and nothing else. Says whether it was one.
    pub(crate) fn end_abandoned(&mut self, id: TabId) -> bool {
        let Some(at) = self.abandoned.iter().position(|abandoned| *abandoned == id) else {
            return false;
        };
        self.abandoned.remove(at);
        true
    }

    #[cfg(test)]
    pub(crate) fn abandoned_has(&self, id: TabId) -> bool {
        self.abandoned.contains(&id)
    }

    #[cfg(test)]
    pub(crate) fn abandoned_len(&self) -> usize {
        self.abandoned.len()
    }

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

    /// Whether a save of this tab is under way.
    pub fn is_saving(&self, id: TabId) -> bool {
        self.saving.contains(&id)
    }

    /// Whether any save is under way, which a hand-off waits for.
    pub fn any(&self) -> bool {
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
