//! **A list's edits onto another version of their file** (the page matcher, slice 2; owner, 2026-11-13; the rule: unsaved edits stay on their own page
//! when the file changes, or are held and said, never put on another page). Pure: given where each named page is in the new version
//! ([`super::matcher`]), [`rebase`] says of every edit whether it follows its page, and builds the list on the new version only if every one does
//! (all or nothing, for 0.1.1: the per-edit verdicts are kept in [`Outcome::edits`] for slice 3, which sets aside one by one). [`apply`] puts it in.
//!
//! **How the new list is made** (the coordinator's design and the owner's answers of 2026-11-15):
//! - Pages the edits don't touch take the new version's order and content, so pages another program added are there.
//! - The person's own pages still in the file's order (the longest such run, [`in_order`]) are *fixed*: each is where the new version has it, with its
//!   turn. A turned fixed page has to be found.
//! - A page the person moved (one of theirs out of that run) goes after the fixed page it follows in their order, wherever the new version has that
//!   page; where both reordered, the person's placement wins, relative to the page theirs was next to. Pages added from another PDF go the same way. A
//!   run at the very start goes before the fixed page after it. The moved page and that page both have to be found.
//! - A page the person deleted has to be found, and is taken out.
//! - A highlight made as an edit goes onto its page if the page is found and its frame (crop and turn, [`super::identity::Known::frame`]) is the same;
//!   a removal of the file's own highlight, if the page is found and the highlight is found on it again ([`refind`]).
//! - Edits on a page the person deleted (a highlight made or taken out there) can be shown or reached only by undoing the delete, and the undo history
//!   ends at a followed change (owner, 2026-11-15), so they go with it; nothing a person can see or save is lost by that.

use super::identity::Known;
use super::marks::FileKey;
use super::matcher::Found;
use super::{MAX_PAGES, PageEdits, PageSlot};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// One edit, as the matcher judges it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// A page of the file turned, by its page in the file as it was.
    Turn {
        page: u32,
    },
    Delete {
        page: u32,
    },
    /// A page of the file moved out of the file's order.
    Move {
        page: u32,
    },
    /// A run of pages added from another PDF (`source`, as a slot says).
    Insert {
        source: u16,
        pages: u32,
    },
    /// A highlight made as an edit, by its number.
    Highlight {
        mark: u32,
    },
    /// One of the file's own highlights taken out, on this page of the file as it was.
    FileHighlightOut {
        page: u32,
    },
}

/// Why an edit can't follow its page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// Its page (or the page it's placed by) wasn't named, or wasn't read in time.
    NotIdentified,
    /// No page of the new version is it.
    NotFound,
    /// Several pages of the new version could be it.
    Ambiguous,
    /// Another page came to the same page of the new version.
    Claimed,
    /// The page it's placed after (or before) can't be found, or there's none.
    NoAnchor,
    /// Its page is found, but shown in another frame (crop or turn) there.
    FrameChanged,
    /// The file's highlight it takes out isn't found on its page in the new version.
    HighlightNotFound,
    /// The list isn't one an edit makes (a page of the file there twice).
    Unsound,
    /// The new list would be longer than a document may be.
    TooLong,
}

/// What an edit came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditMatch {
    pub edit: Edit,
    /// `None` when it follows its page.
    pub held: Option<Held>,
}

/// The list on the new version, ready for [`apply`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Onto {
    pub file_pages: u32,
    pub slots: Vec<PageSlot>,
    /// Each highlight of the tab's own file that stays, by number, and its page in the new version.
    marks: HashMap<u32, u32>,
    hidden: Vec<FileKey>,
    /// What each page named is in the new version, by its page there.
    named: Vec<(u32, Known)>,
    /// Where each position of the list as it was is now, if it's still there and known.
    pub positions: Vec<Option<u32>>,
}

/// What [`rebase`] found: every edit's verdict, and the new list if every one follows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub edits: Vec<EditMatch>,
    pub onto: Option<Onto>,
}

impl Outcome {
    fn held(edit: Edit, why: Held) -> Self {
        Self {
            edits: vec![EditMatch { edit, held: Some(why) }],
            onto: None,
        }
    }
}

/// Where a page of the file as it was is in the new version, from the matcher's verdicts (ascending by old page).
fn found_at(found: &[(u32, Found)], page: u32) -> Result<(u32, Known), Held> {
    match found.binary_search_by_key(&page, |(at, _)| *at).map(|at| found[at].1) {
        Ok(Found::At { page, now }) => Ok((page, now)),
        Ok(Found::NotFound) => Err(Held::NotFound),
        Ok(Found::Ambiguous) => Err(Held::Ambiguous),
        Ok(Found::Claimed) => Err(Held::Claimed),
        Ok(Found::NotIdentified) | Err(_) => Err(Held::NotIdentified),
    }
}

/// **Which of the person's own pages are still in the file's order:** the longest increasing run of their pages (by patience sorting, O(n log n)),
/// the same one every time for the same list.
pub fn in_order(own: &[u32]) -> Vec<bool> {
    let mut tails: Vec<usize> = Vec::new();
    let mut back: Vec<Option<usize>> = vec![None; own.len()];
    for (at, page) in own.iter().enumerate() {
        let spot = tails.partition_point(|tail| own[*tail] < *page);
        back[at] = spot.checked_sub(1).map(|before| tails[before]);
        if spot == tails.len() {
            tails.push(at);
        } else {
            tails[spot] = at;
        }
    }
    let mut kept = vec![false; own.len()];
    let mut at = tails.last().copied();
    while let Some(here) = at {
        kept[here] = true;
        at = back[here];
    }
    kept
}

/// **A file's highlight found again on its page in the new version:** the one at the same place in `/Annots` with the same `/Rect`, else the only one
/// with that `/Rect`, else none. `on_page` is the page's highlights there, as (place in `/Annots`, `/Rect`).
pub fn refind(key: FileKey, page: u32, on_page: &[(u32, Option<[f32; 4]>)]) -> Option<FileKey> {
    let same_rect = |rect: &Option<[f32; 4]>| rect.is_some_and(|rect| rect == key.rect);
    let index = match on_page.iter().find(|(index, rect)| *index == key.index && same_rect(rect)) {
        Some((index, _)) => *index,
        None => {
            let mut alike = on_page.iter().filter(|(_, rect)| same_rect(rect));
            match (alike.next(), alike.next()) {
                (Some((index, _)), None) => *index,
                _ => return None,
            }
        }
    };
    Some(FileKey { page, index, ..key })
}

/// A run of placed slots (moved pages and added ones) waiting for the fixed page it goes by: each slot, its position in the old list, and the verdicts
/// to mark if that page isn't found.
#[derive(Default)]
struct Run {
    slots: Vec<(PageSlot, usize)>,
    verdicts: Vec<usize>,
}

/// **Every edit's verdict, and the list on the new version if every one follows** (see the module's notes). `found` is the matcher's verdicts, by old
/// page ascending; `new_pages` the new version's length; `refound` each of the file's highlights taken out, on a page the document has, with where it is
/// found again ([`refind`]), or `None`.
pub fn rebase(edits: &PageEdits, found: &[(u32, Found)], new_pages: u32, refound: &[(FileKey, Option<FileKey>)]) -> Outcome {
    let file_pages = edits.file_pages as usize;
    let mut present = vec![false; file_pages];
    let mut own: Vec<u32> = Vec::new();
    for slot in edits.slots.iter().filter(|slot| slot.source == 0) {
        match present.get_mut(slot.page as usize) {
            Some(there) if !*there => *there = true,
            _ => return Outcome::held(Edit::Move { page: slot.page }, Held::Unsound),
        }
        own.push(slot.page);
    }
    let placed_count = edits.slots.len() - own.iter().zip(in_order(&own)).filter(|(_, fixed)| *fixed).count();
    if new_pages as usize + placed_count > MAX_PAGES {
        return Outcome::held(Edit::Move { page: 0 }, Held::TooLong);
    }
    let fixed = in_order(&own);
    let mut verdicts: Vec<EditMatch> = Vec::new();
    let verdict = |verdicts: &mut Vec<EditMatch>, edit: Edit, held: Option<Held>| {
        verdicts.push(EditMatch { edit, held });
        verdicts.len() - 1
    };
    // Pages taken out of the new version's own order: those the person deleted, and those they moved.
    let mut gone: HashSet<u32> = HashSet::new();
    for (page, there) in present.iter().enumerate() {
        if !there {
            let page = u32::try_from(page).unwrap_or(u32::MAX);
            let at = found_at(found, page);
            if let Ok((new, _)) = at {
                gone.insert(new);
            }
            verdict(&mut verdicts, Edit::Delete { page }, at.err());
        }
    }
    let mut turn_of: HashMap<u32, u8> = HashMap::new();
    // The new page each found fixed page is at, with its old position; and the runs that go before and after a fixed page.
    let mut fixed_at: HashMap<u32, usize> = HashMap::new();
    let mut before: HashMap<u32, Vec<(PageSlot, usize)>> = HashMap::new();
    let mut after: HashMap<u32, Vec<(PageSlot, usize)>> = HashMap::new();
    let mut anchor: Option<Result<u32, Held>> = None;
    let mut run = Run::default();
    let mut own_at = 0usize;
    let mut last_insert: Option<(usize, u16)> = None;
    // Ends the run waiting, placing it after `anchor` (or before `next`, for the run at the start), or marking it held.
    let mut close = |run: &mut Run, verdicts: &mut Vec<EditMatch>, to: Result<(u32, bool), Held>| {
        let run = std::mem::take(run);
        if run.slots.is_empty() {
            return;
        }
        match to {
            Ok((page, is_after)) => (if is_after { &mut after } else { &mut before }).entry(page).or_default().extend(run.slots),
            Err(_) => {
                for at in run.verdicts {
                    verdicts[at].held = verdicts[at].held.or(Some(Held::NoAnchor));
                }
            }
        }
    };
    for (position, slot) in edits.slots.iter().enumerate() {
        if slot.source == 0 {
            let is_fixed = fixed[own_at];
            own_at += 1;
            if is_fixed {
                let at = found_at(found, slot.page).map(|(new, _)| new);
                if slot.quarter_turns != 0 {
                    if let Ok(new) = at {
                        turn_of.insert(new, slot.quarter_turns);
                    }
                    verdict(&mut verdicts, Edit::Turn { page: slot.page }, at.err());
                }
                if let Ok(new) = at {
                    fixed_at.insert(new, position);
                }
                // A run before the first fixed page goes before it; one after a fixed page, after that one.
                match anchor {
                    None => close(&mut run, &mut verdicts, at.map(|new| (new, false))),
                    Some(previous) => close(&mut run, &mut verdicts, previous.map(|new| (new, true))),
                }
                anchor = Some(at);
                last_insert = None;
                continue;
            }
            let at = found_at(found, slot.page);
            if let Ok((new, _)) = at {
                gone.insert(new);
                run.slots.push((PageSlot { page: new, ..*slot }, position));
            }
            run.verdicts.push(verdict(&mut verdicts, Edit::Move { page: slot.page }, at.err()));
            last_insert = None;
        } else {
            run.slots.push((*slot, position));
            match last_insert {
                Some((at, source)) if source == slot.source => {
                    if let Edit::Insert { pages, .. } = &mut verdicts[at].edit {
                        *pages = pages.saturating_add(1);
                    }
                }
                _ => {
                    let at = verdict(&mut verdicts, Edit::Insert { source: slot.source, pages: 1 }, None);
                    run.verdicts.push(at);
                    last_insert = Some((at, slot.source));
                }
            }
        }
    }
    match anchor {
        Some(previous) => close(&mut run, &mut verdicts, previous.map(|new| (new, true))),
        None => close(&mut run, &mut verdicts, Err(Held::NoAnchor)),
    }
    // Highlights, and the file's taken out, on pages the document has.
    let mut marks: HashMap<u32, u32> = HashMap::new();
    for mark in edits.marks.iter().filter(|mark| mark.source == 0) {
        if !present.get(mark.page as usize).copied().unwrap_or(false) {
            continue;
        }
        let was = edits.identities.as_deref().and_then(|ids| ids.known(mark.page));
        let at = found_at(found, mark.page).and_then(|(new, now)| match was {
            Some(was) if was.frame == now.frame => Ok(new),
            Some(_) => Err(Held::FrameChanged),
            None => Err(Held::NotIdentified),
        });
        if let Ok(new) = at {
            marks.insert(mark.id, new);
        }
        verdict(&mut verdicts, Edit::Highlight { mark: mark.id }, at.err());
    }
    let mut hidden: Vec<FileKey> = Vec::new();
    for key in edits.marks.hidden() {
        if !present.get(key.page as usize).copied().unwrap_or(false) {
            continue;
        }
        let at = found_at(found, key.page).and_then(|_| {
            refound
                .iter()
                .find(|(was, _)| was == key)
                .and_then(|(_, now)| *now)
                .ok_or(Held::HighlightNotFound)
        });
        if let Ok(now) = at {
            hidden.push(now);
        }
        verdict(&mut verdicts, Edit::FileHighlightOut { page: key.page }, at.err());
    }
    if verdicts.iter().any(|edit| edit.held.is_some()) {
        return Outcome { edits: verdicts, onto: None };
    }
    let mut slots: Vec<PageSlot> = Vec::with_capacity(new_pages as usize + placed_count);
    let mut positions: Vec<Option<u32>> = vec![None; edits.slots.len()];
    let mut put = |slots: &mut Vec<PageSlot>, slot: PageSlot, from: Option<usize>| {
        if let Some(from) = from {
            positions[from] = u32::try_from(slots.len()).ok();
        }
        slots.push(slot);
    };
    for page in 0..new_pages {
        for (slot, from) in before.remove(&page).unwrap_or_default() {
            put(&mut slots, slot, Some(from));
        }
        if !gone.contains(&page) {
            let slot = PageSlot {
                source: 0,
                page,
                quarter_turns: turn_of.get(&page).copied().unwrap_or(0),
            };
            put(&mut slots, slot, fixed_at.get(&page).copied());
        }
        for (slot, from) in after.remove(&page).unwrap_or_default() {
            put(&mut slots, slot, Some(from));
        }
    }
    // Something placed by a page past the new version's end can't be: the matcher gives none, so this is a list that isn't sound.
    if !before.is_empty() || !after.is_empty() || slots.is_empty() {
        return Outcome::held(Edit::Move { page: 0 }, Held::Unsound);
    }
    let named = found.iter().filter_map(|(_, verdict)| verdict.page()).collect();
    Outcome {
        edits: verdicts,
        onto: Some(Onto {
            file_pages: new_pages,
            slots,
            marks,
            hidden,
            named,
            positions,
        }),
    }
}

/// **Puts a list onto the new version** [`rebase`] made it for: its pages, highlights and named pages are the new version's from here, and the undo
/// history ends here (owner, 2026-11-15: the edits stay, and ⌘Z doesn't go back past the change). The journal gets the whole list next. Says which
/// files kept for the history nothing needs now.
pub fn apply(edits: &mut PageEdits, onto: Onto) -> Vec<Box<Path>> {
    let Onto {
        file_pages,
        slots,
        marks,
        hidden,
        named,
        positions: _,
    } = onto;
    edits.file_pages = file_pages as usize;
    edits.slots = slots;
    edits.marks.onto(|mark| marks.get(&mark.id).copied(), hidden);
    edits.changes += 1;
    super::identity::onto(edits, named);
    let let_go = edits.drop_history();
    edits.differs = edits.differs_from_the_file();
    edits.rewrites = edits.rewrites_the_file();
    super::marks::settle(edits);
    let_go
}

#[cfg(test)]
mod table;
