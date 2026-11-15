//! **Which page of a new version of the file each page an edit names is** (the page matcher, slice 2; owner, 2026-11-13: "we need to run a page
//! matcher of some kind. That's the way we can ensure edits on the correct page"). Pure: it's given what the old pages were ([`super::identity`]) and
//! the new version's pages as they're read, in order, and says of each old page where it is, only where nothing is in doubt. Nothing here reads a
//! file: [`crate::version_change`] feeds it, off the session's lock, on the page-identity thread.
//!
//! **The rules** (the coordinator's design, accepted 2026-11-15):
//! 1. The new page with the old page's object number, if it still has the old digest, is that page ([`Matcher::by_object`]), **only where the new
//!    version has the edits' version's `/ID[0]`** (review M1): another document from the same generator can have a stock page at the same number.
//!    Our own appends and re-saves keep the number and `/ID[0]`; other programs mostly renumber (research §3), so this is a fast first try, never
//!    the only one.
//! 2. Otherwise the new version is scanned in order ([`Matcher::scanned`]): a digest found once is that page only with other evidence (review S3,
//!    decided 2026-11-15): a page next to it, either side, with the digest of a page next to it before, or the new version's `/ID[0]` the same as the edits'
//!    version's (a re-save keeps it); else not matched, as a blank or stock page another document shares is no evidence it's ours. Found more than
//!    once, it's the copy whose neighbour either side agrees, if exactly one copy's does; else not matched. Found nowhere: not matched.
//! 3. Two old pages that come to one new page are neither matched.
//! 4. A scan stopped at the budget ([`WorkCaps`]) can't say a digest was found only once, so it matches nothing by digest: only rule 1 holds then.
//!
//! **Bounded** (rule 1 and rule 6): memory is a few words a page named ([`super::identity::MAX_IDENTIFIED`] at most), never a word a page of the new
//! version, which can be as long as a hostile file likes. An old digest that more than [`MAX_ALIKE`] named pages share is told apart by nothing, so
//! those pages aren't matched by digest and cost the scan nothing (a scan's work a page is at most [`MAX_ALIKE`] comparisons).

use super::identity::Known;
use std::collections::HashMap;

/// Most named pages one digest is looked for for: past it they're all alike (blank pages, separators), so nothing tells them apart and none is matched
/// by digest.
pub const MAX_ALIKE: usize = 64;

/// **The one budget a follow's reading of the new version spends** (review F2): the pages read by object number, the scan, and the file's highlights
/// looked for again, all counted together in work, never in time (a time is a machine's). Bytes are what [`recto_core::page_id::read_page`] charges
/// (stored lengths and bytes hashed); pages are digests read; lookups are objects looked up. At the budget the scan isn't complete, so nothing is
/// matched by digest, and an edit not found by then is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkCaps {
    pub bytes: u64,
    pub pages: u32,
    pub lookups: u64,
}

impl WorkCaps {
    /// 2 GiB, 200,000 pages (as many as an edited document may have, [`super::MAX_PAGES`]), and 300 million lookups (about what 200,000 pages
    /// are charged at [`PAGE_LOOKUPS`] each, with room for the highlights looked for).
    pub const STANDARD: Self = Self {
        bytes: 2 << 30,
        pages: 200_000,
        lookups: 300_000_000,
    };
}

/// **What a page's read is charged in lookups: a bound, not a count.** The digest's own lookups aren't told (`PageRead` says bytes only), so each read is
/// charged the most it can dereference under `recto-page/1`'s caps: its content streams, the XObjects it draws, and a few for the page, its resources
/// and their dictionaries. A page of references to things that aren't streams costs no bytes, and this.
pub const PAGE_LOOKUPS: u64 = recto_core::page_id::Caps::STANDARD.content_streams as u64 + recto_core::page_id::Caps::STANDARD.xobjects as u64 + 4;

/// What a highlight looked at on a page is charged in lookups, again a bound: itself, its subtype, colour and rect, and each of its quadrilaterals' at
/// most.
pub const ANNOT_LOOKUPS: u64 = 4 + recto_core::highlights::MAX_QUADS as u64;

/// **What's left of [`WorkCaps`],** asked before each piece of work and told what it cost after.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    caps: WorkCaps,
    bytes: u64,
    pages: u32,
    lookups: u64,
}

impl Budget {
    pub fn new(caps: WorkCaps) -> Self {
        Self {
            caps,
            bytes: 0,
            pages: 0,
            lookups: 0,
        }
    }

    /// Whether more may be read.
    pub fn room(&self) -> bool {
        self.pages < self.caps.pages && self.bytes < self.caps.bytes && self.lookups < self.caps.lookups
    }

    /// A page was read, at this many bytes.
    pub fn page(&mut self, bytes: u64) {
        self.pages = self.pages.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        self.lookups = self.lookups.saturating_add(PAGE_LOOKUPS);
    }

    /// The pages read so far.
    pub fn pages(&self) -> u32 {
        self.pages
    }

    /// Objects were looked up.
    pub fn lookups(&mut self, lookups: u64) {
        self.lookups = self.lookups.saturating_add(lookups);
    }
}

/// Where an old page is in the new version, or why it isn't known to be anywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// It's this page of the new version, which is this.
    At { page: u32, now: Known },
    /// Not one of the pages named, or named and not read in time, so there's nothing to find it by.
    NotIdentified,
    /// No page of the new version has its digest (or the scan was cut short before one was known to be the only one).
    NotFound,
    /// Several have it, and its neighbours don't tell which.
    Ambiguous,
    /// One has it, but nothing else says it's ours: no neighbour agrees, and the new version hasn't the edits' version's `/ID[0]`.
    Unconfirmed,
    /// Another old page came to the same new page.
    Claimed,
}

impl Found {
    pub fn page(self) -> Option<(u32, Known)> {
        match self {
            Self::At { page, now } => Some((page, now)),
            _ => None,
        }
    }
}

/// The digests of a named page's neighbours in the old file, before and after, where those were named and read.
type Neighbours = (Option<[u8; 16]>, Option<[u8; 16]>);

/// What a scan has seen of one named page's digest.
#[derive(Clone, Copy, Debug, Default)]
struct Seen {
    copies: u32,
    first: Option<(u32, Known)>,
    by_neighbour: u32,
    neighbour_at: Option<(u32, Known)>,
}

/// **The match in progress.** Made from the named pages that were read; given the new version's pages by object number first, then, if that didn't
/// find every page wanted, the whole version in order; then [`Self::finish`].
pub struct Matcher {
    /// The named pages read, ascending: (old page, what it was).
    old: Vec<(u32, Known)>,
    /// The digests of each one's neighbours in the old file, where those were named and read.
    neighbours: Vec<Neighbours>,
    by_object: HashMap<(u32, u16), usize>,
    by_digest: HashMap<[u8; 16], Vec<usize>>,
    /// Found by object number: the new page, or `None` where two new pages had the number (a hostile page tree can list one page twice).
    by_id: Vec<Option<Option<(u32, Known)>>>,
    seen: Vec<Seen>,
    /// The last two pages fed to the scan, waiting for the page after them: (page, what it is, the digest before it).
    window: Option<(u32, Known, Option<[u8; 16]>)>,
    scanned: bool,
    /// The scan read every page.
    complete: bool,
    /// The new version has the edits' version's `/ID[0]`, which makes a digest found once enough.
    same_document: bool,
}

impl Matcher {
    /// From the named pages and what each was, ascending, as [`super::identity::Identities::pages`] keeps them; pages not read are left out.
    /// `same_document`: the new version's `/ID[0]` is the one the edits' version had (both known).
    pub fn new(named: &[(u32, Option<Known>)], same_document: bool) -> Self {
        let old: Vec<(u32, Known)> = named.iter().filter_map(|(page, known)| Some((*page, (*known)?))).collect();
        let digest_of = |page: Option<u32>| {
            page.and_then(|page| old.binary_search_by_key(&page, |(at, _)| *at).ok())
                .map(|at| old[at].1.digest)
        };
        let neighbours = old
            .iter()
            .map(|(page, _)| (digest_of(page.checked_sub(1)), digest_of(page.checked_add(1))))
            .collect();
        let mut by_object = HashMap::with_capacity(old.len());
        let mut by_digest: HashMap<[u8; 16], Vec<usize>> = HashMap::with_capacity(old.len());
        for (at, (_, known)) in old.iter().enumerate() {
            by_object.insert(known.object, at);
            by_digest.entry(known.digest).or_default().push(at);
        }
        // Too many alike to tell apart: none of them is looked for by digest.
        by_digest.retain(|_, alike| alike.len() <= MAX_ALIKE);
        Self {
            by_id: vec![None; old.len()],
            seen: vec![Seen::default(); old.len()],
            old,
            neighbours,
            by_object,
            by_digest,
            window: None,
            scanned: false,
            complete: false,
            same_document,
        }
    }

    /// The object numbers to look for in the new version's page list, so only those pages are read for rule 1: none in another document, where rule
    /// 1 doesn't hold, so nothing is spent on it there.
    pub fn wants_object(&self, object: (u32, u16)) -> bool {
        self.same_document && self.by_object.contains_key(&object)
    }

    /// **Rule 1:** page `page` of the new version has an object number a named page had, and is `now`.
    pub fn by_object(&mut self, page: u32, now: Known) {
        let Some(&at) = self.by_object.get(&now.object) else { return };
        if now.digest != self.old[at].1.digest {
            return;
        }
        self.by_id[at] = match self.by_id[at] {
            None => Some(Some((page, now))),
            // The same object listed twice: neither copy is it.
            Some(_) => Some(None),
        };
    }

    /// **A page list names this object a second time** (a hostile page tree can list one page many times): no copy of it is that page by its number,
    /// and the pass by object number stops here, as one that went on could be made to read the same page without end.
    pub fn listed_twice(&mut self, object: (u32, u16)) {
        if let Some(&at) = self.by_object.get(&object) {
            self.by_id[at] = Some(None);
        }
    }

    /// Whether every one of `pages` (old page numbers) is found by its object number, so no scan is needed. Never for another document (review M1):
    /// its pages need a neighbour to agree, which only the scan can see.
    pub fn all_found_by_object(&self, pages: &[u32]) -> bool {
        self.same_document && pages.iter().all(|page| {
            self.old
                .binary_search_by_key(page, |(at, _)| *at)
                .is_ok_and(|at| matches!(self.by_id[at], Some(Some(_))))
        })
    }

    /// **Rule 2:** the next page of the new version, in order from the first, is `now`. Each page is decided once the page after it is seen.
    pub fn scanned(&mut self, page: u32, now: Known) {
        self.scanned = true;
        let before = match self.window.take() {
            Some((last, known, before_last)) => {
                self.decide(last, known, before_last, Some(now.digest));
                Some(known.digest)
            }
            None => None,
        };
        self.window = Some((page, now, before));
    }

    /// **The next page of the new version has no identity** (it reached a cap of the digest's, so it's like no page named, which never did): it's no
    /// copy of any, and no neighbour of the pages either side of it.
    pub fn scanned_unidentified(&mut self) {
        self.scanned = true;
        if let Some((last, known, before_last)) = self.window.take() {
            self.decide(last, known, before_last, None);
        }
    }

    /// The scan read every page of the new version: without this, it was cut at its caps.
    pub fn scan_complete(&mut self) {
        self.complete = true;
    }

    fn decide(&mut self, page: u32, now: Known, before: Option<[u8; 16]>, after: Option<[u8; 16]>) {
        let Some(alike) = self.by_digest.get(&now.digest) else { return };
        for &at in alike {
            let seen = &mut self.seen[at];
            seen.copies = seen.copies.saturating_add(1);
            if seen.first.is_none() {
                seen.first = Some((page, now));
            }
            let (old_before, old_after) = self.neighbours[at];
            // Next to a page that was next to it, on either side: a run another program reversed keeps its pages' neighbours, the other way round.
            if [old_before, old_after]
                .into_iter()
                .flatten()
                .any(|old| Some(old) == before || Some(old) == after)
            {
                seen.by_neighbour = seen.by_neighbour.saturating_add(1);
                seen.neighbour_at = Some((page, now));
            }
        }
    }

    /// **Where each named page is,** ascending by old page: for every page named, read or not.
    pub fn finish(mut self, named: &[(u32, Option<Known>)]) -> Vec<(u32, Found)> {
        if let Some((last, known, before)) = self.window.take() {
            self.decide(last, known, before, None);
        }
        let mut found: Vec<(u32, Found)> = Vec::with_capacity(named.len());
        let mut old = self.old.iter().enumerate().peekable();
        for (page, _) in named {
            let Some((at, _)) = old.next_if(|(_, (old_page, _))| old_page == page) else {
                found.push((*page, Found::NotIdentified));
                continue;
            };
            let verdict = match self.by_id[at] {
                // Rule 1 holds only in the same document: another document's page at our old number, with our old digest (a stock or blank page from
                // the same generator), is no more evidence than a digest found once, and goes by the scan's rules (review M1).
                Some(Some((new, now))) if self.same_document => Found::At { page: new, now },
                _ if !(self.scanned && self.complete) => Found::NotFound,
                _ => {
                    let seen = self.seen[at];
                    match (seen.copies, seen.by_neighbour) {
                        (0, _) => Found::NotFound,
                        (1, 0) if !self.same_document => Found::Unconfirmed,
                        (1, _) => seen.first.map_or(Found::NotFound, |(page, now)| Found::At { page, now }),
                        (_, 1) => seen.neighbour_at.map_or(Found::Ambiguous, |(page, now)| Found::At { page, now }),
                        _ => Found::Ambiguous,
                    }
                }
            };
            found.push((*page, verdict));
        }
        // Rule 3: two old pages at one new page, neither.
        let mut claims: HashMap<u32, u32> = HashMap::new();
        for (_, verdict) in &found {
            if let Found::At { page, .. } = verdict {
                *claims.entry(*page).or_default() += 1;
            }
        }
        for (_, verdict) in &mut found {
            if let Found::At { page, .. } = verdict
                && claims.get(page).is_some_and(|claims| *claims > 1)
            {
                *verdict = Found::Claimed;
            }
        }
        found
    }
}

/// **The whole match over a new version in hand,** as the session runs it: by object number, then a scan if any of `wanted` isn't found, all under one
/// budget of `caps` (each page costing its bytes, and [`PAGE_LOOKUPS`]). For the table tests, where the new version is a list; a page of it with no
/// identity is `None`.
pub fn match_pages(named: &[(u32, Option<Known>)], wanted: &[u32], new: &[(Option<Known>, u64)], caps: WorkCaps, same_document: bool) -> Vec<(u32, Found)> {
    match_pages_spending(named, wanted, new, caps, same_document).0
}

/// [`match_pages`], with what it spent of the budget.
pub fn match_pages_spending(
    named: &[(u32, Option<Known>)],
    wanted: &[u32],
    new: &[(Option<Known>, u64)],
    caps: WorkCaps,
    same_document: bool,
) -> (Vec<(u32, Found)>, Budget) {
    let mut matcher = Matcher::new(named, same_document);
    let mut budget = Budget::new(caps);
    let mut seen = std::collections::HashSet::new();
    for (page, (now, cost)) in new.iter().enumerate() {
        let Some(now) = now else { continue };
        if !matcher.wants_object(now.object) {
            continue;
        }
        if !seen.insert(now.object) {
            matcher.listed_twice(now.object);
            break;
        }
        if !budget.room() {
            break;
        }
        budget.page(*cost);
        matcher.by_object(u32::try_from(page).unwrap_or(u32::MAX), *now);
    }
    if !matcher.all_found_by_object(wanted) {
        let mut complete = true;
        for (page, (now, cost)) in new.iter().enumerate() {
            if !budget.room() {
                complete = false;
                break;
            }
            budget.page(*cost);
            match now {
                Some(now) => matcher.scanned(u32::try_from(page).unwrap_or(u32::MAX), *now),
                None => matcher.scanned_unidentified(),
            }
        }
        if complete {
            matcher.scan_complete();
        }
    }
    (matcher.finish(named), budget)
}
