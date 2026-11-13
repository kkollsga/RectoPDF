//! What became of a tab's file: notices and status.

use super::*;

impl<L: DocLoader> Session<L> {
    /// What a look at a tab's file found, from [`record::look_at_file`], made on whatever thread could afford it. Nothing's written to any PDF. A file that moved is followed. One that's changed under edits keeps the stamp they were made on, and the tab knows its edits don't fit it ([`Notices::fit`]), so the edits' own checks go on saying so.
    ///
    /// **Says whether anything was new.** A look that finds what's already known (edits that don't fit their file, finding it still another version; a file
    /// already said to be gone, finding it gone) changes nothing, while one back from gone is news, and callers forget what's held of the file and read it again
    /// only on new: every return to the front and every show of such a tab would otherwise read the file's structure again (rule 1).
    pub fn apply_file_status(&mut self, id: TabId, status: FileStatus) -> bool {
        // A confined tab whose path no longer leads into its folders (one swapped for a link after the open) is a missing file, whatever the look found
        // where the path leads now: see [`confined`] and [`Self::path_of`].
        let status = if self.tabs.tab(id).is_some() && self.tabs.path_of(id).is_none() {
            FileStatus::Missing
        } else {
            status
        };
        let Some(tab) = self.tabs.tab(id) else { return false };
        let has_edits = self.lists.has_list(id);
        let edited = self.lists.is_edited(id);
        let (stamp, missing) = (tab.file.stamp, matches!(tab.file.notice(), Some(FileNotice::Missing)));
        // Under edits that don't fit, the version last taken in, found again, is nothing new; any other is, as what's held is of that one (F2).
        let fit = self.notices().fit(id);
        let elsewhere = tab.file.elsewhere().filter(|_| !fit.fits());
        let seen_again = |now: &FileStamp| elsewhere.and_then(|elsewhere| elsewhere.seen).is_some_and(|seen| seen.same_contents(*now));
        let known = match &status {
            FileStatus::NotLooked => true,
            FileStatus::Changed(now) => seen_again(now) && !missing,
            FileStatus::Missing => missing,
            // With no stamp a file found is never taken as the edits' own. Back from gone is news, and so is a version not yet taken in.
            FileStatus::Same(now) => has_edits && stamp.is_none() && !missing && (!edited || seen_again(now)),
            FileStatus::Moved(..) => false,
        };
        if known {
            return false;
        }
        // A file changed or gone takes its generated contents with it, so the sidebar shows what an agent's `contents --generated` finds, which goes by the file as it is now.
        // So does another version under edits that name none.
        let other_version_unnamed = edited && stamp.is_none() && matches!(&status, FileStatus::Same(now) if !seen_again(now));
        if matches!(status, FileStatus::Changed(_) | FileStatus::Missing) || other_version_unnamed {
            self.generated.forget(id);
            // Folds are by entry number, which another version's contents don't share.
            if let Some(tab) = self.tabs.tab_mut(id) {
                tab.folds = None;
            }
        }
        match status {
            FileStatus::NotLooked => {}
            FileStatus::Same(now) => {
                if let Some(tab) = self.tabs.tab_mut(id) {
                    // Edits about another version than what's held, found to fit again, are news whatever's said: what's held is of the other one.
                    let was = (tab.file.stamp, tab.file.notice().is_some(), tab.file.elsewhere().is_some());
                    if other_version_unnamed {
                        // What's read next is this version, and it's what a look will find again; the edits stay about none, and what's known against
                        // them stays known until a read of this one says otherwise.
                        let file_changed = matches!(fit, EditsFit::FileChanged | EditsFit::FileChangedVersionUnknown);
                        let other_length = elsewhere.is_some_and(|elsewhere| elsewhere.other_length);
                        tab.file.set_elsewhere(Some(fit::Elsewhere {
                            seen: Some(now),
                            seen_pages: None,
                            file_changed,
                            other_length,
                        }));
                    }
                    if stamp.is_some() || !has_edits {
                        // The version the edits were made on, or the file as it is for a tab with none: so it fits.
                        tab.file.take_version(Some(now));
                    }
                    // Back to what edits are about, or back from gone. With none, the stamp's moved on to what the file is now, so being the same says nothing: it goes on saying it changed until it's been read, and left.
                    if matches!(tab.file.notice(), Some(FileNotice::Missing))
                        || (has_edits && stamp.is_some() && matches!(tab.file.notice(), Some(FileNotice::Changed)))
                    {
                        tab.file.set_notice(None);
                    }
                    return other_version_unnamed || was != (tab.file.stamp, tab.file.notice().is_some(), tab.file.elsewhere().is_some());
                }
            }
            FileStatus::Moved(path, now) => {
                // Another tab having it means this one isn't a move of that file so much as a second look at it. And a quiet run's tab doesn't
                // follow its file out of the run's folders: see [`confined`].
                if self.tabs.for_path(&path).is_some_and(|other| other != id) || !self.tabs.confinement().admits(&path) {
                    if let Some(tab) = self.tabs.tab_mut(id) {
                        tab.file.set_notice(Some(FileNotice::Missing));
                    }
                    return true;
                }
                let from = self.tabs.named_path(id).map(|from| PathField::from_path(&from));
                self.relocate(id, &path);
                if let Some(tab) = self.tabs.tab_mut(id) {
                    tab.file.take_version(Some(now));
                    tab.file.set_notice(from.map(|from| FileNotice::Moved { from }));
                }
            }
            FileStatus::Changed(now) => {
                self.retire_sleep_file(id);
                // With nothing unsaved there's only history, and its steps don't apply to this. Unsaved edits stay, and say so.
                if has_edits && !edited {
                    self.clear_history(id, ClearReason::FileChanged);
                }
                let has_edits = self.lists.has_list(id);
                if let Some(tab) = self.tabs.tab_mut(id) {
                    if has_edits {
                        // The stamp stays the version the edits were made on, and they don't fit what's there now.
                        tab.file.set_elsewhere(Some(fit::Elsewhere {
                            seen: Some(now),
                            seen_pages: None,
                            file_changed: true,
                            other_length: false,
                        }));
                    } else {
                        tab.file.take_version(Some(now));
                    }
                    tab.file.set_notice(Some(FileNotice::Changed));
                }
            }
            FileStatus::Missing => {
                if let Some(tab) = self.tabs.tab_mut(id) {
                    tab.file.set_notice(Some(FileNotice::Missing));
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
            .filter_map(|tab| Some((tab.id, self.tabs.named_path_of(tab)?, tab.file.stamp)))
            .collect()
    }
}
