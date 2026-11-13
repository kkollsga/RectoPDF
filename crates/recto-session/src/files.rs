//! What became of a tab's file: notices and status.

use super::*;

impl<L: DocLoader> Session<L> {
    /// Words for what's wrong with a tab's file, if anything is.
    pub fn tab_notice_message(&self, id: TabId) -> Option<&'static str> {
        tab_notice_words(self.tabs.tab(id)?, self.lists.has_list(id))
    }

    /// What a look at a tab's file found, from [`record::look_at_file`], made on whatever thread could afford it. Nothing's written to any PDF. A file that moved is followed. One that's changed under edits keeps the stamp they were made on, so the edits' own checks go on saying so.
    ///
    /// **Says whether anything was new.** A look that finds what's already known (an edited tab already saying its file changed, finding it changed; one already
    /// saying it's gone, finding it gone; edits with no stamp, finding a file) changes nothing, and callers forget what's held of the file and read it again only
    /// on new: every return to the front and every show of such a tab would otherwise read the file's structure again (rule 1).
    pub fn apply_file_status(&mut self, id: TabId, status: FileStatus) -> bool {
        let has_edits = self.lists.has_list(id);
        let notice = self.tabs.tab_notice(id).map(FileNotice::code);
        let known = match &status {
            FileStatus::NotLooked => true,
            FileStatus::Changed(_) => self.lists.is_edited(id) && notice == Some("file_changed"),
            FileStatus::Missing => notice == Some("file_missing"),
            FileStatus::Same(_) => has_edits && self.tabs.own_stamp(id).is_none(),
            FileStatus::Moved(..) => false,
        };
        if known {
            return false;
        }
        // A file changed or gone takes its generated contents with it, so the sidebar shows what an agent's `contents --generated` finds, which goes by the file as it is now.
        if matches!(status, FileStatus::Changed(_) | FileStatus::Missing) {
            self.generated.forget(id);
            // Folds are by entry number, which another version's contents don't share.
            if let Some(tab) = self.tab_mut(id) {
                tab.folds = None;
            }
        }
        match status {
            FileStatus::NotLooked => {}
            FileStatus::Same(now) => {
                if let Some(tab) = self.tab_mut(id) {
                    let was = (tab.file.stamp, tab.file.notice.is_some());
                    tab.file.stamp = Some(now);
                    // Back to what edits are about. With none, the stamp's moved on to what the file is now, so being the same says nothing: it goes on saying it changed until it's been read, and left.
                    if matches!(tab.file.notice.as_deref(), Some(FileNotice::Missing))
                        || (has_edits && matches!(tab.file.notice.as_deref(), Some(FileNotice::Changed)))
                    {
                        tab.file.notice = None;
                    }
                    return was != (tab.file.stamp, tab.file.notice.is_some());
                }
            }
            FileStatus::Moved(path, now) => {
                // Another tab having it means this one isn't a move of that file so much as a second look at it.
                if self.tab_for_path(&path).is_some_and(|other| other != id) {
                    if let Some(tab) = self.tab_mut(id) {
                        tab.file.notice = Some(Box::new(FileNotice::Missing));
                    }
                    return true;
                }
                let from = self.path_of(id).map(|from| PathField::from_path(&from));
                self.relocate(id, &path);
                if let Some(tab) = self.tab_mut(id) {
                    tab.file.stamp = Some(now);
                    tab.file.notice = from.map(|from| Box::new(FileNotice::Moved { from }));
                }
            }
            FileStatus::Changed(now) => {
                self.retire_sleep_file(id);
                // With nothing unsaved there's only history, and its steps don't apply to this. Unsaved edits stay, and say so.
                if has_edits && !self.is_edited(id) {
                    self.clear_history(id, ClearReason::FileChanged);
                }
                let has_edits = self.lists.has_list(id);
                if let Some(tab) = self.tab_mut(id) {
                    if !has_edits {
                        tab.file.stamp = Some(now);
                    }
                    tab.file.notice = Some(Box::new(FileNotice::Changed));
                }
            }
            FileStatus::Missing => {
                if let Some(tab) = self.tab_mut(id) {
                    tab.file.notice = Some(Box::new(FileNotice::Missing));
                }
            }
        }
        true
    }

    /// Every tab's file and what's known of it, for one pass by one thread after a restore. No thread or handle per tab.
    pub fn files_to_look_at(&self) -> Vec<(TabId, PathBuf, Option<FileStamp>)> {
        self.tabs
            .all()
            .iter()
            .filter_map(|tab| Some((tab.id, self.tabs.path_of(tab.id)?, tab.file.stamp)))
            .collect()
    }
}
