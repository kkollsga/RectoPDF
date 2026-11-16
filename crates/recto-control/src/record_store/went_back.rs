//! **A copy that went back to an older app and came forward again** (`docs/RELEASING.md`, a bad release; `tools/release/check-carry.sh`, runs C to
//! F). An app from before highlights (0.1.0) leaves a journal of a newer version (one with highlights, [`record::MARKED_JOURNAL_VERSION`]) alone,
//! byte for byte, and writes the record with that tab naming no journal and saying `edits_unreadable`, which was so for it. An edit made there then
//! starts a list of its own from the file as it is, under another name beside the newer one (`journal-<key>-<n>.log`, the newer one being
//! `journal-<key>.log`). Coming forward, both are this tab's.
//!
//! - **D, no edit there:** the journal is claimed by its file and read whole, so the older app's notice isn't so ([`unreadable_answered`]).
//! - **Two lists, which combine** (case 1): the newer app's list, N, is the file's own pages, in place and unturned, with only highlights on them,
//!   and the older app's, O, started from the file with nothing but page steps. Then O's steps follow N's, in the order they were made: a highlight
//!   belongs to its page by what the page is, so a turn, a move or a removal made in the older app takes it along. The person gets every edit and no
//!   notice. On disk, N (under its `.log` name again, if 0.1.1 had set it aside as `.not-merged`) has O's lines appended, its own bytes its prefix,
//!   and O goes to a closed history's name, byte for byte, aged out with them in 30 days, its every step in N. A crash between any two steps is taken
//!   up by the next launch where it stopped: O's steps already at N's end aren't appended again ([`merge_plan`]).
//! - **Two lists, which don't** (case 2), anything not provably case 1: the tab keeps O, what the person last worked on, and N is kept as
//!   `journal-<key>.earlier`, byte for byte. Each launch says so again from that file ([`FileNotice::EditsSetAsideWentBack`], never in a record, so no
//!   older app misreads one and it survives another trip back), while the tab's list is that O. Once it isn't (saved, discarded, the tab closed), the
//!   kept list goes to a closed history's name, and the 30 days closed histories have start then. Nothing else prunes it.
//!
//! The links are those `followed` uses (the tab's id and path in both headers) and both lists about the one version of the file, found by the names
//! the two apps give them: nothing is looked at for a tab without an older app's list, and nothing at all with no candidate in the folder. Reads count
//! towards the launch's total, and replays go by the replay's own budget.

use super::{JournalRead, Progress, claim, close_journal, local, page_ids, read_journal, sync};
use recto_platform::file_stamp;
use recto_session::RestoredEdits;
use recto_session::edit::PageEdits;
use recto_session::record::{self, FileNotice, JournalHeader, JournalOp, ParsedJournal, SessionRecord, TabEntry, closed_journal_name};
use std::collections::{HashMap, HashSet};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

/// The name an earlier list is kept under, beside its tab's (case 2).
const EARLIER: &str = "earlier";
/// The name 0.1.1 set one aside under, which the store uses for a list it keeps and won't read (`followed`).
const NOT_MERGED: &str = "not-merged";
/// Most names of lists set aside a launch looks at. Past it, the rest wait for a later launch: a hostile folder costs a bounded look.
pub(super) const MAX_SET_ASIDE_NAMES: usize = 1024;
/// Most places a merge a crash cut short is looked for at N's end: each is a comparison and a replay, within the replay's budget.
const MAX_PARTIAL_TRIES: usize = 8;

/// **A record's "couldn't be read" against the journal as it reads now** (`fault:D.notice.edits_unreadable`, found by `check-carry.sh` on
/// 2026-11-15). The tab named no journal, said its edits couldn't be read, and has now claimed one that read whole, of a version an app reading only
/// [`record::CURRENT_VERSION`]'s journals leaves alone: that app's words about this journal, and not so here. The caller says it again when what's
/// claimed has lines that couldn't be used.
///
/// Kept, as this app's own and still so: a record naming the journal it says this of (one that had lines it couldn't use, written clean by `claim`,
/// said until the person has seen it), and one claiming a journal of the version every app reads (a list started after a journal of this tab's
/// couldn't be read, its record not yet written: what couldn't be read is kept beside it, as the notice says).
pub(super) fn unreadable_answered(tab: &mut TabEntry, named_it: bool, header: &JournalHeader) {
    if !named_it && header.version > record::CURRENT_VERSION && matches!(tab.notice, Some(FileNotice::EditsUnreadable)) {
        tab.notice = None;
    }
}

/// The key of a name, `journal-<16 hex>`, with what follows it.
fn split(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("journal-")?;
    let key = rest.get(..16)?;
    key.bytes().all(|byte| byte.is_ascii_hexdigit()).then(|| (key, &rest[16..]))
}

/// An older app's list's name, `journal-<key>-<n>.log`: its key.
fn older_key(name: &str) -> Option<&str> {
    let (key, tail) = split(name)?;
    let number = tail.strip_prefix('-')?.strip_suffix(".log")?;
    (!number.is_empty() && number.len() <= 10 && number.bytes().all(|byte| byte.is_ascii_digit())).then_some(key)
}

/// A newer app's list's own name, `journal-<key>.log`: its key.
fn newer_key(name: &str) -> Option<&str> {
    split(name).filter(|(_, tail)| *tail == ".log").map(|(key, _)| key)
}

/// **A list set aside that may be an earlier one:** `journal-<key>.not-merged` (0.1.1's) or `journal-<key>[-<n>].earlier` (this). Its key.
pub(super) fn set_aside_key(name: &str) -> Option<&str> {
    let (key, tail) = split(name)?;
    if tail == format!(".{NOT_MERGED}") {
        return Some(key);
    }
    let number = tail.strip_suffix(&format!(".{EARLIER}"))?;
    (number.is_empty()
        || number
            .strip_prefix('-')
            .is_some_and(|n| !n.is_empty() && n.len() <= 10 && n.bytes().all(|byte| byte.is_ascii_digit())))
    .then_some(key)
}

/// Where a newer app's list was found.
enum Found {
    /// Under its own name, claimed by no tab.
    Own(JournalRead),
    /// Set aside, under this name.
    Aside(String, ParsedJournal),
}

/// **The earlier lists of tabs whose list is an older app's,** after every tab has claimed the journal it names: merged in (case 1) or kept beside
/// it and said (case 2), each newer list found under its own name taken out of `unclaimed`. And a kept one no tab's list is O for any more goes to a
/// closed history's name. See the module's notes.
#[allow(clippy::too_many_arguments)]
pub(super) fn take_earlier(
    folder: &Path,
    set_aside: &[String],
    record: &mut SessionRecord,
    taken: &[bool],
    journals: &mut HashMap<String, RestoredEdits>,
    unclaimed: &mut Vec<JournalRead>,
    progress: &mut Progress,
    read_total: &mut u64,
) {
    let newer_unclaimed = unclaimed
        .iter()
        .any(|read| read.parsed.header.version >= record::MARKED_JOURNAL_VERSION && newer_key(&read.name).is_some());
    if !newer_unclaimed && set_aside.is_empty() {
        return;
    }
    // Tabs whose list is an older app's, by its key. Only when there's something to find.
    let older: HashMap<String, usize> = record
        .tabs
        .iter()
        .enumerate()
        .filter(|(at, _)| taken.get(*at).copied().unwrap_or(false))
        .filter_map(|(at, tab)| {
            let name = tab.journal.as_deref().filter(|name| journals.contains_key(*name))?;
            Some((older_key(name)?.to_string(), at))
        })
        .collect();
    let mut done: HashSet<usize> = HashSet::new();
    let mut found: Vec<(usize, Found)> = Vec::new();
    let mut at_read = 0;
    while at_read < unclaimed.len() {
        let read = &unclaimed[at_read];
        let tab = newer_key(&read.name)
            .filter(|_| read.parsed.header.version >= record::MARKED_JOURNAL_VERSION)
            .and_then(|key| older.get(key).copied())
            .filter(|at| !done.contains(at));
        match tab {
            Some(at) => {
                done.insert(at);
                found.push((at, Found::Own(unclaimed.remove(at_read))));
            }
            None => at_read += 1,
        }
    }
    for name in set_aside {
        let Some(key) = set_aside_key(name) else { continue };
        match older.get(key).copied() {
            Some(at) if !done.contains(&at) => {
                let path = folder.join(name);
                *read_total += std::fs::metadata(&path).map_or(0, |meta| meta.len());
                if *read_total > super::MAX_JOURNAL_BYTES_AT_LAUNCH {
                    continue;
                }
                // Not a journal this build reads (not one, damaged, or a newer app's): not this tab's, and left as it is.
                if let Ok(Some((parsed, _))) = read_journal(&path) {
                    done.insert(at);
                    found.push((at, Found::Aside(name.clone(), parsed)));
                }
            }
            Some(_) => {}
            // Kept for a tab whose list isn't an older app's any more: its notice is over, and it ages out as closed histories do.
            None if name.ends_with(&format!(".{EARLIER}")) => close_kept(folder, name, key),
            None => {}
        }
    }
    for (at, newer) in found {
        if let Some(back) = take_one(folder, at, newer, record, journals, progress, read_total) {
            unclaimed.push(back);
        }
    }
}

/// One tab's newer list, with its older app's list claimed: merged, kept and said, or (not linked to it) given back as it was found.
fn take_one(
    folder: &Path,
    at: usize,
    newer: Found,
    record: &mut SessionRecord,
    journals: &mut HashMap<String, RestoredEdits>,
    progress: &mut Progress,
    read_total: &mut u64,
) -> Option<JournalRead> {
    let tab = &record.tabs[at];
    let older_name = tab.journal.clone()?;
    let (newer_name, n) = match &newer {
        Found::Own(read) => (read.name.clone(), &read.parsed),
        Found::Aside(name, parsed) => (name.clone(), parsed),
    };
    let give_back = |newer: Found| match newer {
        Found::Own(read) => Some(read),
        Found::Aside(..) => None,
    };
    let older_path = folder.join(&older_name);
    *read_total += std::fs::metadata(&older_path).map_or(0, |meta| meta.len());
    if *read_total > super::MAX_JOURNAL_BYTES_AT_LAUNCH {
        return give_back(newer);
    }
    let Some((o, _)) = read_journal(&older_path).ok().flatten() else {
        return give_back(newer);
    };
    // Both this tab's (its id and path, as `followed` links them), about the one version of its file.
    let linked = [&n.header, &o.header].iter().all(|header| header.tab == tab.id && header.path == tab.path)
        && n.header.file.is_some()
        && n.header.file == o.header.file
        && n.header.file_pages == o.header.file_pages
        && o.header.version < record::MARKED_JOURNAL_VERSION;
    if !linked {
        return give_back(newer);
    }
    let now = tab.path.to_path().and_then(|path| file_stamp(&path).ok());
    let is_now = n
        .header
        .file
        .map(recto_platform::FileStamp::from)
        .zip(now)
        .is_some_and(|(file, now)| file.same_contents(now) && file.inode == now.inode);
    let Some(key) = split(&newer_name).map(|(key, _)| key.to_string()) else {
        return give_back(newer);
    };
    let own_name = format!("journal-{key}.log");
    if let Some(append) = merge_plan(n, &o, is_now)
        && merged(folder, &newer_name, &own_name, n.good_bytes, &append)
        && let Ok(Some((parsed, _))) = read_journal(&folder.join(&own_name))
    {
        // O's every step is in N now: kept as a closed history, aged out with them.
        journals.remove(&older_name);
        progress.journal_bytes.remove(&older_name);
        page_ids::remove(folder, &older_name, progress);
        page_ids::remove(folder, &own_name, progress);
        if let Some(closed) = free_closed_name(folder, &older_name, &key) {
            let _ = close_journal(&older_path, &folder.join(closed));
        }
        let read = JournalRead {
            name: own_name.clone(),
            path: folder.join(&own_name),
            parsed,
        };
        claim(folder, read, record, at, progress, journals);
        return None;
    }
    // Case 2: the tab keeps O, and N is kept beside it and said. From where it is now: under its own name, should a merge have brought it back there
    // and not got through.
    let at_now = if std::fs::symlink_metadata(folder.join(&newer_name)).is_ok() {
        &newer_name
    } else {
        &own_name
    };
    let kept = if at_now.ends_with(&format!(".{EARLIER}")) {
        Some(at_now.clone())
    } else {
        free_earlier_name(folder, &key).filter(|kept| std::fs::rename(folder.join(at_now), folder.join(kept)).is_ok())
    };
    if kept.is_none() {
        return give_back(newer);
    }
    if *at_now == own_name {
        progress.journal_bytes.remove(&own_name);
        page_ids::remove(folder, &own_name, progress);
    }
    if let Some(list) = journals.get_mut(&older_name) {
        list.set_aside_earlier = true;
    }
    None
}

/// **Whether two lists combine (case 1), and what of O's is still to be appended to N.** N: of the newer version, whole, and the file's own pages
/// in place and unturned, nothing added (so it differs from the file by highlights alone). O: below it, whole, started from the file (no base, its
/// steps counted from 0), nothing but page steps (no save begun, no highlight). The file as it is now is the version both are about. And the two
/// together replay whole, within the replay's budget.
///
/// **A merge a crash cut short:** O's first steps may already be at N's end (appended, a last line torn, or all of them before O went). They're
/// found there, with N before them still the file's own pages, and only the rest is appended. A torn last line isn't read (`parse_journal`), and
/// the append cuts it off before it writes, so a half-written step is written again, whole. Should N's own last steps be the same lines as O's first
/// and come to nothing on the pages, they'd be taken for O's: the pages come out as they should, and only those steps' history is short.
pub(super) fn merge_plan(n: &ParsedJournal, o: &ParsedJournal, file_is_now: bool) -> Option<Vec<JournalOp>> {
    let o_plain = o.base.is_none()
        && o.header.base_seq == 0
        && o.header.saving.is_none()
        && o.header.keeping.is_none()
        && !o
            .ops
            .iter()
            .any(|op| matches!(op, JournalOp::Saving(_) | JournalOp::Keeping(_) | JournalOp::Mark(_)));
    if !file_is_now
        || n.header.version < record::MARKED_JOURNAL_VERSION
        || o.header.version >= record::MARKED_JOURNAL_VERSION
        || n.lost_lines
        || o.lost_lines
        || !o_plain
    {
        return None;
    }
    let pages = n.header.file_pages;
    let at_end = |k: usize| n.ops.len() >= k && n.ops[n.ops.len() - k..] == o.ops[..k];
    let mut tries = vec![o.ops.len()];
    tries.extend((1..o.ops.len()).rev().filter(|k| n.ops.last() == o.ops.get(k - 1)).take(MAX_PARTIAL_TRIES));
    tries.push(0);
    for k in tries {
        if !at_end(k) {
            continue;
        }
        let before = &n.ops[..n.ops.len() - k];
        let Ok((list, applied)) = PageEdits::replay(pages, n.base.as_ref(), before) else {
            continue;
        };
        if applied != before.len() || !the_files_own_pages(&list) {
            continue;
        }
        let all: Vec<JournalOp> = before.iter().chain(&o.ops).cloned().collect();
        let (_, applied) = PageEdits::replay(pages, n.base.as_ref(), &all).ok()?;
        return (applied == all.len()).then(|| o.ops[k..].to_vec());
    }
    None
}

/// Every page of the file in its own place, unturned, and nothing added: what's on the pages is the file's, whatever highlights are on them.
fn the_files_own_pages(list: &PageEdits) -> bool {
    list.added_sources().is_empty()
        && list.slots().len() == list.file_pages() as usize
        && list
            .slots()
            .iter()
            .enumerate()
            .all(|(at, slot)| slot.source == 0 && slot.quarter_turns == 0 && slot.page as usize == at)
}

/// **N under its own name with O's steps appended,** its good bytes kept as they are: brought back from where it was set aside if it was, then
/// what's past its good bytes (a torn line) cut and the lines written after, as the journal's own writer appends. Whether it's all there.
fn merged(folder: &Path, newer_name: &str, own_name: &str, good: u64, append: &[JournalOp]) -> bool {
    if newer_name != own_name {
        // Set aside: back under its own name, which nothing else may have.
        if std::fs::symlink_metadata(folder.join(own_name)).is_ok() || std::fs::rename(folder.join(newer_name), folder.join(own_name)).is_err() {
            return false;
        }
    }
    if append.is_empty() {
        return true;
    }
    let lines = record::journal_lines(append);
    (|| -> std::io::Result<()> {
        let mut file = local::open_private_for_writing(&folder.join(own_name), false)?;
        file.set_len(good)?;
        file.seek(SeekFrom::Start(good))?;
        file.write_all(&lines)?;
        sync(&file, false)
    })()
    .is_ok()
}

/// A name for N kept beside its tab's list: `journal-<key>.earlier`, or with a number if something's there.
fn free_earlier_name(folder: &Path, key: &str) -> Option<String> {
    std::iter::once(format!("journal-{key}.{EARLIER}"))
        .chain((1..64u32).map(|at| format!("journal-{key}-{at}.{EARLIER}")))
        .find(|name| std::fs::symlink_metadata(folder.join(name)).is_err())
}

/// A closed history's name for a list of this key: its own closed name if it's free, or one with a number. Never `closed-<key>.log`, which opening the
/// file would take up as its history: what's kept here is only aged out.
fn free_closed_name(folder: &Path, name: &str, key: &str) -> Option<String> {
    closed_journal_name(name)
        .filter(|closed| closed != &format!("closed-{key}.log"))
        .into_iter()
        .chain((1..64u32).map(|at| format!("closed-{key}-{at}.log")))
        .find(|name| std::fs::symlink_metadata(folder.join(name)).is_err())
}

/// A list kept for a tab whose list isn't an older app's any more: under a closed history's name, its 30 days from now.
fn close_kept(folder: &Path, name: &str, key: &str) {
    if let Some(closed) = free_closed_name(folder, &format!("journal-{key}.log"), key) {
        let _ = close_journal(&folder.join(name), &folder.join(closed));
    }
}

#[cfg(test)]
mod tests;
