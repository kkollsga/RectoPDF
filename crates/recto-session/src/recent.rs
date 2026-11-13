//! Recent files: what File > Open Recent and ⌘K offer, newest first, at most [`MAX_RECENT`].

use crate::MAX_RECENT;
use crate::record::PathField;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

/// Files opened lately, for File > Open Recent and ⌘K. What's public here says when the list changed, or clears it; a file's noted on opening it.
#[derive(Debug)]
pub struct Recent {
    /// Newest first, at most [`MAX_RECENT`]. In the state record.
    recent: VecDeque<PathBuf>,
    /// Moves whenever `recent` does, so the app rebuilds what it shows of it only then.
    recent_version: u64,
}

impl Recent {
    /// No files yet. The crate's: see [`crate::parts::PartMut`].
    pub(crate) fn new() -> Self {
        Self {
            recent: VecDeque::new(),
            recent_version: 0,
        }
    }

    /// **A file opened, for File > Open Recent and ⌘K:** to the front, once, and the oldest past [`MAX_RECENT`] goes. Only a whole path is kept.
    pub(crate) fn note(&mut self, path: &Path) {
        if !path.is_absolute() || !PathField::from_path(path).fits() {
            return;
        }
        if self.recent.front().is_some_and(|front| front == path) {
            return;
        }
        self.recent.retain(|known| known != path);
        self.recent.push_front(path.to_path_buf());
        self.recent.truncate(MAX_RECENT);
        self.recent_version += 1;
    }

    /// Newest first.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.recent.iter()
    }

    /// A number that moves whenever the list of files opened lately does. Which of them are open moves with the tabs.
    pub fn version(&self) -> u64 {
        self.recent_version
    }

    /// As a record has them: only whole paths that fit, at most [`MAX_RECENT`].
    pub(crate) fn restore(&mut self, recent: &[PathField]) {
        self.recent = recent
            .iter()
            .filter(|path| path.fits())
            .filter_map(PathField::to_path)
            .filter(|path| path.is_absolute())
            .take(MAX_RECENT)
            .collect();
        self.recent_version += 1;
    }

    /// For the record.
    pub(crate) fn to_record(&self) -> Vec<PathField> {
        self.recent.iter().map(|path| PathField::from_path(path)).collect()
    }
}

/// What a caller outside the crate may change of the recent files: only clearing them. A file's noted on opening it.
impl crate::parts::PartMut<'_, Recent> {
    /// File > Open Recent > Clear Menu. Says whether there were any.
    pub fn clear(&mut self) -> bool {
        let recent = &mut *self.0;
        let had = !recent.recent.is_empty();
        recent.recent.clear();
        recent.recent_version += 1;
        had
    }
}
