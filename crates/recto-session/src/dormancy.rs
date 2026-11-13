//! Going dormant and waking from sleep files: the session's clock, the holds that keep a tab from going dormant, and what's on its way between the warm tabs, the sleep files and whoever writes and checks them.

use crate::TabId;
use crate::sleep::SleepFile;
use recto_core::Toc;
use std::sync::Arc;

/// What going dormant and sleep files need of the session.
#[derive(Debug)]
pub(crate) struct Dormancy {
    /// What tabs' `touched` count from.
    started: std::time::Instant,
    /// Tabs being worked on now, once for each thing working on it. Next to none.
    held: Vec<TabId>,
    /// Cold tabs wake from their sleep files here.
    sleep_files: bool,
    /// The sleep file a tab was just woken from, for its first frame.
    just_read: Option<(TabId, SleepFile)>,
    /// Tabs woken from sleep files whose files nobody's looked at yet.
    woken_from_file: Vec<TabId>,
    /// Tabs just pushed out of the warm ones, with their contents, kept long enough for their sleep files to be written.
    left_warm: Vec<(TabId, Arc<Toc>)>,
}

impl Default for Dormancy {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            held: Vec::new(),
            sleep_files: false,
            just_read: None,
            woken_from_file: Vec::new(),
            left_warm: Vec::new(),
        }
    }
}

impl Dormancy {
    /// Seconds of this session so far.
    pub(crate) fn seconds_now(&self) -> u32 {
        u32::try_from(self.started.elapsed().as_secs()).unwrap_or(u32::MAX)
    }

    pub(crate) fn hold(&mut self, id: TabId) {
        self.held.push(id);
    }

    /// One hold of this tab's let go of.
    pub(crate) fn let_go(&mut self, id: TabId) {
        if let Some(at) = self.held.iter().position(|held| *held == id) {
            self.held.swap_remove(at);
        }
    }

    pub(crate) fn is_held(&self, id: TabId) -> bool {
        self.held.contains(&id)
    }

    pub(crate) fn sleep_files(&self) -> bool {
        self.sleep_files
    }

    pub(crate) fn set_sleep_files(&mut self, on: bool) {
        self.sleep_files = on;
    }

    /// A tab's gone cold from the warm ones: what its sleep file needs is kept long enough to write that, if there are sleep files here.
    pub(crate) fn note_left(&mut self, left: (TabId, Arc<Toc>)) {
        if self.sleep_files {
            self.left_warm.push(left);
        }
    }

    pub(crate) fn take_left(&mut self) -> Vec<(TabId, Arc<Toc>)> {
        std::mem::take(&mut self.left_warm)
    }

    /// A tab's been woken from this sleep file, which is kept for its first frame, and its file's to be looked at.
    pub(crate) fn woke_from_file(&mut self, id: TabId, sleep: SleepFile) {
        self.just_read = Some((id, sleep));
        self.woken_from_file.push(id);
    }

    pub(crate) fn take_woken_from_file(&mut self) -> Vec<TabId> {
        std::mem::take(&mut self.woken_from_file)
    }

    pub(crate) fn take_sleep_read(&mut self, id: TabId) -> Option<SleepFile> {
        self.just_read.take().and_then(|(tab, sleep)| (tab == id).then_some(sleep))
    }

    /// A restore: what's here is about the tabs of the session it replaces. The clock and whether there are sleep files stay.
    pub(crate) fn restored(&mut self) {
        self.held.clear();
        self.just_read = None;
        self.woken_from_file.clear();
        self.left_warm.clear();
    }

    /// A tab woken from its sleep file, with nothing kept of the file.
    #[cfg(test)]
    pub(crate) fn note_woken_from_file(&mut self, id: TabId) {
        self.woken_from_file.push(id);
    }

    /// Every part that holds something of this tab, by name.
    #[cfg(test)]
    pub(crate) fn holding(&self, id: TabId) -> [(&'static str, bool); 4] {
        [
            ("held", self.held.contains(&id)),
            ("just_read", self.just_read.as_ref().is_some_and(|(tab, _)| *tab == id)),
            ("woken_from_file", self.woken_from_file.contains(&id)),
            ("left_warm", self.left_warm.iter().any(|(tab, _)| *tab == id)),
        ]
    }
}
