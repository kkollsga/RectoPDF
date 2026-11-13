//! Going dormant and waking from sleep files: the session's clock, the holds that keep a tab from going dormant, and what's on its way between the warm tabs, the sleep files and whoever writes and checks them.

use crate::TabId;
use crate::sleep::SleepFile;
use recto_core::Toc;
use std::sync::Arc;

/// Tabs gone cold whose sleep files wait to be written, held at once. The app's sleeper takes them as they come, so this is only ever reached if it's stopped. Each holds a document's contents, which is why it's small.
pub(crate) const MAX_LEFT_WARM: usize = 8;
/// Tabs woken from sleep files whose files wait to be looked at, held at once. The same: the sleeper takes them as they come.
pub(crate) const MAX_WOKEN_FROM_FILE: usize = 64;

/// The session's clock, the holds that keep a tab from going dormant, and what's on its way to and from the sleep files. What's public here reads the clock,
/// holds a tab, turns sleep files on or off, or takes what's waiting; letting a hold go stays with the session, as it stamps the tab.
#[derive(Debug)]
pub struct Dormancy {
    /// What tabs' `touched` count from.
    started: std::time::Instant,
    /// Tabs being worked on now, once for each thing working on it. Next to none.
    held: Vec<TabId>,
    /// Cold tabs wake from their sleep files here.
    sleep_files: bool,
    /// The sleep file a tab was just woken from, for its first frame.
    just_read: Option<(TabId, SleepFile)>,
    /// Tabs woken from sleep files whose files nobody's looked at yet. Each once, at most [`MAX_WOKEN_FROM_FILE`].
    woken_from_file: Vec<TabId>,
    /// Tabs just pushed out of the warm ones, with their contents, kept long enough for their sleep files to be written. Each once, at most [`MAX_LEFT_WARM`].
    left_warm: Vec<(TabId, Arc<Toc>)>,
}

impl Dormancy {
    /// The clock starting now, no holds, and no sleep files till someone says. The crate's: see [`crate::parts::PartMut`].
    pub(crate) fn new() -> Self {
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
    pub fn seconds_now(&self) -> u32 {
        u32::try_from(self.started.elapsed().as_secs()).unwrap_or(u32::MAX)
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

    /// A tab's gone cold from the warm ones: what its sleep file needs is kept long enough to write that, if there are sleep files here. **At the cap the oldest goes, and its sleep file isn't written this time:** the tab wakes from its PDF instead, a read made with the session free, and nothing of anyone's is lost.
    pub(crate) fn note_left(&mut self, left: (TabId, Arc<Toc>)) {
        if self.sleep_files {
            self.left_warm.retain(|(tab, _)| *tab != left.0);
            if self.left_warm.len() >= MAX_LEFT_WARM {
                self.left_warm.remove(0);
            }
            self.left_warm.push(left);
        }
    }

    /// A tab's no longer on its way to a sleep file from the warm ones: it's warm again, so what it left with would be written for a tab on
    /// screen, or what was read of it has turned out to be of another file, which no sleep file may carry on.
    pub(crate) fn not_leaving(&mut self, id: TabId) {
        self.left_warm.retain(|(tab, _)| *tab != id);
    }

    pub(crate) fn take_left(&mut self) -> Vec<(TabId, Arc<Toc>)> {
        std::mem::take(&mut self.left_warm)
    }

    /// A tab's been woken from this sleep file, which is kept for its first frame, and its file's to be looked at. **At the cap the oldest goes, and its file isn't looked at on this way past:** a file changed behind its sleep file is still found when the renderer opens it to draw and says how long it is ([`crate::Sleeper::file_length_is`]), which overrules what the sleep file said.
    pub(crate) fn woke_from_file(&mut self, id: TabId, sleep: SleepFile) {
        self.just_read = Some((id, sleep));
        self.woken_from_file.retain(|tab| *tab != id);
        if self.woken_from_file.len() >= MAX_WOKEN_FROM_FILE {
            self.woken_from_file.remove(0);
        }
        self.woken_from_file.push(id);
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

/// What a caller outside the crate may change of dormancy: a hold on a tab, whether there are sleep files, and taking what's waiting for whoever writes
/// and checks them. Letting a hold go stays the session's, as it stamps the tab.
impl crate::parts::PartMut<'_, Dormancy> {
    /// Something's working on this tab, a command of an agent's or an export, so it doesn't go dormant, whatever's asking. As many holds as there are, each let go by [`crate::Sleeper::let_go`].
    pub fn hold(&mut self, id: TabId) {
        self.0.held.push(id);
    }

    /// Whether cold tabs wake from their sleep files here: only with someone to look at the file on the way past, and tell [`crate::Session::apply_file_status`] if it's not what it was, by [`Self::take_woken_from_file`].
    pub fn set_sleep_files(&mut self, on: bool) {
        self.0.sleep_files = on;
    }

    /// The tabs woken from sleep files, whose files nobody's looked at yet.
    pub fn take_woken_from_file(&mut self) -> Vec<TabId> {
        std::mem::take(&mut self.0.woken_from_file)
    }

    /// The sleep file a tab was just woken from, for its first frame.
    pub fn take_sleep_read(&mut self, id: TabId) -> Option<SleepFile> {
        self.0.just_read.take().and_then(|(tab, sleep)| (tab == id).then_some(sleep))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::PartMut;
    use recto_platform::FileStamp;

    fn sleep_file() -> SleepFile {
        SleepFile {
            file: FileStamp {
                size: 1,
                mtime_ns: 1,
                device: 1,
                inode: 1,
            },
            file_pages: 1,
            pages: 1,
            list_seq: 0,
            renderer_only: false,
            version: None,
            toc: Toc::default(),
            usual: None,
            sizes: Vec::new(),
            pictures: Vec::new(),
        }
    }

    /// Failure: with the sleeper stopped, tabs going cold pile up here past the cap, each holding its document's contents; or one tab going cold twice is held twice.
    #[test]
    fn tabs_gone_cold_wait_for_their_sleep_files_within_the_cap() {
        let mut dormancy = Dormancy::new();
        PartMut(&mut dormancy).set_sleep_files(true);
        for n in 0..=MAX_LEFT_WARM as u32 {
            dormancy.note_left((TabId(n), Arc::new(Toc::default())));
        }
        dormancy.note_left((TabId(5), Arc::new(Toc::default())));
        let left: Vec<TabId> = dormancy.take_left().into_iter().map(|(id, _)| id).collect();
        assert_eq!(left.len(), MAX_LEFT_WARM);
        assert!(!left.contains(&TabId(0)), "the oldest went");
        assert_eq!(left.last(), Some(&TabId(5)), "a tab gone cold again is held once, as the latest");
        assert!(dormancy.take_left().is_empty());
    }

    /// Failure: with the sleeper stopped, tabs woken from sleep files pile up past the cap; or one woken twice is looked at twice.
    #[test]
    fn tabs_woken_from_sleep_files_wait_for_a_look_within_the_cap() {
        let mut dormancy = Dormancy::new();
        for n in 0..=MAX_WOKEN_FROM_FILE as u32 {
            dormancy.woke_from_file(TabId(n), sleep_file());
        }
        dormancy.woke_from_file(TabId(9), sleep_file());
        let woken = PartMut(&mut dormancy).take_woken_from_file();
        assert_eq!(woken.len(), MAX_WOKEN_FROM_FILE);
        assert!(!woken.contains(&TabId(0)), "the oldest went");
        assert_eq!(woken.last(), Some(&TabId(9)));
        assert_eq!(woken.iter().filter(|id| **id == TabId(9)).count(), 1);
        assert!(
            PartMut(&mut dormancy).take_sleep_read(TabId(9)).is_some(),
            "the latest one's sleep file is kept for its first frame"
        );
    }
}
