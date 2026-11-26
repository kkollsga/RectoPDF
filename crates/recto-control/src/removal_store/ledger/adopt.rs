//! **Adoption, once, when the ledger is first made:** what the journals already on disk say about which file was where. Each journal's first line (at most 8 KiB,
//! the file opened with no link followed, and not a pipe waited on) names the path a tab had and the version of the file it was about; a save that was about to land is named too.
//! Nothing else is taken from a journal, and nothing of a document is opened.

use super::{Config, canonical_version};
use recto_core::removal::{Entry, FileVersion, Link};
use recto_platform::FileStamp;
use recto_session::record::{is_closed_journal_name, is_journal_name, journal_header};
use std::io::Read as _;
use std::path::Path;
use std::time::Instant;

use crate::removal_store::journals::HEADER_BYTES;
/// The most directory entries looked at in one folder, whatever is in it.
const MAX_ENTRIES: usize = 65_536;

/// The links adopted from the journals of both sessions, and whether it stopped before the end: at the header cap, the time, or the folder's entry cap.
pub(super) fn adopt(root: &Path, config: &Config) -> (Vec<Entry>, bool) {
    let deadline = Instant::now() + config.adoption_time;
    let mut found = Vec::new();
    let mut read = 0usize;
    for session in ["session", "session-windowless"] {
        let Ok(entries) = std::fs::read_dir(root.join(session).join("journals")) else {
            continue;
        };
        for (seen, entry) in entries.enumerate() {
            if seen >= MAX_ENTRIES {
                return (found, true);
            }
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !is_journal_name(name) && !is_closed_journal_name(name) {
                continue;
            }
            if read >= config.adoption_headers || Instant::now() >= deadline {
                return (found, true);
            }
            read += 1;
            found.extend(from_header(&entry.path()));
        }
    }
    (found, false)
}

/// What one journal says, if it is a plain file with a header that reads.
fn from_header(path: &Path) -> Vec<Entry> {
    let Some(bytes) = head(path) else { return Vec::new() };
    let Some(header) = journal_header(&bytes) else { return Vec::new() };
    // A path a hostile header names is checked as any other: absolute, and what it names is looked at only by stat.
    let Some(path) = header.path.to_path().filter(|p| p.is_absolute()) else {
        return Vec::new();
    };
    let (file, saving) = (header.file.map(FileStamp::from), header.saving.map(FileStamp::from));
    let at = |stamp: FileStamp| canonical_version(&path, stamp);
    let mut out = Vec::new();
    if let Some((real, version)) = file.and_then(at) {
        out.push(Entry::Link(Link::At { version, path: real }));
    } else if let Some((real, now)) = saving.and_then(at) {
        out.push(Entry::Link(Link::At { version: now, path: real }));
        // The file the journal was of, on the same volume as the one that landed: the save is the step between them.
        if let Some(file) = file.filter(|file| file.device == saving.map_or(u64::MAX, |s| s.device)) {
            let from = FileVersion::new(file, now.volume);
            out.push(Entry::Link(Link::Saved { from, to: now }));
        }
    }
    out
}

fn head(path: &Path) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    recto_platform::local::open_private_file(path)
        .ok()?
        .take(HEADER_BYTES as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}
