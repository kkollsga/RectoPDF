//! **A copy that went back to an older app and came forward again** (`docs/RELEASING.md`, a bad release; `tools/release/check-carry.sh`, runs C to
//! F). An app from before highlights (0.1.0) leaves a journal of a newer version (one with highlights, [`record::MARKED_JOURNAL_VERSION`]) alone,
//! byte for byte, and writes the record with that tab naming no journal and saying `edits_unreadable`, which was so for it. An edit made there then
//! starts a list of its own from the file as it is, under another name beside the newer one (`journal-<key>-<n>.log`, the newer one being
//! `journal-<key>.log`). Coming forward, both are this tab's: the older app's, O, is the tab's list and the newer app's, N, beside it (under its own
//! name, or as 0.1.1 sets it aside, `journal-<key>.not-merged`); or, where 0.1.1 itself ran on that and took N for the newer by its steps, N is the
//! tab's list and O is set aside beside it (`journal-<key>-<n>.not-merged`). Either way the same test decides.
//!
//! - **D, no edit there:** the journal is claimed by its file and read whole, so the older app's notice isn't so ([`unreadable_answered`]).
//! - **Two lists, which combine** (case 1, [`merge_plan`]): N is the file's own pages, in place and unturned, with only highlights on them, all its
//!   steps from the file itself (no base); O started from the file with nothing but page steps. Then O's steps follow N's, in the order they were
//!   made: a highlight belongs to its page by what the page is, so a turn, a move or a removal made in the older app takes it along. The person gets
//!   every edit and no notice. On disk, in this order: N back under its `.log` name if it was set aside, and the folder flushed; O's lines appended to
//!   N, its own bytes its prefix, and N flushed to the drive itself; then O to a closed history's name, byte for byte, and the folder flushed. So a
//!   power cut can't leave O gone with its steps not yet in N. O ages out with closed histories (30 days), its every step in N.
//! - **A merge cut short** (a crash or a power cut between those steps, or O's move refused): the next launch finds what it reached by comparing
//!   operations for equality: O's first steps at N's end, at most [`MAX_PARTIAL_TRIES`] places tried besides all and none, with what's before them in
//!   N being the file's own pages as the test is made on. It appends only the rest, and moves O again. A cut it doesn't recognise falls to case 2;
//!   and an N whose own last steps were the same lines as O's first, and came to nothing on the pages, would be taken for merged that far (the pages
//!   come out right, and only those steps' history is short). No marker is written.
//! - **Two lists, which don't** (case 2), anything not provably case 1: the tab keeps O, what the person last worked on, and N is kept as
//!   `journal-<key>.earlier`, byte for byte. Each launch says so again ([`FileNotice::EditsSetAsideWentBack`], never in a record, so no older app
//!   misreads one, and it survives another trip back), decided from that file's first line alone, its tab's id, while the tab's list is that O by its
//!   name: nothing more is read or replayed for it. Once it isn't (saved, discarded, the tab closed), the kept list goes to a closed history's name,
//!   and the 30 days closed histories have start then. Nothing else prunes it.
//!
//! The links are those `followed` uses (the tab's id and path in both headers) and both lists about the one version of the file, found by the names
//! the two apps give them: nothing is looked at for a tab whose list isn't one of the two, and nothing at all with no candidate in the folder. Reads
//! count towards the launch's total, and replays go by the replay's own budget. **No link is followed:** what's read is opened without following
//! one, and only a plain file is moved.

use super::{JournalRead, Progress, claim, close_journal, local, page_ids, read_journal, sync};
use recto_platform::file_stamp;
use recto_session::RestoredEdits;
use recto_session::edit::PageEdits;
use recto_session::record::{self, FileNotice, JournalHeader, JournalOp, ParsedJournal, SessionRecord, TabEntry, closed_journal_name};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The name an earlier list is kept under, beside its tab's (case 2).
const EARLIER: &str = "earlier";
/// The name the store sets a list aside under, which 0.1.1 used for either list (`followed`).
const NOT_MERGED: &str = "not-merged";
/// Most names of lists set aside a launch looks at. Past it, the rest wait for a later launch: a hostile folder costs a bounded look.
pub(super) const MAX_SET_ASIDE_NAMES: usize = 1024;
/// Most places a merge cut short is looked for at N's end besides all and none: each is a comparison and a replay, within the replay's budget.
const MAX_PARTIAL_TRIES: usize = 8;
/// Most of a kept list's start read for its first line.
const HEADER_BYTES: u64 = 64 * 1024;

/// The limit on set-aside names a launch looks at: [`MAX_SET_ASIDE_NAMES`], and in a test build what a test sets, so the limit plus one is tested
/// with the limit small.
#[cfg(not(test))]
fn set_aside_limit() -> usize {
    MAX_SET_ASIDE_NAMES
}

#[cfg(test)]
pub(super) static SET_ASIDE_LIMIT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(MAX_SET_ASIDE_NAMES);

#[cfg(test)]
fn set_aside_limit() -> usize {
    SET_ASIDE_LIMIT.load(std::sync::atomic::Ordering::Relaxed)
}

// What a merge did to the disk, in order, for a test to hold the order by (a power cut can't be made in a test).
#[cfg(test)]
thread_local! {
    pub(super) static STEPS: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn step(what: &str) {
    #[cfg(test)]
    STEPS.with(|steps| steps.borrow_mut().push(what.to_string()));
    let _ = what;
}

/// **A record's "couldn't be read" against the journal as it reads now** (`fault:D.notice.edits_unreadable`, found by `check-carry.sh` on
/// 2026-11-15). The tab named no journal, said its edits couldn't be read, and has now claimed one that read whole, of a version an app reading only
/// [`record::CURRENT_VERSION`]'s journals leaves alone: that app's words about this journal, and not so here. The caller says it again when what's
/// claimed has lines that couldn't be used.
///
/// Kept, as this app's own and still so: a record naming the journal it says this of (one that had lines it couldn't use, written clean by `claim`,
/// said until the person has seen it); one claiming a journal of the version every app reads (a list started after a journal of this tab's couldn't
/// be read, its record not yet written); and (the review, item 5) one with what couldn't be read of a journal of this file kept beside it
/// (`journal-<key>….unreadable` or `.kept`), which a first edit's highlight can make a journal of the newer version before the record names it.
pub(super) fn unreadable_answered(folder: &Path, tab: &mut TabEntry, name: &str, header: &JournalHeader) {
    let named_it = tab.journal.as_deref() == Some(name);
    if named_it || header.version <= record::CURRENT_VERSION || !matches!(tab.notice, Some(FileNotice::EditsUnreadable)) {
        return;
    }
    let Some((key, _)) = split(name) else { return };
    let kept_beside = std::fs::read_dir(folder).into_iter().flatten().flatten().any(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|other| split(other).is_some_and(|(of, _)| of == key) && (other.ends_with(".unreadable") || other.ends_with(".kept")))
    });
    if !kept_beside {
        tab.notice = None;
    }
}

/// The key of a name, `journal-<16 hex>`, with what follows it.
fn split(name: &str) -> Option<(&str, &str)> {
    let rest = name.strip_prefix("journal-")?;
    let key = rest.get(..16)?;
    key.bytes().all(|byte| byte.is_ascii_hexdigit()).then(|| (key, &rest[16..]))
}

/// `-<n>`, a number of up to ten digits.
fn is_number(tail: &str) -> bool {
    tail.strip_prefix('-')
        .is_some_and(|n| !n.is_empty() && n.len() <= 10 && n.bytes().all(|byte| byte.is_ascii_digit()))
}

/// An older app's list's name, `journal-<key>-<n>.log`: its key.
fn older_key(name: &str) -> Option<&str> {
    let (key, tail) = split(name)?;
    is_number(tail.strip_suffix(".log")?).then_some(key)
}

/// A newer app's list's own name, `journal-<key>.log`: its key.
fn newer_key(name: &str) -> Option<&str> {
    split(name).filter(|(_, tail)| *tail == ".log").map(|(key, _)| key)
}

/// What a set-aside name may be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Aside {
    /// N set aside by 0.1.1, `journal-<key>.not-merged`.
    NewerNotMerged,
    /// N kept here (case 2), `journal-<key>[-<n>].earlier`.
    Earlier,
    /// O set aside by 0.1.1, `journal-<key>-<n>.not-merged`.
    OlderNotMerged,
}

/// **A list set aside that may be one of a tab's two:** its key, and which it may be.
pub(super) fn set_aside_key(name: &str) -> Option<(&str, Aside)> {
    let (key, tail) = split(name)?;
    let not_merged = format!(".{NOT_MERGED}");
    if tail == not_merged {
        return Some((key, Aside::NewerNotMerged));
    }
    if tail.strip_suffix(&not_merged).is_some_and(is_number) {
        return Some((key, Aside::OlderNotMerged));
    }
    let number = tail.strip_suffix(&format!(".{EARLIER}"))?;
    (number.is_empty() || is_number(number)).then_some((key, Aside::Earlier))
}

/// The journals in the journals' folder, in order, and from the same look the lists set aside that may be one of a tab's two: a bounded number of
/// names, nothing read.
pub(super) fn names_in(folder: &Path) -> (Vec<String>, Vec<String>) {
    let (mut names, mut set_aside) = (Vec::new(), Vec::new());
    let limit = set_aside_limit();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if record::is_journal_name(&name) {
            names.push(name);
        } else if set_aside.len() < limit && set_aside_key(&name).is_some() {
            set_aside.push(name);
        }
    }
    names.sort_unstable();
    set_aside.sort_unstable();
    (names, set_aside)
}

/// A list found beside a tab's, as it was found.
enum Found {
    /// Under a journal's name, claimed by no tab.
    Own(JournalRead),
    /// Set aside, under this name.
    Aside(String, ParsedJournal),
}

impl Found {
    fn name(&self) -> &str {
        match self {
            Self::Own(read) => &read.name,
            Self::Aside(name, _) => name,
        }
    }

    fn parsed(&self) -> &ParsedJournal {
        match self {
            Self::Own(read) => &read.parsed,
            Self::Aside(_, parsed) => parsed,
        }
    }

    /// Not this tab's after all: one under a journal's name goes back to the store's other rules, one set aside stays as it is.
    fn give_back(self) -> Option<JournalRead> {
        match self {
            Self::Own(read) => Some(read),
            Self::Aside(..) => None,
        }
    }
}

/// Which of the two the tab's list is, and so what the list found beside it is.
enum Beside {
    /// The tab's list is O; this may be N.
    Newer(Found),
    /// The tab's list is N; this may be O.
    Older(Found),
}

/// What the launch carries through: the folder, and the launch's read total.
pub(super) struct Launch<'a> {
    pub folder: &'a Path,
    pub progress: &'a mut Progress,
    pub read_total: &'a mut u64,
}

impl Launch<'_> {
    /// Counts a file's size towards the launch's total, and says whether it's still within it.
    fn within(&mut self, name: &str) -> bool {
        *self.read_total += std::fs::symlink_metadata(self.folder.join(name)).map_or(0, |meta| meta.len());
        *self.read_total <= super::MAX_JOURNAL_BYTES_AT_LAUNCH
    }
}

/// **The two lists of tabs after a copy went back,** after every tab has claimed the journal it names: merged (case 1) or kept beside it and said
/// (case 2), each list found under a journal's name taken out of `unclaimed` unless it isn't the tab's after all. And a kept list no tab's list is
/// O for any more goes to a closed history's name. See the module's notes.
pub(super) fn take_earlier(
    launch: &mut Launch<'_>,
    set_aside: &[String],
    record: &mut SessionRecord,
    taken: &[bool],
    journals: &mut HashMap<String, RestoredEdits>,
    unclaimed: &mut Vec<JournalRead>,
) {
    let of_two = |read: &JournalRead| {
        let marked = read.parsed.header.version >= record::MARKED_JOURNAL_VERSION;
        (marked && newer_key(&read.name).is_some()) || (!marked && older_key(&read.name).is_some())
    };
    if set_aside.is_empty() && !unclaimed.iter().any(of_two) {
        return;
    }
    // Tabs whose list is one of the two, by its key. Only when there's something to find.
    let (mut older_tabs, mut newer_tabs) = (HashMap::new(), HashMap::new());
    for (at, tab) in record.tabs.iter().enumerate() {
        let Some(name) = tab
            .journal
            .as_deref()
            .filter(|name| taken.get(at).copied().unwrap_or(false) && journals.contains_key(*name))
        else {
            continue;
        };
        if let Some(key) = older_key(name) {
            older_tabs.insert(key.to_string(), at);
        } else if let Some(key) = newer_key(name) {
            newer_tabs.insert(key.to_string(), at);
        }
    }
    let mut done: HashSet<usize> = HashSet::new();
    let mut found: Vec<(usize, Beside)> = Vec::new();
    let mut at_read = 0;
    while at_read < unclaimed.len() {
        let read = &unclaimed[at_read];
        let marked = read.parsed.header.version >= record::MARKED_JOURNAL_VERSION;
        let newer = newer_key(&read.name).filter(|_| marked).and_then(|key| older_tabs.get(key).copied());
        let older = older_key(&read.name).filter(|_| !marked).and_then(|key| newer_tabs.get(key).copied());
        match newer.or(older).filter(|at| !done.contains(at)) {
            Some(at) => {
                done.insert(at);
                let read = Found::Own(unclaimed.remove(at_read));
                found.push((at, if newer.is_some() { Beside::Newer(read) } else { Beside::Older(read) }));
            }
            None => at_read += 1,
        }
    }
    for name in set_aside {
        let Some((key, kind)) = set_aside_key(name) else { continue };
        let tabs = if kind == Aside::OlderNotMerged { &newer_tabs } else { &older_tabs };
        let Some(at) = tabs.get(key).copied() else {
            if kind == Aside::Earlier {
                // Kept for a tab whose list isn't O any more: its notice is over, and it ages out as closed histories do.
                close_kept(launch.folder, name, key);
            }
            continue;
        };
        if done.contains(&at) {
            continue;
        }
        if kind == Aside::Earlier {
            // Decided from its first line: its tab's id. Nothing more is read, and nothing replayed.
            done.insert(at);
            match read_header(&launch.folder.join(name)) {
                Some(header) if header.tab == record.tabs[at].id => mark_earlier(journals, &record.tabs[at]),
                _ => close_kept(launch.folder, name, key),
            }
            continue;
        }
        if !launch.within(name) {
            continue;
        }
        // Not a journal this build reads (not one, damaged, or a newer app's): not this tab's, and left as it is.
        if let Ok(Some((parsed, _))) = read_journal(&launch.folder.join(name)) {
            done.insert(at);
            let aside = Found::Aside(name.clone(), parsed);
            found.push((
                at,
                if kind == Aside::OlderNotMerged {
                    Beside::Older(aside)
                } else {
                    Beside::Newer(aside)
                },
            ));
        }
    }
    for (at, beside) in found {
        if let Some(back) = take_one(launch, at, beside, record, journals) {
            unclaimed.push(back);
        }
    }
}

/// The first line of a kept list, read without following a link and no further than [`HEADER_BYTES`].
fn read_header(path: &Path) -> Option<JournalHeader> {
    let mut bytes = Vec::new();
    local::open_private_file(path).ok()?.take(HEADER_BYTES).read_to_end(&mut bytes).ok()?;
    record::journal_header(&bytes)
}

/// The tab's list is told an earlier list of its own is kept beside it.
fn mark_earlier(journals: &mut HashMap<String, RestoredEdits>, tab: &TabEntry) {
    if let Some(list) = tab.journal.as_deref().and_then(|name| journals.get_mut(name)) {
        list.set_aside_earlier = true;
    }
}

/// One tab's two lists: merged, kept and said, or (not linked to it) the one found given back as it was found.
fn take_one(
    launch: &mut Launch<'_>,
    at: usize,
    beside: Beside,
    record: &mut SessionRecord,
    journals: &mut HashMap<String, RestoredEdits>,
) -> Option<JournalRead> {
    let folder = launch.folder;
    let tabs_name = record.tabs[at].journal.clone()?;
    let (found, tab_has_older) = match beside {
        Beside::Newer(found) => (found, true),
        Beside::Older(found) => (found, false),
    };
    if !launch.within(&tabs_name) {
        return found.give_back();
    }
    let Some((tabs_list, _)) = read_journal(&folder.join(&tabs_name)).ok().flatten() else {
        return found.give_back();
    };
    let (n, o) = if tab_has_older {
        (found.parsed(), &tabs_list)
    } else {
        (&tabs_list, found.parsed())
    };
    let tab = &record.tabs[at];
    // Both this tab's (its id and path, as `followed` links them), about the one version of its file, N of the newer version and O of the older.
    let linked = [&n.header, &o.header].iter().all(|header| header.tab == tab.id && header.path == tab.path)
        && n.header.file.is_some()
        && n.header.file == o.header.file
        && n.header.file_pages == o.header.file_pages
        && n.header.version >= record::MARKED_JOURNAL_VERSION
        && o.header.version < record::MARKED_JOURNAL_VERSION;
    let Some(key) = split(&tabs_name).map(|(key, _)| key.to_string()).filter(|_| linked) else {
        return found.give_back();
    };
    let now = tab.path.to_path().and_then(|path| file_stamp(&path).ok());
    let is_now = n
        .header
        .file
        .map(recto_platform::FileStamp::from)
        .zip(now)
        .is_some_and(|(file, now)| file.same_contents(now) && file.inode == now.inode);
    let own_name = format!("journal-{key}.log");
    let (n_name, o_name) = if tab_has_older {
        (found.name().to_string(), tabs_name.clone())
    } else {
        (tabs_name.clone(), found.name().to_string())
    };
    // Case 1: O's steps after N's, under N's own name; O then to a closed history's name.
    if let Some(append) = merge_plan(n, o, is_now)
        && n_under_own_name(folder, &n_name, &own_name)
        && appended(folder, &own_name, n.good_bytes, &append)
        && let Ok(Some((parsed, _))) = read_journal(&folder.join(&own_name))
    {
        for name in [&tabs_name, &own_name] {
            journals.remove(name.as_str());
            launch.progress.journal_bytes.remove(name.as_str());
            page_ids::remove(folder, name, launch.progress);
        }
        let read = JournalRead {
            name: own_name.clone(),
            path: folder.join(&own_name),
            parsed,
        };
        claim(folder, read, record, at, launch.progress, journals);
        // Refused (not a crash): O stays under its name, and the next launch finds its every step at N's end and moves it then.
        close_older(folder, &o_name, &key);
        return None;
    }
    // Case 2: the tab's list is O, and N is kept beside it and said.
    if tab_has_older {
        let at_now = if std::fs::symlink_metadata(folder.join(&n_name)).is_ok() {
            n_name.clone()
        } else {
            own_name
        };
        if keep_earlier(folder, &at_now, &key).is_none() {
            return found.give_back();
        }
        if newer_key(&at_now).is_some() {
            launch.progress.journal_bytes.remove(&at_now);
            page_ids::remove(folder, &at_now, launch.progress);
        }
        mark_earlier(journals, &record.tabs[at]);
        return None;
    }
    // The tab's list was N (0.1.1 took it for the newer by its steps): O, its list again, and N kept beside it.
    let o_log = match found.name().strip_suffix(&format!(".{NOT_MERGED}")) {
        Some(stem) => format!("{stem}.log"),
        None => found.name().to_string(),
    };
    if o_log != found.name() && !move_plain(folder, found.name(), &o_log) {
        return None;
    }
    if keep_earlier(folder, &tabs_name, &key).is_none() {
        if o_log != found.name() {
            let _ = move_plain(folder, &o_log, found.name());
        }
        return found.give_back();
    }
    journals.remove(&tabs_name);
    launch.progress.journal_bytes.remove(&tabs_name);
    page_ids::remove(folder, &tabs_name, launch.progress);
    let Ok(Some((parsed, _))) = read_journal(&folder.join(&o_log)) else {
        return None;
    };
    let read = JournalRead {
        name: o_log.clone(),
        path: folder.join(&o_log),
        parsed,
    };
    claim(folder, read, record, at, launch.progress, journals);
    mark_earlier(journals, &record.tabs[at]);
    None
}

/// **Whether two lists combine (case 1), and what of O's is still to be appended to N.** N: of the newer version, whole, every step its own from the
/// file (no base, counted from 0), and the file's own pages in place and unturned, nothing added (so it differs from the file by highlights alone). O:
/// below it, whole, started from the file the same way, nothing but page steps (no save begun, no highlight). The file as it is now is the version
/// both are about. And the two together replay whole, within the replay's budget.
///
/// **A merge cut short:** O's first steps may already be at N's end. They're found by comparing operations for equality, at all of O, at most
/// [`MAX_PARTIAL_TRIES`] shorter cuts, and none, with what's before them in N being the file's own pages: the test is made on N without the part
/// appended. Only the rest is appended. A torn last line isn't read (`parse_journal`), and the append cuts it off before it writes, so a half-written
/// step is written again, whole. See the module's notes for what this can't tell apart.
pub(super) fn merge_plan(n: &ParsedJournal, o: &ParsedJournal, file_is_now: bool) -> Option<Vec<JournalOp>> {
    let from_the_file = |parsed: &ParsedJournal| {
        parsed.base.is_none() && parsed.header.base_seq == 0 && parsed.header.saving.is_none() && parsed.header.keeping.is_none() && !parsed.lost_lines
    };
    let o_page_steps = !o
        .ops
        .iter()
        .any(|op| matches!(op, JournalOp::Saving(_) | JournalOp::Keeping(_) | JournalOp::Mark(_)));
    if !file_is_now
        || n.header.version < record::MARKED_JOURNAL_VERSION
        || o.header.version >= record::MARKED_JOURNAL_VERSION
        || !from_the_file(n)
        || !from_the_file(o)
        || !o_page_steps
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
        let Ok((list, applied)) = PageEdits::replay(pages, None, before) else {
            continue;
        };
        if applied != before.len() || !the_files_own_pages(&list) {
            continue;
        }
        let all: Vec<JournalOp> = before.iter().chain(&o.ops).cloned().collect();
        let (_, applied) = PageEdits::replay(pages, None, &all).ok()?;
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

/// A plain file of the folder (never a link or a folder) under another name there, and the folder flushed to the drive: what a merge's moves are.
fn move_plain(folder: &Path, from: &str, to: &str) -> bool {
    let plain = std::fs::symlink_metadata(folder.join(from)).is_ok_and(|meta| meta.file_type().is_file());
    if !plain || std::fs::symlink_metadata(folder.join(to)).is_ok() || std::fs::rename(folder.join(from), folder.join(to)).is_err() {
        return false;
    }
    let _ = local::sync_folder_of(&folder.join(to));
    step(&format!("moved {from} to {to}, the folder flushed"));
    true
}

/// N under its own name: there already, or brought back from where it was set aside.
fn n_under_own_name(folder: &Path, n_name: &str, own_name: &str) -> bool {
    n_name == own_name || move_plain(folder, n_name, own_name)
}

/// **O's steps appended to N,** its good bytes kept as they are: what's past them (a torn line) cut and the lines written after, as the journal's own
/// writer appends, opened without following a link, and then **flushed by the drive itself** (`F_FULLFSYNC`): O is moved away next, and that mustn't
/// reach the disk before this does. The writer's own appends are only handed to the drive (a plain `fsync`), with the drive made to flush at a
/// hand-off (`RecordStore::force_to_drive`): nothing of theirs is removed on the strength of them. Whether it's all there.
fn appended(folder: &Path, own_name: &str, good: u64, append: &[JournalOp]) -> bool {
    if append.is_empty() {
        return true;
    }
    let lines = record::journal_lines(append);
    let by_the_drive = true;
    let written = (|| -> std::io::Result<()> {
        let mut file = local::open_private_for_writing(&folder.join(own_name), false)?;
        file.set_len(good)?;
        file.seek(SeekFrom::Start(good))?;
        file.write_all(&lines)?;
        sync(&file, by_the_drive)
    })()
    .is_ok();
    if written {
        step(&format!("appended to {own_name}, flushed by the drive: {by_the_drive}"));
    }
    written
}

/// N kept beside its tab's list as `journal-<key>.earlier` (or with a number, if that's taken). Its name, or `None` if it isn't a plain file or
/// can't be moved.
fn keep_earlier(folder: &Path, name: &str, key: &str) -> Option<String> {
    if name.ends_with(&format!(".{EARLIER}")) {
        return Some(name.to_string());
    }
    let kept = std::iter::once(format!("journal-{key}.{EARLIER}"))
        .chain((1..64u32).map(|at| format!("journal-{key}-{at}.{EARLIER}")))
        .find(|kept| std::fs::symlink_metadata(folder.join(kept)).is_err())?;
    move_plain(folder, name, &kept).then_some(kept)
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

/// O, its every step in N, under a closed history's name, its 30 days from now, and the folder flushed. A plain file only.
fn close_older(folder: &Path, name: &str, key: &str) {
    let plain = std::fs::symlink_metadata(folder.join(name)).is_ok_and(|meta| meta.file_type().is_file());
    if let Some(closed) = free_closed_name(folder, name, key).filter(|_| plain)
        && close_journal(&folder.join(name), &folder.join(&closed)).is_ok()
    {
        let _ = local::sync_folder_of(&folder.join(&closed));
        step(&format!("moved {name} to {closed}, the folder flushed"));
    }
}

/// A list kept for a tab whose list isn't O any more: under a closed history's name, its 30 days from now. A plain file only: a link or a folder
/// left at the name is left alone.
fn close_kept(folder: &Path, name: &str, key: &str) {
    let plain = std::fs::symlink_metadata(folder.join(name)).is_ok_and(|meta| meta.file_type().is_file());
    if let Some(closed) = free_closed_name(folder, &format!("journal-{key}.log"), key).filter(|_| plain) {
        let _ = close_journal(&folder.join(name), &folder.join(closed));
    }
}

#[cfg(test)]
mod tests;
