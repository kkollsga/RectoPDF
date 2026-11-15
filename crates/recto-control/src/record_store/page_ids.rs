//! **Journals' sidecars:** what an edited tab's pages are, for the page matcher, in `pages-<key>.ids` beside its `journal-<key>.log` (the format is
//! `recto_session::edit::identity::sidecar`'s; the coordinator's decision B1, 2026-11-15). A journal is byte for byte what it is without them, so an app
//! from before reads every journal as it did.
//!
//! **Tied to its journal:** a sidecar is taken in for a list only when its header names that journal, that version of its file (size, time and file
//! number, as the journal's own header says) and its page count; any other is stale or another's, and goes. **Goes with its journal:** removed when the
//! journal is retired, replaced, closed or removed, and at a launch any sidecar whose journal isn't there goes too, which covers every way a journal is
//! renamed or set aside (`.not-merged`, `.unreadable`, `closed-`, a crash between the two). **Losing one costs only following:** its pages come back
//! unread, or not named, and their edits are held when the file changes, as before the matcher; so nothing here is said when it fails.
//!
//! **What an older app does with one:** 0.1.0 (`git show v0.1.0:crates/recto-control/src/record_store.rs`) reads a journals folder for `journal-*.log`
//! (`is_journal_name`), prunes `closed-*` and `kept-*`, removes its own temp files by the prefixes `record.`, `journal-`, `closed-`, `kept-`, `sleep-`
//! and `places.log`, and prunes `sleep-*.bin`: none of which a `pages-*.ids` is. It copies every file of a folder when it goes forward. So it leaves a
//! sidecar alone, and a sidecar it left beside a journal it then changed is judged by its header here.

use super::{Progress, read_ours, replace, sync};
use recto_platform::local;
use recto_session::edit::PageEdits;
use recto_session::edit::identity::{self, sidecar};
use recto_session::record::{JournalHeader, is_journal_name};
use recto_session::{PagesAppend, TabId};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

/// What's known of a sidecar this process has read or written: its good length and its header.
pub(super) type Known = std::collections::HashMap<String, (u64, sidecar::Header)>;

/// A journal's sidecar's name: `journal-<key>.log` is `pages-<key>.ids`. `None` for a name that isn't a journal's.
pub(super) fn name_of(journal: &str) -> Option<String> {
    let key = journal.strip_prefix("journal-")?.strip_suffix(".log")?;
    is_journal_name(journal).then(|| format!("pages-{key}.ids"))
}

/// Whether a name is a journal's sidecar's, and of which journal.
pub(super) fn journal_of(name: &str) -> Option<String> {
    let key = name.strip_prefix("pages-")?.strip_suffix(".ids")?;
    let journal = format!("journal-{key}.log");
    is_journal_name(&journal).then_some(journal)
}

/// Removes a journal's sidecar, if it has one.
pub(super) fn remove(folder: &Path, journal: &str, progress: &mut Progress) {
    if let Some(name) = name_of(journal) {
        let _ = std::fs::remove_file(folder.join(&name));
        progress.sidecars.remove(&name);
    }
}

/// **A launch's sweep:** every sidecar whose journal isn't there goes, and temp files of ours a crash left.
pub(super) fn sweep(folder: &Path) {
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let orphan = journal_of(name).is_some_and(|journal| std::fs::symlink_metadata(folder.join(journal)).is_err());
        let temp = name.starts_with("pages-")
            && name
                .rsplit('.')
                .next()
                .is_some_and(|tail| tail.starts_with("tmp") && tail[3..].bytes().all(|byte| byte.is_ascii_digit()));
        if (orphan || temp) && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// **Takes a journal's sidecar in for a list restored from it,** if it's that journal's and that version's; one that isn't goes. Read with its cap.
pub(super) fn take_in(folder: &Path, journal: &str, header: &JournalHeader, edits: &mut PageEdits, progress: &mut Progress) {
    let Some(name) = name_of(journal) else { return };
    let path = folder.join(&name);
    let Ok(Some(bytes)) = read_ours(&path, sidecar::MAX_BYTES) else {
        // Past its cap, or not ours to read: nothing taken. One past its cap is no sidecar of ours, and goes.
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > sidecar::MAX_BYTES as u64) {
            let _ = std::fs::remove_file(&path);
        }
        return;
    };
    match sidecar::decode(&bytes) {
        Some(read) if read.header.is_of(journal, header) => {
            identity::restored(edits, &read.pages, read.document);
            progress.sidecars.insert(name, (read.good as u64, read.header));
        }
        _ => {
            let _ = std::fs::remove_file(&path);
            progress.sidecars.remove(&name);
        }
    }
}

/// **Writes what's waiting for sidecars,** after the journals' appends: each to the journal its tab's ops went under in this work (`written`), and only
/// for a journal that's there. What became of each goes in `done`; a tab whose journal isn't there yet gets nothing, and its pages wait.
pub(super) fn write_all(
    folder: &Path,
    progress: &mut Progress,
    pages: &[PagesAppend],
    written: &[(TabId, u64, String)],
    force: bool,
    done: &mut Vec<(TabId, sidecar::Waiting, sidecar::Done)>,
) {
    for append in pages {
        let journal = written
            .iter()
            .rev()
            .find(|(tab, _, _)| *tab == append.tab)
            .map_or(append.journal.as_str(), |(_, _, name)| name.as_str());
        if !progress.journal_bytes.contains_key(journal) {
            continue;
        }
        let header = sidecar::Header {
            journal: journal.to_string(),
            ..append.header.clone()
        };
        if let Some(outcome) = write_one(folder, progress, journal, &header, &append.waiting, force) {
            done.push((append.tab, append.waiting.clone(), outcome));
        }
    }
}

/// One sidecar's write: whole, in place of what's there, or appended where what's there is this journal's and this version's and reads whole. `None`
/// if the disk refused, to try again.
fn write_one(
    folder: &Path,
    progress: &mut Progress,
    journal: &str,
    header: &sidecar::Header,
    waiting: &sidecar::Waiting,
    force: bool,
) -> Option<sidecar::Done> {
    let name = name_of(journal)?;
    let path = folder.join(&name);
    if waiting.whole {
        let mut bytes = header.encode()?;
        bytes.extend_from_slice(&waiting.bytes);
        if bytes.len() > sidecar::MAX_BYTES {
            return Some(sidecar::Done::Written);
        }
        replace(&path, &bytes, force).ok()?;
        progress.wrote_bytes += bytes.len() as u64;
        progress.sidecars.insert(name, (bytes.len() as u64, header.clone()));
        return Some(sidecar::Done::Written);
    }
    let good = match progress.sidecars.get(&name) {
        Some((good, known)) if known == header => Some(*good),
        Some(_) => None,
        None => match read_ours(&path, sidecar::MAX_BYTES) {
            // Nothing there: a new sidecar starts with its header.
            Ok(None) => Some(0),
            Ok(Some(bytes)) => sidecar::decode(&bytes).filter(|read| read.header == *header).map(|read| read.good as u64),
            Err(()) => None,
        },
    };
    let Some(good) = good else { return Some(sidecar::Done::WantsWhole) };
    let mut bytes = if good == 0 { header.encode()? } else { Vec::new() };
    bytes.extend_from_slice(&waiting.bytes);
    let end = good + bytes.len() as u64;
    if end > sidecar::MAX_BYTES as u64 {
        return Some(sidecar::Done::WantsWhole);
    }
    let appended = (|| -> std::io::Result<()> {
        let mut file = local::open_private_for_writing(&path, false)?;
        // A torn tail from a crash, or a write that failed, is cut off, and what's good is built on.
        file.set_len(good)?;
        file.seek(SeekFrom::Start(good))?;
        file.write_all(&bytes)?;
        sync(&file, force)
    })();
    appended.ok()?;
    progress.wrote_bytes += bytes.len() as u64;
    progress.sidecars.insert(name, (end, header.clone()));
    Some(sidecar::Done::Written)
}

#[cfg(test)]
mod tests;
