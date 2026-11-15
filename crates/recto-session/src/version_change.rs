//! **Unsaved edits following their pages into another version of their file** (the page matcher, slice 2; owner, 2026-11-13: "we need to run a page
//! matcher of some kind. That's the way we can ensure edits on the correct page"; the rule: they stay on their own page when the file changes, or are
//! held and said, never put on another page).
//!
//! When a read of a tab finds its file is another version than its edits were made on, and that version is known (`compare_read_with_edits`), the
//! list may follow (`edit::identity::may_follow`). The app's page-identity thread then runs the three steps here, as it runs
//! [`crate::identify`]'s: [`work`] says which tabs and what's known of their pages, under the session's lock and briefly; [`read`] opens the new version
//! through the file wall and matches the pages ([`crate::edit::matcher`]), off the lock; [`followed`] takes the match in under the lock, if nothing
//! changed meanwhile, and either puts the list onto the new version ([`crate::edit::rebase`]) and says so, or holds it exactly as before (the tab says
//! its file changed under its edits, as it did before the matcher: `fit.rs`), all or nothing for 0.1.1.
//!
//! **Never for a protected document**: a tab opened with a password, or needing one, has no named pages read ([`crate::identify`]), and is left out
//! here before anything is computed, so its edits are held as before and an agent learns nothing new of it. Nor for a file only the renderer reads.
//! Nothing here keeps a thread, a handle or a buffer; a job holds a copy of what's named (at most [`crate::edit::identity::MAX_IDENTIFIED`] pages).

use crate::edit::identity::{self, Known};
use crate::edit::marks::FileKey;
use crate::edit::matcher::{ANNOT_LOOKUPS, Budget, Found, Matcher, WorkCaps};
use crate::edit::{PageEdits, rebase};
use crate::fit::EditsFit;
use crate::record::FileNotice;
use crate::{DocLoader, Session, TabId};
use recto_platform::FileStamp;
use std::path::PathBuf;

/// The one budget of a follow's reading, which a test sets small on its own thread to hold it at its limit plus one with a small file.
fn work_caps() -> WorkCaps {
    #[cfg(test)]
    if let Some(caps) = crate::tests::version_change::WORK_CAPS.with(std::cell::Cell::get) {
        return caps;
    }
    WorkCaps::STANDARD
}

/// One tab whose edits may follow their pages into the version of its file there now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowJob {
    pub tab: TabId,
    pub path: PathBuf,
    /// The version found, which the job reads only while it's still there.
    pub to: FileStamp,
    /// What the edits' pages were, as named.
    named: Vec<(u32, Option<Known>)>,
    /// The file's own highlights the edits take out, to be found again.
    hidden: Vec<FileKey>,
    /// The list's changes when the job was made: one made since names pages the job doesn't know of.
    changes: u64,
    /// The edits' version's `/ID` pair, as read with its pages.
    document: Option<recto_core::page_id::DocumentIds>,
}

/// What a job found.
#[derive(Clone, Debug, PartialEq)]
pub struct Matched {
    pub tab: TabId,
    pub to: FileStamp,
    changes: u64,
    found: Vec<(u32, Found)>,
    pages: u32,
    refound: Vec<(FileKey, Option<FileKey>)>,
    /// The new version's `/ID` pair, if it has one.
    document: Option<recto_core::page_id::DocumentIds>,
}

/// What became of a tab whose edits followed their pages: its length now and how many edits followed, for the event and `edits`. Counts only, and no
/// digest, ever.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Followed {
    pub tab: TabId,
    pub pages: u32,
    pub edits: u32,
}

/// **Whether a list in memory may follow its pages into a version not yet tried:** a look at each list's own field, with no tab looked up, so the
/// session thread can ask after every call.
pub fn wanted<L: DocLoader>(session: &Session<L>) -> bool {
    session.lists.iter().any(|(_, edits)| identity::follow_wanted(edits).is_some())
}

/// Whether a tab's edits may follow their pages into `to` now: it isn't protected or only the renderer's, the file is known to have changed to `to`
/// under its edits, from a version that's known, and no save of it is under way.
fn may_follow<L: DocLoader>(session: &Session<L>, id: TabId, to: FileStamp) -> Option<PathBuf> {
    let tab = session.tabs.tab(id)?;
    if crate::registry::withheld_tab(tab) || tab.renderer_only || session.saves.is_saving(id) {
        return None;
    }
    if session.notices().fit_of(tab) != EditsFit::FileChanged || tab.file.elsewhere().and_then(|elsewhere| elsewhere.seen) != Some(to) {
        return None;
    }
    session.tabs.path_of(id)
}

/// **The tabs whose edits may follow their pages now,** and what's known of those pages. A list that can't (protected, past its cap, or no longer
/// changed to that version) is marked tried, and held as it is: nothing is read for it.
pub fn work<L: DocLoader>(session: &mut Session<L>) -> Vec<FollowJob> {
    let wanting: Vec<(TabId, FileStamp)> = session
        .lists
        .iter()
        .filter_map(|(id, edits)| Some((*id, identity::follow_wanted(edits)?)))
        .collect();
    let mut jobs = Vec::new();
    for (id, to) in wanting {
        let path = may_follow(session, id, to);
        let Some(edits) = session.lists.get_mut(id) else { continue };
        let over_cap = identity::identities(edits).is_none_or(identity::Identities::over_cap);
        match path {
            Some(path) if !over_cap => jobs.push(FollowJob {
                tab: id,
                path,
                to,
                named: identity::identities(edits).map(|ids| ids.pages().to_vec()).unwrap_or_default(),
                hidden: edits.marks().hidden().to_vec(),
                changes: edits.changes(),
                document: identity::identities(edits).and_then(identity::Identities::document),
            }),
            _ => identity::follow_tried(edits, Some(to)),
        }
    }
    jobs
}

/// **A follow being read, kept by the page-identity thread across its steps** ([`read_part`]): the new version open, the match so far, and what's been
/// spent of the one budget. One at a time, and only while a tab's edits are following: nothing here per idle tab. What it holds is the job's copy of the
/// named pages, the match's few words for each (both at most [`identity::MAX_IDENTIFIED`]), and the file open as slice 1's thread keeps one.
pub struct Following {
    job: FollowJob,
    opened: crate::identify::Opened,
    matcher: Option<Matcher>,
    budget: Budget,
    phase: Phase,
    document: Option<recto_core::page_id::DocumentIds>,
}

impl std::fmt::Debug for Following {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Following").field("tab", &self.job.tab).field("phase", &self.phase).finish_non_exhaustive()
    }
}

/// Where a follow's reading is.
#[derive(Debug)]
enum Phase {
    /// The pages with an object number a named page had, from this place in the page list; each object once (`seen`).
    Objects { next: usize, seen: std::collections::HashSet<(u32, u16)> },
    /// Every page in order, from this place.
    Scan { next: usize },
    /// The file's highlights the edits take out, looked for again on their pages, by page: the match's verdicts, and those found so far.
    Highlights {
        found: Vec<(u32, Found)>,
        refound: Vec<(FileKey, Option<FileKey>)>,
        next: usize,
    },
}

/// What a step of a follow's reading came to.
#[derive(Debug)]
pub enum Read {
    /// More to read: the next step goes on from here.
    More,
    Matched(Box<Matched>),
    /// The file isn't the version found any more (or doesn't open as it): nothing read is used.
    NotRead,
}

/// What a step reads at most: as many bytes as a job of slice 1's (`crate::identify::MAX_JOB_BYTES`), and as many pages, so a step of pages that cost
/// lookups and no bytes is bounded too. A test sets it small on its own thread.
fn step_bytes() -> u64 {
    #[cfg(test)]
    if let Some(bytes) = crate::tests::version_change::STEP_BYTES.with(std::cell::Cell::get) {
        return bytes;
    }
    crate::identify::MAX_JOB_BYTES
}

const STEP_PAGES: u32 = crate::identify::MAX_PAGES_A_JOB as u32;

/// **Starts reading a follow:** the new version opened through the file wall and checked by its own descriptor against the version found (review S1),
/// so a file renamed in for the moment of the open is never matched for it. `None` if it isn't that version or doesn't open.
pub fn start(job: FollowJob) -> Option<Following> {
    let opened = crate::identify::Opened::open(&job.path, job.to)?;
    let document = recto_core::page_id::document_ids(opened.reader());
    // A re-save keeps `/ID[0]`; another document hasn't it, and a page found once in it needs a neighbour to agree (review S3).
    let same_document = matches!((job.document, document), (Some(was), Some(now)) if was.first == now.first);
    Some(Following {
        matcher: Some(Matcher::new(&job.named, same_document)),
        budget: Budget::new(work_caps()),
        phase: Phase::Objects {
            next: 0,
            seen: std::collections::HashSet::new(),
        },
        opened,
        job,
        document,
    })
}

impl Following {
    /// The job this is the reading of.
    pub fn job(&self) -> &FollowJob {
        &self.job
    }
}

/// **One step of a follow's reading,** off the session's lock: up to [`step_bytes`] bytes and as many pages as a job of slice 1's, then it says
/// there's more, so the page-identity thread is never held by one long file in one piece. Every step's work is under the one budget ([`WorkCaps`]).
pub fn read_part(following: &mut Following) -> Read {
    let (limit, mut spent, mut pages) = (step_bytes(), 0u64, 0u32);
    let caps = recto_core::page_id::Caps::STANDARD;
    let Following {
        job,
        opened,
        matcher,
        budget,
        phase,
        document,
    } = following;
    let (reader, map) = (opened.reader(), opened.map());
    loop {
        if spent >= limit || pages >= STEP_PAGES {
            return still(job, Read::More);
        }
        match phase {
            Phase::Objects { next, seen } => {
                let Some(matcher_now) = matcher.as_mut() else { return Read::NotRead };
                let ended = match map.get(*next) {
                    None => true,
                    Some(entry) => {
                        let id = entry.id();
                        let object = (id.0, id.1);
                        if !matcher_now.wants_object(object) {
                            *next += 1;
                            false
                        } else if !seen.insert(object) {
                            matcher_now.listed_twice(object);
                            true
                        } else if !budget.room() {
                            true
                        } else {
                            let read = recto_core::page_id::read_page(reader, entry, &caps);
                            budget.page(read.bytes);
                            (spent, pages) = (spent.saturating_add(read.bytes), pages + 1);
                            // A page that reached a cap has no identity: nothing is matched to it (slice 1's rule).
                            if let Some(identity) = &read.identity {
                                matcher_now.by_object(u32::try_from(*next).unwrap_or(u32::MAX), Known::of(identity));
                            }
                            *next += 1;
                            false
                        }
                    }
                };
                if ended {
                    *phase = after_objects(job, matcher);
                }
            }
            Phase::Scan { next } => {
                let Some(matcher_now) = matcher.as_mut() else { return Read::NotRead };
                let entry = map.get(*next);
                let at_budget = !budget.room();
                if entry.is_none() || at_budget {
                    if entry.is_none() {
                        matcher_now.scan_complete();
                    }
                    *phase = to_highlights(job, matcher);
                    continue;
                }
                let Some(entry) = entry else { continue };
                let read = recto_core::page_id::read_page(reader, entry, &caps);
                budget.page(read.bytes);
                (spent, pages) = (spent.saturating_add(read.bytes), pages + 1);
                match &read.identity {
                    Some(identity) => matcher_now.scanned(u32::try_from(*next).unwrap_or(u32::MAX), Known::of(identity)),
                    None => matcher_now.scanned_unidentified(),
                }
                *next += 1;
            }
            Phase::Highlights { found, refound, next } => {
                // The keys are in order of the page they're on, so each page's highlights are read once for all of its keys.
                let Some((key, _)) = refound.get(*next).copied() else {
                    let Ok(length) = u32::try_from(map.len()) else { return Read::NotRead };
                    let matched = Matched {
                        tab: job.tab,
                        to: job.to,
                        changes: job.changes,
                        found: std::mem::take(found),
                        pages: length,
                        refound: std::mem::take(refound),
                        document: *document,
                    };
                    return still(job, Read::Matched(Box::new(matched)));
                };
                let at = found.binary_search_by_key(&key.page, |(page, _)| *page).ok().and_then(|at| found[at].1.page());
                let end = refound[*next..].iter().position(|(other, _)| other.page != key.page).map_or(refound.len(), |run| *next + run);
                if let Some((page, _)) = at
                    && budget.room()
                    && let Some(entry) = map.get(page as usize)
                {
                    let (highlights, looked) = recto_core::highlights::page_highlights_at(reader, entry);
                    budget.lookups((looked as u64).saturating_mul(ANNOT_LOOKUPS).saturating_add(1));
                    pages += 1;
                    let on_page: Vec<(u32, Option<[f32; 4]>)> = highlights.found.iter().map(|highlight| (highlight.index, highlight.rect)).collect();
                    for (key, again) in &mut refound[*next..end] {
                        *again = rebase::refind(*key, page, &on_page);
                    }
                }
                // Not found, or past the budget: not found again, and that edit is held.
                *next = end;
            }
        }
    }
}

/// The step's answer, if the file is still the version found; else nothing it read is used.
fn still(job: &FollowJob, read: Read) -> Read {
    if recto_platform::file_stamp(&job.path).ok() == Some(job.to) { read } else { Read::NotRead }
}

/// After the pass by object number: the highlights if every page read before is found by it, else the scan.
fn after_objects(job: &FollowJob, matcher: &mut Option<Matcher>) -> Phase {
    let read: Vec<u32> = job.named.iter().filter(|(_, known)| known.is_some()).map(|(page, _)| *page).collect();
    if matcher.as_ref().is_some_and(|matcher| matcher.all_found_by_object(&read)) {
        to_highlights(job, matcher)
    } else {
        Phase::Scan { next: 0 }
    }
}

/// The match decided, and the highlights to look for again, by the page they're on.
fn to_highlights(job: &FollowJob, matcher: &mut Option<Matcher>) -> Phase {
    let found = matcher.take().map(|matcher| matcher.finish(&job.named)).unwrap_or_default();
    let mut refound: Vec<(FileKey, Option<FileKey>)> = job.hidden.iter().map(|key| (*key, None)).collect();
    refound.sort_by_key(|(key, _)| key.page);
    Phase::Highlights { found, refound, next: 0 }
}

/// **Reads the new version and matches the named pages in it, every step at once,** off the session's lock: `None` if the file isn't the version found,
/// before the read or after it, or can't be opened. What the tests use; the page-identity thread takes it a step at a time.
pub fn read(job: &FollowJob) -> Option<Matched> {
    let mut following = start(job.clone())?;
    loop {
        match read_part(&mut following) {
            Read::More => {}
            Read::Matched(matched) => return Some(*matched),
            Read::NotRead => return None,
        }
    }
}

/// Whether a follow being read is still wanted: the tab may still follow into that version, and nothing was edited since its job was made. A look at
/// one tab, under the lock, between steps.
pub fn current<L: DocLoader>(session: &Session<L>, job: &FollowJob) -> bool {
    may_follow(session, job.tab, job.to).is_some()
        && session
            .lists
            .get(job.tab)
            .is_some_and(|edits| identity::follow_wanted(edits) == Some(job.to) && edits.changes() == job.changes)
}

/// **A job's file couldn't be read as the version found** (it changed again, or doesn't open): not tried again for that version. A change since is
/// found again by the next read of the tab, which asks for that one.
pub fn not_read<L: DocLoader>(session: &mut Session<L>, job: &FollowJob) {
    if let Some(edits) = session.lists.get_mut(job.tab) {
        identity::follow_tried(edits, Some(job.to));
    }
}

/// **Takes a match in,** if the tab's edits are still about the version they were and the file is still the one matched, and nothing was edited
/// meanwhile: every edit follows its page and the tab says so, or the list is held exactly as it was, and isn't tried again for that version.
/// `None` when nothing changed.
pub fn followed<L: DocLoader>(session: &mut Session<L>, matched: &Matched) -> Option<Followed> {
    let id = matched.tab;
    may_follow(session, id, matched.to)?;
    let edits = session.lists.get_mut(id)?;
    if identity::follow_wanted(edits) != Some(matched.to) || edits.changes() != matched.changes {
        // Another version since, or an edit: the next job is about that.
        return None;
    }
    let outcome = rebase::rebase(edits, &matched.found, matched.pages, &matched.refound);
    let Some(onto) = outcome.onto else {
        identity::follow_tried(edits, Some(matched.to));
        return None;
    };
    let positions = onto.positions.clone();
    let let_go = rebase::apply(edits, onto, matched.document);
    // Not told yet: kept in the sidecar's header, so a quit, a hand-off or a crash before the person has seen it doesn't lose it (review F3).
    identity::set_followed_untold(edits, true);
    let (pages, seq) = (edits.len(), edits.journal_seq());
    let count = u32::try_from(outcome.edits.len()).unwrap_or(u32::MAX);
    session.journals.retire_kept(let_go);
    on_the_new_version(session, id, matched, &positions, count);
    // A journal goes by its file's identity, which this has changed, as after a save: the old one goes once the new one's there.
    let replaces = session.journals.forget_name(id).map(String::from);
    session.journals.drop_fresh(id);
    if session.journals.journaling() {
        session.journal_desk().name_for(id);
        session.journals.start_fresh(id, seq, replaces);
    } else if let Some(old) = replaces {
        session.journals.retire_name(old.into());
    }
    Some(Followed { tab: id, pages, edits: count })
}

/// **The person has been shown that a tab's edits followed** (they left the tab with its notice): its sidecar says so no more.
pub(crate) fn told<L: DocLoader>(session: &mut Session<L>, id: TabId) {
    if let Some(edits) = session.lists.get_mut(id) {
        identity::set_followed_untold(edits, false);
    }
}

/// **The notice for a list restored that followed its file and wasn't told** (its sidecar's header), with how many edits it has, as the follow counted
/// them ([`rebase::edits_in`]). Counts only.
pub(crate) fn untold_notice(edits: &PageEdits) -> Option<FileNotice> {
    identity::followed_untold(edits).then(|| FileNotice::EditsFollowed { edits: rebase::edits_in(edits) })
}

/// The tab is about the new version now: its stamp and length, what's held of the old one gone, the reader on the page they were on, and said.
fn on_the_new_version<L: DocLoader>(session: &mut Session<L>, id: TabId, matched: &Matched, positions: &[Option<u32>], edits: u32) {
    session.journal_desk().retire_sleep_file(id);
    session.generated.forget(id);
    let pages = session.lists.get(id).map_or(matched.pages, PageEdits::len);
    // Where a place in the list as it was is now (`rebase::moved_position`).
    let moved = |position: u32| Some(rebase::moved_position(positions, position, pages));
    if let Some(tab) = session.tabs.tab_mut(id) {
        tab.folds = None;
        tab.file.take_version(Some(matched.to));
        tab.file.set_notice(Some(FileNotice::EditsFollowed { edits }));
        tab.page_count = matched.pages;
        tab.page = rebase::moved_position(positions, tab.page, pages);
        tab.place_dirty = true;
        tab.problem = None;
        if let Some(trail) = tab.reading.trail.as_deref_mut() {
            trail.follow_pages(moved);
        }
    }
    session.mail.forget_draws(id);
}
