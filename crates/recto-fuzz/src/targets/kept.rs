//! Files kept for a history, as a launch and a closed PDF opened again find them: the copy of the pages a save took out, which undo puts back from, in the journals' folder, which something else may have touched since, and the journal that names it, which may say anything. Each input makes a real one, through `keep_pages_of` and a save's rebase, writes the journal that names it, open or closed, then changes the journal's word for it and the file on disk as the bytes say: removed, cut, grown, rewritten in place, replaced by a link, a folder or a copy with the same time, made unreadable, made old, with old strays beside it, and named in the journal by other paths, inside the folder and out.
//!
//! What must hold: no panic, bounded time and memory; nothing outside the journals' folder is removed or changed, whatever a journal names; no page a restored list shows, at any step back or forward, is from a kept file that isn't the one it was, by its stamp, as its steps are rolled off rather than undone wrongly; what's restored is undone and redone all the way; and what's written back reads back.

use crate::guard::{Budget, within};
use crate::input::Take;
use crate::targets::{DocOnly, facts_of};
use recto_control::StatePaths;
use recto_control::record_store::{Flavor, Loaded, RecordStore};
use recto_platform::{FileStamp, file_stamp, local};
use recto_session::edit::PageEdits;
use recto_session::record::{self, AuthorName, FileRecord, JournalHeader, JournalOp, PathField, PlaceDelta};
use recto_session::{HistorySource, Session};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

const BUDGET: Budget = Budget {
    time: Duration::from_secs(60),
    memory: 512 * 1024 * 1024,
};
const PAGES: u32 = 5;
/// What a canary holds, so a change to it shows.
const CANARY: &[u8] = b"not the history's to touch";

static NEXT_FOLDER: AtomicU32 = AtomicU32::new(0);

fn folder() -> PathBuf {
    let base = std::env::var_os("RECTO_FUZZ_STATE").map_or_else(|| std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()), PathBuf::from);
    base.join(format!("kept{}-{}", std::process::id(), NEXT_FOLDER.fetch_add(1, Ordering::Relaxed)))
}

fn five_pages() -> Vec<u8> {
    let pages: Vec<recto_testpdf::Page> = (0..PAGES)
        .map(|at| recto_testpdf::Page::new(recto_testpdf::text_lines(&[&format!("PAGEMARK{at}")])))
        .collect();
    recto_testpdf::pages(&pages)
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

fn set_age(path: &Path, days: u64) {
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400));
    }
}

/// Whether a file kept for a history is the one it was: what the store goes by.
fn is_there(path: &Path, stamp: Option<FileStamp>) -> bool {
    stamp.is_some_and(|want| file_stamp(path).is_ok_and(|now| now.same_contents(want) && now.inode == want.inode))
}

/// Changes to the kept file and the folder around it, as the bytes say.
fn damage(take: &mut Take<'_>, kept: &Path, journals: &Path, other: &Path) {
    for _ in 0..take.u8() % 4 {
        // What changes a file's bytes, time or mode does so only to a file there, never through a link put in its place: that would be this check changing what the link points at, not the store.
        let plain = std::fs::symlink_metadata(kept).is_ok_and(|meta| meta.file_type().is_file());
        let op = take.u8() % 12;
        if !plain && matches!(op, 1 | 2 | 3 | 6 | 7 | 8 | 10) {
            continue;
        }
        match op {
            0 => {
                let _ = std::fs::remove_file(kept);
            }
            1 => {
                // Rewritten in place with its size and time put back: the one change a stamp can't see.
                let before = std::fs::metadata(kept).and_then(|meta| meta.modified()).ok();
                if let Ok(mut bytes) = std::fs::read(kept) {
                    let at = usize::from(take.u16()) % bytes.len().max(1);
                    if let Some(byte) = bytes.get_mut(at) {
                        *byte ^= take.u8() | 1;
                    }
                    let _ = std::fs::write(kept, &bytes);
                    if let (Some(before), Ok(file)) = (before, std::fs::OpenOptions::new().write(true).open(kept)) {
                        let _ = file.set_modified(before);
                    }
                }
            }
            2 => {
                if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(kept) {
                    use std::io::Write;
                    let _ = file.write_all(b"%% more");
                }
            }
            3 => {
                if let Ok(file) = std::fs::OpenOptions::new().write(true).open(kept) {
                    let _ = file.set_len(u64::from(take.u16()) % 512);
                }
            }
            4 => {
                let _ = std::fs::remove_file(kept);
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink(other, kept);
            }
            5 => {
                let _ = std::fs::remove_file(kept);
                let _ = std::fs::create_dir(kept);
            }
            6 => {
                // A copy put in its place, with its time: the same contents to a stamp, another file to its inode.
                let before = std::fs::metadata(kept).and_then(|meta| meta.modified()).ok();
                let copy = kept.with_extension("copy");
                if std::fs::copy(kept, &copy).is_ok() {
                    if let (Some(before), Ok(file)) = (before, std::fs::OpenOptions::new().write(true).open(&copy)) {
                        let _ = file.set_modified(before);
                    }
                    let _ = std::fs::rename(&copy, kept);
                }
            }
            7 => set_age(kept, 40),
            8 => set_mode(kept, 0),
            9 => {
                // An old stray beside it, under a name of the kind, that no history needs.
                let stray = journals.join(record::kept_file_name(
                    FileStamp {
                        size: u64::from(take.u8()),
                        mtime_ns: 1,
                        device: 1,
                        inode: 1,
                    },
                    0,
                ));
                let _ = std::fs::write(&stray, b"%PDF-1.7 stray");
                set_age(&stray, 40);
            }
            10 => {
                let _ = std::fs::write(kept, b"");
            }
            _ => set_mode(journals, 0o500),
        }
    }
}

/// A list a launch or a reopened PDF goes on with: no page it shows, at any step back or forward, is from a kept file that isn't the one it was, and it undoes and redoes all the way. A source whose steps rolled off stays listed, used by nothing.
fn check_list(mut edits: PageEdits) {
    let gone: Vec<u16> = edits
        .kept_sources()
        .filter(|(_, added)| !is_there(&added.path, added.stamp))
        .map(|(source, _)| source)
        .collect();
    let shows_none_gone = |edits: &PageEdits| edits.slots().iter().all(|slot| !gone.contains(&slot.source));
    assert!(shows_none_gone(&edits), "a page from a kept file that isn't what it was");
    let start = edits.slots().to_vec();
    let mut steps = 0;
    while edits.undo() {
        assert!(shows_none_gone(&edits), "undo put back a page from a kept file that isn't what it was");
        steps += 1;
    }
    for _ in 0..steps {
        assert!(edits.redo());
        assert!(shows_none_gone(&edits), "redo put back a page from a kept file that isn't what it was");
    }
    assert_eq!(edits.slots(), start.as_slice(), "redo undoes undo");
}

/// Catches: a journal or a damaged folder that gets a file outside the journals' folder removed or changed, a kept file that isn't what it was used for undo, a restore that can't be undone and redone all the way, and what's written back not reading back.
pub fn check(data: &[u8]) {
    let root = folder();
    within("kept files and the journals that name them", BUDGET, || run(data, &root));
    // Whatever was done to the folder's modes, it all goes.
    set_mode(&root.join("session").join("journals"), 0o700);
    if let Ok(entries) = std::fs::read_dir(root.join("session").join("journals")) {
        for entry in entries.flatten() {
            set_mode(&entry.path(), 0o600);
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn run(data: &[u8], root: &Path) {
    let mut take = Take(data);
    let Ok(paths) = StatePaths::overridden(root) else { return };
    if paths.prepare().is_err() {
        return;
    }
    let journals = root.join("session").join("journals");
    let _ = local::ensure_private_dir(&root.join("session"));
    let _ = local::ensure_private_dir(&journals);
    // The PDF as saved, the file it was before the save, and canaries outside the folder, one with a kept file's name.
    let file = root.join("doc.pdf");
    let original = root.join("original.pdf");
    let canary = root.join(record::kept_file_name(
        FileStamp {
            size: 7,
            mtime_ns: 7,
            device: 7,
            inode: 7,
        },
        0,
    ));
    if std::fs::write(&file, b"saved").is_err() || std::fs::write(&original, five_pages()).is_err() || std::fs::write(&canary, CANARY).is_err() {
        return;
    }
    set_age(&canary, 400);
    let Ok(now) = file_stamp(&file) else { return };
    let closed = take.u8() & 1 == 1;
    let at = u32::from(take.u8()) % (PAGES - 1);
    let count = 1 + u32::from(take.u8()) % (PAGES - 1 - at);
    let kept_pages: Vec<u32> = (at..at + count).collect();
    let target = FileStamp {
        size: u64::from(take.u16()),
        mtime_ns: i64::from(take.u32()),
        device: 1,
        inode: u64::from(take.u8()),
    };
    let store = RecordStore::open(&paths, Flavor::App);
    let kept = store.keep_pages_of(&original, target, &kept_pages);
    let Ok((mut list, _)) = PageEdits::replay(PAGES, None, &[JournalOp::Delete(at, count, AuthorName::Owner)]) else {
        return;
    };
    let pages = PAGES - count;
    if list.rebase_on_saved(pages, kept.clone()).is_none() {
        return;
    }
    // What the journal says of the kept file: as it is, or with its path, stamp, pages or kind as the bytes say.
    let mut said = list.to_record();
    for added in &mut said.added {
        match take.u8() % 8 {
            0 => added.path = PathField::from_path(&canary),
            1 => {
                added.path = PathField::from_path(Path::new(&format!(
                    "{}/../{}",
                    journals.display(),
                    canary.file_name().and_then(|name| name.to_str()).unwrap_or("")
                )))
            }
            2 => added.path = PathField::from_path(&file),
            3 => added.file = file_stamp(&canary).ok().map(FileRecord::from),
            4 => added.holds = Some((0..u32::from(take.u8() % 6)).map(|_| u32::from(take.u8())).collect()),
            5 => added.kept = !added.kept,
            6 => added.page_count = u32::from(take.u8()),
            _ => {}
        }
    }
    let name = record::journal_file_name(Some(now), &file, None);
    let header = JournalHeader::new(1, &file, Some(now), pages, 0);
    let journal_name = if closed {
        record::closed_journal_name(&name).unwrap_or_else(|| name.clone())
    } else {
        name.clone()
    };
    if local::write_private_file(&journals.join(&journal_name), &record::journal_start(&header, Some(&said))).is_err() {
        return;
    }
    let facts = Arc::new(facts_of(pages));
    let mut session = Session::new(DocOnly(Arc::clone(&facts)));
    if !closed {
        if session.open_with_facts(&file, (*facts).clone(), None, None).is_err() {
            return;
        }
        let mut main = session.to_record();
        if let Some(tab) = main.tabs.first_mut() {
            tab.file = Some(FileRecord::from(now));
            tab.page_count = pages;
            tab.journal = Some(name.clone());
        }
        if store.write_record(&main, &PlaceDelta::default(), false).is_err() {
            return;
        }
    }
    drop(store);
    if let Some(kept) = &kept {
        damage(&mut take, &kept.path, &journals, &file);
    }
    for _ in 0..2 {
        let store = RecordStore::open(&paths, Flavor::App);
        if let Loaded::Restored(restored) = store.load() {
            for list in restored.journals.values() {
                check_list(list.edits.clone());
            }
            let mut session = Session::new(DocOnly(Arc::clone(&facts)));
            assert!(
                session.restore(&restored.record, restored.journals).is_ok(),
                "what a load gives can be restored"
            );
            let written = session.to_record();
            if store.write_record(&written, &PlaceDelta::default(), false).is_ok() {
                let by_name: HashMap<String, record::EditsRecord> = session.lists_by_journal();
                store.next_write_whole();
                let _ = store.write_record(&written, &PlaceDelta::default(), false);
                let _ = store.verify(&written, &by_name);
            }
        }
        if closed && let Some(taken) = store.history_of(Some(now), &file) {
            check_list(taken.restored.edits);
        }
    }
    assert_eq!(
        std::fs::read(&canary).ok().as_deref(),
        Some(CANARY),
        "a file outside the journals' folder was changed or removed"
    );
    assert!(std::fs::read(&file).is_ok_and(|bytes| bytes == b"saved"), "the PDF itself was touched");
}
