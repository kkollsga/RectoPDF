//! **A look kept for want of an answer is asked again, once the Mac may be quiet** ([`heal`]). A page whose image checks stayed busy through [`LOOK_TRIES`](super::LOOK_TRIES) takes [`PageLook::undecided`](crate::paper::PageLook::undecided), which dims the whole page, and a dark picture sheet stays dimmed for good: it's decided from no answer, so it isn't asked of a Mac that's quiet. [`Looks`] holds when each such page is next asked ([`HEAL_AFTER`] after it was kept, or after the last busy ask of its file), and the thread that draws asks at rest ([`at_rest`]), for pages in view of the tab showing on dark paper, and wakes by then ([`next_heal`]).
//!
//! **One owner, the engine:** the host answers each ask honestly (`Places::NotNow` for a check the Mac was too busy for) and keeps nothing for it. A draw can't reset this clock, as one resets the host's count, since nothing here counts: while the Mac stays busy the file is asked once per [`HEAL_AFTER`], whatever is drawn between. A look decided from answers is never healed again, so a page changes at most once, and every tile of it goes by the one look until then (`page_look` returns the kept look as before).

use super::{HEAL_AFTER, ImagesAsked, LookSource, Looks, NoLook, decided_from};
use crate::engine::run::{OpenDoc, job_ended_with, seen_replaced};
use crate::engine::{Shared, lock, signal_ops};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What [`heal`] came to.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::engine) enum Healed {
    /// The page isn't kept for want of an answer, or its time isn't yet: nothing was asked. Also a busy ask, after which the file's pages wait for their next time.
    NotDue,
    /// The job went stale on the way: the clock is as it was.
    Stale,
    /// Decided, and the same as what was kept (or a try that failed for a reason that may pass, asked again after [`HEAL_AFTER`]).
    Same,
    /// Decided, and not what was kept: every tile of the page drawn so far is of the old look.
    Changed,
}

/// **Asks a page kept for want of an answer again, if its time has come** at `now`, and decides it as `page_look` does when the answer is there. A busy answer moves every healing page of the file to `now + HEAL_AFTER`, so while the Mac stays busy one walk a file is made per [`HEAL_AFTER`], whatever is drawn between and however many pages wait. A picture that fails for a reason that may pass is tried again after [`HEAL_AFTER`] too, and one that can never be had ends as [`PageLook::undecided`](crate::paper::PageLook::undecided) for a page with no images.
pub(in crate::engine) fn heal(doc: &mut impl LookSource, looks: &mut Looks, page: u32, generation: u64, now: Instant) -> Healed {
    if doc.take_image_places_cleared() {
        // What was decided from the places is no longer to stand: asked again as any page is, by its next draw.
        looks.clear();
    }
    if !looks.healing.get(&page).is_some_and(|&due| now >= due) {
        return Healed::NotDue;
    }
    let Some(kept) = looks.decided.get(&page).cloned() else {
        looks.healing.remove(&page);
        return Healed::NotDue;
    };
    let asked = doc.image_boxes(page, generation);
    if doc.is_stale(generation) {
        return Healed::Stale;
    }
    if asked == ImagesAsked::Busy {
        for due in looks.healing.values_mut() {
            *due = now + HEAL_AFTER;
        }
        return Healed::NotDue;
    }
    match decided_from(doc, looks, page, generation, asked, None, None, now) {
        Ok(look) => {
            if *look == *kept {
                Healed::Same
            } else {
                Healed::Changed
            }
        }
        Err(NoLook::Stale) => Healed::Stale,
        // The picture failed for a reason that may pass: its tries are the look's own, spaced by [`HEAL_AFTER`] here.
        Err(NoLook::Again) => {
            looks.healing.insert(page, now + HEAL_AFTER);
            Healed::Same
        }
    }
}

/// How long till the soonest page in `pages` kept for want of an answer is asked again, `Duration::ZERO` if one's due; `None` if none waits or no renderer is open for them (`looks_of` gives a file's looks, if one is). The thread that draws wakes by then.
pub(in crate::engine) fn next_heal<'a>(pages: &[(PathBuf, u32)], looks_of: impl Fn(&Path) -> Option<&'a Looks>, now: Instant) -> Option<Duration> {
    pages
        .iter()
        .filter_map(|(file, page)| looks_of(file)?.healing.get(page))
        .map(|due| due.saturating_duration_since(now))
        .min()
}

/// **One step of healing, between the thread's turns:** for one due page in view of the tab showing on dark paper, by the renderer already open for its file (none is opened for this), asked again, and if its look changed, drawn again once. Says whether it did anything that makes the thread come round again.
///
/// The swap, when a look changed: the page's preview is drawn again by the new look (`do_job`, which makes no walk), and then, under one hold of the page view's lock, every drawn thing of that file page on dark paper in every held tab is dropped and the preview put on the bare page. So the dimmed page turns into its real look in one step: no blank frame and never two looks at once (`tiles/relook.rs`). Where no preview is possible (zoomed out past one, or no surface) the tiles are just dropped, and drawn again by the new look.
///
/// A walk made here can't be cancelled: while the Mac stays starved it can hold this thread for a page deadline, at most once per [`HEAL_AFTER`] per file, and when the Mac is quiet it takes milliseconds.
pub(in crate::engine) fn at_rest(shared: &Shared, renderers: &mut HashMap<PathBuf, OpenDoc>) -> bool {
    let wanted = crate::tiles::relook::on_dark_paper(&mut lock(&shared.reader));
    let Some((pages, generation)) = wanted else { return false };
    let now = Instant::now();
    for (file, page) in pages {
        let Some(open) = renderers.get_mut(&file) else { continue };
        if !open.looks.healing.contains_key(&page) {
            continue;
        }
        let healed = heal(&mut open.doc, &mut open.looks, page, generation, now);
        // Replaced while it was asked: the look may be of either file, and none of it is kept or swapped. The next job takes the replacement up.
        if seen_replaced(open) {
            return true;
        }
        match healed {
            Healed::NotDue => {}
            Healed::Stale | Healed::Same => return true,
            Healed::Changed => {
                swap(shared, renderers, &file, page);
                return true;
            }
        }
    }
    false
}

/// A page's look changed: its preview drawn by the new look and swapped in with the old tiles' going, as [`at_rest`] says.
fn swap(shared: &Shared, renderers: &mut HashMap<PathBuf, OpenDoc>, file: &PathBuf, page: u32) {
    let planned = crate::tiles::relook::stand_in_plan(&mut lock(&shared.reader), file, page);
    let job = planned.and_then(crate::tiles::JobPlan::ready);
    let (Some(job), Some(open)) = (job, renderers.get_mut(file)) else {
        // No preview to put there: what was drawn of it goes, and its tiles are drawn again as the plan comes to them.
        let mut reader = lock(&shared.reader);
        crate::tiles::relook::bare(&mut reader, file, page);
        let has_ops = reader.host.has_ops();
        drop(reader);
        told(shared, has_ops);
        return;
    };
    {
        let mut reader = lock(&shared.reader);
        reader.work.note_started(&job, None);
    }
    lock(&shared.reader).work.set_cancel(open.doc.cancel_handle());
    let outcome = crate::engine::run::do_job(open, &job);
    if seen_replaced(open) || open.replaced {
        // Another file's picture: nothing is dropped or put, and the job ends as cancelled. The next job takes the replacement up (`open.replaced`).
        job_ended_with(shared, job, crate::tiles::Outcome::Cancelled, false, |_| {});
        return;
    }
    let (file, source_page) = (job.file.clone(), job.source_page);
    job_ended_with(shared, job, outcome, false, |reader| {
        crate::tiles::relook::drop_look(reader, &file, source_page)
    });
}

/// What's been put on the host's queue is signalled, and whoever waits for the view is woken.
fn told(shared: &Shared, has_ops: bool) {
    if has_ops {
        signal_ops(shared);
    }
    shared.settled.notify_all();
}
