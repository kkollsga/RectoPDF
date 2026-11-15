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

use super::{JournalRead, Progress, page_ids, read_journal};
use recto_platform::{FileStamp, file_stamp};
use recto_session::RestoredEdits;
use recto_session::record::{FileRecord, SessionRecord};
use std::collections::HashMap;
use std::path::Path;

/// What an unclaimed journal is to a tab that has claimed one.
pub(super) enum Link {
    /// Not linked to a tab that's claimed its journal: as before.
    None,
    /// It takes over as the tab's (at this place in the record), the tab's claimed one set aside: claim it.
    TakesOver(usize),
    /// Linked, but not the tab's: set aside.
    SetAside,
}

/// **An unclaimed journal, after every tab has claimed the journal it names:** whether it's what a follow's crash left beside a tab's (see the module).
pub(super) fn after_a_follow(
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
    let Some(claimed) = record.tabs[at].journal.clone().filter(|claimed| journals.contains_key(claimed) && *claimed != read.name) else {
        return Link::None;
    };
    let now = record.tabs[at].path.to_path().and_then(|path| file_stamp(&path).ok());
    let is_now = |file: Option<FileRecord>| file.map(FileStamp::from).zip(now).is_some_and(|(file, now)| file.same_contents(now) && file.inode == now.inode);
    let claimed_file = read_journal(&folder.join(&claimed)).ok().flatten().and_then(|(parsed, _)| parsed.header.file);
    let no_older = journals.get(&claimed).is_some_and(|list| header.base_seq >= list.seq);
    if is_now(header.file) && !is_now(claimed_file) && no_older {
        journals.remove(&claimed);
        set_aside(folder, &claimed, progress);
        let tab = &mut record.tabs[at];
        tab.file = header.file;
        tab.page_count = header.file_pages;
        // What the record said of the old list (its file changed under it) isn't so of this one, which is about the file as it is.
        tab.notice = None;
        return Link::TakesOver(at);
    }
    set_aside(folder, &read.name, progress);
    Link::SetAside
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
