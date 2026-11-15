//! **Which page of a new version of the file each page an edit names is** (the page matcher, slice 2; owner, 2026-11-13: "we need to run a page
//! matcher of some kind. That's the way we can ensure edits on the correct page"). Pure: it's given what the old pages were ([`super::identity`]) and
//! the new version's pages as they're read, in order, and says of each old page where it is, only where nothing is in doubt. Nothing here reads a
//! file: [`crate::version_change`] feeds it, off the session's lock, on the page-identity thread.
//!
//! **The rules** (the coordinator's design, accepted 2026-11-15):
//! 1. The new page with the old page's object number, if it still has the old digest, is that page ([`Matcher::by_object`]). Our own appends keep the
//!    number; other programs mostly renumber (research §3), so this is a fast first try, never the only one.
//! 2. Otherwise the new version is scanned in order ([`Matcher::scanned`]): a digest found once is that page; found more than once, it's the copy
//!    whose neighbour either side has the old page's neighbour's digest, if exactly one copy has; else not matched. Found nowhere: not matched.
//! 3. Two old pages that come to one new page are neither matched.
//! 4. A scan stopped at its caps ([`ScanCaps`]) can't say a digest was found only once, so it matches nothing by digest: only rule 1 holds then.
//!
//! **Bounded** (rule 1 and rule 6): memory is a few words a page named ([`super::identity::MAX_IDENTIFIED`] at most), never a word a page of the new
//! version, which can be as long as a hostile file likes. An old digest that more than [`MAX_ALIKE`] named pages share is told apart by nothing, so
//! those pages aren't matched by digest and cost the scan nothing (a scan's work a page is at most [`MAX_ALIKE`] comparisons).

use super::identity::Known;
use std::collections::HashMap;

/// Most named pages one digest is looked for for: past it they're all alike (blank pages, separators), so nothing tells them apart and none is matched
/// by digest.
pub const MAX_ALIKE: usize = 64;

/// How far a scan of the new version goes: at most this many pages, and this many bytes hashed or read, whichever comes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanCaps {
    pub pages: u32,
    pub bytes: u64,
}

impl ScanCaps {
    /// 200,000 pages (as many as an edited document may have, [`super::MAX_PAGES`]) or 2 GiB.
    pub const STANDARD: Self = Self {
        pages: 200_000,
        bytes: 2 << 30,
    };
}

/// **What's left of a scan's caps,** asked before each page is read and told what it cost after.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    caps: ScanCaps,
    pages: u32,
    bytes: u64,
}

impl Budget {
    pub fn new(caps: ScanCaps) -> Self {
        Self { caps, pages: 0, bytes: 0 }
    }

    /// Whether another page may be read.
    pub fn room(&self) -> bool {
        self.pages < self.caps.pages && self.bytes < self.caps.bytes
    }

    /// A page was read, at this many bytes hashed or read.
    pub fn spend(&mut self, cost: u64) {
        self.pages = self.pages.saturating_add(1);
        self.bytes = self.bytes.saturating_add(cost);
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
    neighbours: Vec<(Option<[u8; 16]>, Option<[u8; 16]>)>,
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
}

impl Matcher {
    /// From the named pages and what each was, ascending, as [`super::identity::Identities::pages`] keeps them; pages not read are left out.
    pub fn new(named: &[(u32, Option<Known>)]) -> Self {
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
        }
    }

    /// The object numbers to look for in the new version's page list, so only those pages are read for rule 1.
    pub fn wants_object(&self, object: (u32, u16)) -> bool {
        self.by_object.contains_key(&object)
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

    /// Whether every one of `pages` (old page numbers) is found by its object number, so no scan is needed.
    pub fn all_found_by_object(&self, pages: &[u32]) -> bool {
        pages.iter().all(|page| {
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
            if (old_before.is_some() && old_before == before) || (old_after.is_some() && old_after == after) {
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
                Some(Some((new, now))) => Found::At { page: new, now },
                _ if !(self.scanned && self.complete) => Found::NotFound,
                _ => {
                    let seen = self.seen[at];
                    match (seen.copies, seen.by_neighbour) {
                        (0, _) => Found::NotFound,
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

/// **The whole match over a new version in hand,** as the driver runs it: by object number, then a scan if any of `wanted` isn't found, cut at `caps`
/// (counting each page's `cost`, its bytes hashed or read). For the table tests, where the new version is a list; a page of it with no identity is
/// `None`.
pub fn match_pages(named: &[(u32, Option<Known>)], wanted: &[u32], new: &[(Option<Known>, u64)], caps: ScanCaps) -> Vec<(u32, Found)> {
    let mut matcher = Matcher::new(named);
    for (page, (now, _)) in new.iter().enumerate() {
        if let Some(now) = now
            && matcher.wants_object(now.object)
        {
            matcher.by_object(u32::try_from(page).unwrap_or(u32::MAX), *now);
        }
    }
    if !matcher.all_found_by_object(wanted) {
        let mut budget = Budget::new(caps);
        let mut complete = true;
        for (page, (now, cost)) in new.iter().enumerate() {
            if !budget.room() {
                complete = false;
                break;
            }
            budget.spend(*cost);
            match now {
                Some(now) => matcher.scanned(u32::try_from(page).unwrap_or(u32::MAX), *now),
                None => matcher.scanned_unidentified(),
            }
        }
        if complete {
            matcher.scan_complete();
        }
    }
    matcher.finish(named)
}
