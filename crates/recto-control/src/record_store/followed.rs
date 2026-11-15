//! **A follow's crash left two journals for one tab** (the page matcher, slice 2). When a tab's edits follow their pages onto a new version of its file,
//! its journal starts again under the new version's name, as after a save: the fresh journal is written whole, then the old one goes, then the new
//! one's sidecar, and the record later. A crash between the first two leaves both, with the record naming the old: the tab takes the old by its name,
//! and the new one, unclaimed and with no save noted in it, would otherwise come back as a second tab on the same file.
//!
//! **The link is the tab's own id** in the journal's header, which every journal of a tab carries (`replaces` is in the store's work, never on disk),
//! with the tab's path: nothing fires on two journals that merely share a path. The unclaimed journal takes over only if its file is the file as it is
//! now (size, time and inode), the tab's claimed journal's isn't, and it's no older than the tab's list (its base at or past the list's steps). Then the
//! old journal is **set aside, not removed** (`.not-merged`, the store's existing name for a journal it keeps and won't use: it says nothing to the
//! person, nothing prunes it, and 0.1.0 leaves it alone as it isn't a journal's name), with its sidecar gone (an orphan's), so if this misjudges, the
//! person's earlier list is still on disk. A linked journal that doesn't take over is set aside the same way: never a second tab on the file.
//!
//! **Two lists of one version** (review M2: a journal started again under another name, with a crash before the old one's removal): the newer by its
//! steps takes over, as nothing about the file argues for the older. And a list set aside other than the old one a follow superseded, with edits not
//! all in the one kept, is said on the tab (`FileNotice::EditsSetAsideAfterQuit`, kept out of the record): on `main` it came back as a second tab, so
//! nothing was hidden, and nothing a person made is put out of reach unsaid now either.

use super::{JournalRead, Progress, claim, page_ids, read_journal, restored_from};
use recto_platform::{FileStamp, file_stamp};
use recto_session::edit::{PageEdits, identity};
use recto_session::record::{FileNotice, FileRecord, JournalHeader, SessionRecord, TabEntry};
use recto_session::RestoredEdits;
use std::collections::HashMap;
use std::path::Path;

/// **An unclaimed journal, after every tab has claimed the journal it names,** taken care of here or given back: one a save's crash left beside the
/// journal it was carried over to (already in `journals` by its name) is done with; one a follow's crash left is claimed by its tab, or set aside.
pub(super) fn unclaimed(
    folder: &Path,
    read: JournalRead,
    record: &mut SessionRecord,
    taken: &[bool],
    journals: &mut HashMap<String, RestoredEdits>,
    progress: &mut Progress,
) -> Option<JournalRead> {
    if journals.contains_key(&read.name) {
        return None;
    }
    match after_a_follow(folder, &read, record, taken, journals, progress) {
        Link::TakesOver { at, said } => {
            claim(folder, read, record, at, progress, journals);
            if said && let Some(kept) = record.tabs[at].journal.clone() {
                mark(journals, &kept);
            }
            None
        }
        Link::SetAside => None,
        Link::None => Some(read),
    }
}

/// What an unclaimed journal is to a tab that has claimed one.
enum Link {
    /// Not linked to a tab that's claimed its journal: as before.
    None,
    /// It takes over as the tab's (at this place in the record), the tab's claimed one set aside: claim it. `said`: the one set aside has edits not
    /// all in this one, so the tab says another list was kept to one side.
    TakesOver { at: usize, said: bool },
    /// Linked, but not the tab's: set aside.
    SetAside,
}

/// What's known of a journal for the rule: its file, its steps, where it last started again from a whole list, and whether it holds unsaved edits.
struct Steps {
    file: Option<FileRecord>,
    seq: u64,
    base_seq: u64,
    edited: bool,
}

impl Steps {
    /// Every step of `self` is in `other`, known by sequence: the two are one tab's lists about one version, and `other` started again from the
    /// whole list at or after `self`'s last step (a list started again under another name carries every step before it).
    fn all_in(&self, other: &Steps) -> bool {
        self.file.is_some() && self.file == other.file && self.seq <= other.base_seq
    }

    /// Set aside, it takes something from the person they aren't told of: it holds edits, and not all of them are in the list kept.
    fn would_be_lost(&self, kept: &Steps) -> bool {
        self.edited && !self.all_in(kept)
    }
}

/// **An unclaimed journal, after every tab has claimed the journal it names:** whether it's what a crash left beside a tab's, and which of the two the
/// tab keeps. The rule, in this order (review M2):
/// 1. A follow's: the unclaimed one is about the file as it is now, the claimed one isn't, and it's no older than the claimed list (its base at or past
///    the claimed list's steps). It takes over, and the claimed one is the old list it superseded: set aside, unsaid.
/// 2. A list started again under another name for the same version: the newer by its steps takes over, the older is set aside.
/// 3. Otherwise the tab keeps the one it claimed, and the unclaimed one is set aside.
///
/// Set aside in 2 or 3 with edits not all in the one kept (by sequence, [`Steps::all_in`]), the tab says so
/// ([`recto_session::record::FileNotice::EditsSetAsideAfterQuit`]): nothing a person made is put out of reach unsaid.
fn after_a_follow(
    folder: &Path,
    read: &JournalRead,
    record: &mut SessionRecord,
    taken: &[bool],
    journals: &mut HashMap<String, RestoredEdits>,
    progress: &mut Progress,
) -> Link {
    let header = &read.parsed.header;
    let Some(at) = record
        .tabs
        .iter()
        .enumerate()
        .position(|(at, tab)| taken.get(at).copied().unwrap_or(false) && tab.id == header.tab && tab.path == header.path)
    else {
        return Link::None;
    };
    let Some(claimed) = record.tabs[at]
        .journal
        .clone()
        .filter(|claimed| journals.contains_key(claimed) && *claimed != read.name)
    else {
        return Link::None;
    };
    let now = record.tabs[at].path.to_path().and_then(|path| file_stamp(&path).ok());
    let is_now = |file: Option<FileRecord>| is_the_file(file, now);
    let claimed_header = read_journal(&folder.join(&claimed)).ok().flatten().map(|(parsed, _)| parsed.header);
    let Some(claimed_list) = journals.get(&claimed) else { return Link::None };
    let kept = Steps {
        file: claimed_header.as_ref().and_then(|header| header.file),
        seq: claimed_list.seq,
        base_seq: claimed_header.as_ref().map_or(0, |header| header.base_seq),
        edited: claimed_list.edits.is_edited(),
    };
    let unclaimed = restored_from(&read.parsed).map(|list| Steps {
        file: header.file,
        seq: list.seq,
        base_seq: header.base_seq,
        edited: list.edits.is_edited(),
    });
    let Some(unclaimed) = unclaimed else {
        // Doesn't replay: kept to one side, its tab keeping its own, and said as nothing of it can be shown.
        set_aside(folder, &read.name, progress);
        mark(journals, &claimed);
        return Link::SetAside;
    };
    let follow = is_now(header.file) && !is_now(kept.file) && header.base_seq >= kept.seq;
    let newer_of_one_version = kept.file.is_some() && kept.file == unclaimed.file && unclaimed.seq > kept.seq;
    if follow || newer_of_one_version {
        let said = !follow && kept.would_be_lost(&unclaimed);
        journals.remove(&claimed);
        set_aside(folder, &claimed, progress);
        let tab = &mut record.tabs[at];
        tab.file = header.file;
        tab.page_count = header.file_pages;
        if follow {
            // What the record said of the old list (its file changed under it) isn't so of this one, which is about the file as it is.
            tab.notice = None;
        }
        return Link::TakesOver { at, said };
    }
    set_aside(folder, &read.name, progress);
    if unclaimed.would_be_lost(&kept) {
        mark(journals, &claimed);
    }
    Link::SetAside
}

/// Whether a journal's file is the file as it is (size, time and inode).
fn is_the_file(file: Option<FileRecord>, now: Option<FileStamp>) -> bool {
    file.map(FileStamp::from)
        .zip(now)
        .is_some_and(|(file, now)| file.same_contents(now) && file.inode == now.inode)
}

/// **A list claimed that followed its file and wasn't told, under a record saying the file changed** (review N1): a record written in the moment
/// between noticing the change and the follow, or naming the old journal a crash then left gone. The list is about the file as it is, so the record's
/// "changed" isn't so of it: it goes, and the tab says the edits followed (`recto_session::version_change::untold_notice`) rather than holding them.
pub(super) fn untold_over_changed(tab: &mut TabEntry, header: &JournalHeader, edits: &PageEdits) {
    if matches!(tab.notice, Some(FileNotice::Changed))
        && identity::followed_untold(edits)
        && is_the_file(header.file, tab.path.to_path().and_then(|path| file_stamp(&path).ok()))
    {
        tab.notice = None;
    }
}

/// The list kept for a tab is told another of its lists was set aside beside it.
fn mark(journals: &mut HashMap<String, RestoredEdits>, kept: &str) {
    if let Some(list) = journals.get_mut(kept) {
        list.set_aside_beside = true;
    }
}

/// A journal kept to one side under the store's `.not-merged` name, its sidecar gone with it.
fn set_aside(folder: &Path, name: &str, progress: &mut Progress) {
    let path = folder.join(name);
    let _ = std::fs::rename(&path, path.with_extension("not-merged"));
    progress.journal_bytes.remove(name);
    page_ids::remove(folder, name, progress);
}

#[cfg(test)]
mod tests;
