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
use crate::edit::matcher::{Budget, Found, Matcher, ScanCaps};
use crate::edit::{PageEdits, rebase};
use crate::fit::EditsFit;
use crate::record::FileNotice;
use crate::{DocLoader, Session, TabId};
use recto_platform::FileStamp;
use std::path::PathBuf;

/// The scan's caps, which a test sets small on its own thread to hold them at their limit plus one with a small file.
fn scan_caps() -> ScanCaps {
    #[cfg(test)]
    if let Some(caps) = crate::tests::version_change::SCAN_CAPS.with(std::cell::Cell::get) {
        return caps;
    }
    ScanCaps::STANDARD
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
            }),
            _ => identity::follow_tried(edits, Some(to)),
        }
    }
    jobs
}

/// **Reads the new version and matches the named pages in it,** off the session's lock: `None` if the file isn't the version found, before the read or
/// after it, or can't be opened. Bounded by [`ScanCaps`] and the digest's own caps a page.
pub fn read(job: &FollowJob) -> Option<Matched> {
    let current = || recto_platform::file_stamp(&job.path).ok() == Some(job.to);
    if !current() {
        return None;
    }
    let reader = recto_core::page_id::open_for_identity(&job.path).ok()?;
    let map = reader.page_map().ok()?;
    let caps = recto_core::page_id::Caps::STANDARD;
    let identify = |entry| {
        let read = recto_core::page_id::read_page(&reader, entry, &caps);
        // A page that reached a cap has no identity: nothing is matched to it (slice 1's rule).
        (read.identity.as_ref().map(Known::of), read.bytes)
    };
    let mut matcher = Matcher::new(&job.named);
    // Rule 1: the pages with an object number a named page had, at most as many as are named, under the scan's bytes as a whole (not its pages: a
    // re-save that keeps the numbers is matched without a scan however long it is).
    let mut budget = Budget::new(ScanCaps {
        pages: u32::MAX,
        bytes: ScanCaps::STANDARD.bytes,
    });
    for (page, entry) in map.iter().enumerate() {
        let id = entry.id();
        if !matcher.wants_object((id.0, id.1)) {
            continue;
        }
        if !budget.room() {
            break;
        }
        let (now, cost) = identify(entry);
        budget.spend(cost);
        if let Some(now) = now {
            matcher.by_object(u32::try_from(page).ok()?, now);
        }
    }
    let read: Vec<u32> = job.named.iter().filter(|(_, known)| known.is_some()).map(|(page, _)| *page).collect();
    if !matcher.all_found_by_object(&read) {
        let mut budget = Budget::new(scan_caps());
        let mut complete = true;
        for (page, entry) in map.iter().enumerate() {
            if !budget.room() {
                complete = false;
                break;
            }
            let (now, cost) = identify(entry);
            budget.spend(cost);
            match now {
                Some(now) => matcher.scanned(u32::try_from(page).ok()?, now),
                None => matcher.scanned_unidentified(),
            }
        }
        if complete {
            matcher.scan_complete();
        }
    }
    let found = matcher.finish(&job.named);
    let refound = job
        .hidden
        .iter()
        .map(|key| {
            let at = found.binary_search_by_key(&key.page, |(page, _)| *page).ok().and_then(|at| found[at].1.page());
            let again = at.and_then(|(page, _)| {
                let on_page: Vec<(u32, Option<[f32; 4]>)> = recto_core::highlights::page_highlights_in(&reader, page)
                    .found
                    .iter()
                    .map(|highlight| (highlight.index, highlight.rect))
                    .collect();
                rebase::refind(*key, page, &on_page)
            });
            (*key, again)
        })
        .collect();
    let pages = u32::try_from(map.len()).ok()?;
    let document = recto_core::page_id::document_ids(&reader);
    drop(map);
    drop(reader);
    if !current() {
        return None;
    }
    Some(Matched {
        tab: job.tab,
        to: job.to,
        changes: job.changes,
        found,
        pages,
        refound,
        document,
    })
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

/// The tab is about the new version now: its stamp and length, what's held of the old one gone, the reader on the page they were on, and said.
fn on_the_new_version<L: DocLoader>(session: &mut Session<L>, id: TabId, matched: &Matched, positions: &[Option<u32>], edits: u32) {
    // Where a position of the list as it was is now: its own place if it's known, else the nearest before it that is.
    let moved = |position: u32| -> Option<u32> {
        let position = (position as usize).min(positions.len().checked_sub(1)?);
        positions[..=position].iter().rev().find_map(|at| *at)
    };
    session.journal_desk().retire_sleep_file(id);
    session.generated.forget(id);
    let pages = session.lists.get(id).map_or(matched.pages, PageEdits::len);
    if let Some(tab) = session.tabs.tab_mut(id) {
        tab.folds = None;
        tab.file.take_version(Some(matched.to));
        tab.file.set_notice(Some(FileNotice::EditsFollowed { edits }));
        tab.page_count = matched.pages;
        tab.page = moved(tab.page).unwrap_or(0).min(pages.saturating_sub(1));
        tab.place_dirty = true;
        tab.problem = None;
        if let Some(trail) = tab.reading.trail.as_deref_mut() {
            trail.follow_pages(moved);
        }
    }
    session.mail.forget_draws(id);
}
