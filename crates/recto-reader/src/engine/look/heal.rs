//! **A look kept for want of an answer is asked again, once the Mac may be quiet** ([`heal`]). A page whose image checks stayed busy through [`LOOK_TRIES`](super::LOOK_TRIES) takes [`PageLook::undecided`](crate::paper::PageLook::undecided), which dims the whole page, and a dark picture sheet stays dimmed for good: it's decided from no answer, so it isn't asked of a Mac that's quiet. [`Looks`] holds when each such page is next asked ([`HEAL_AFTER`] after it was kept, or after the last busy ask of its file), and the thread that draws asks at rest ([`at_rest`]), for pages in view of the tab showing on dark paper, and wakes by then ([`next_heal`]).
//!
//! **One owner, the engine:** the host answers each ask honestly (`Places::NotNow` for a check the Mac was too busy for) and keeps nothing for it. A draw can't reset this clock, as one resets the host's count, since nothing here counts: while the Mac stays busy the file is asked once per [`HEAL_AFTER`], whatever is drawn between. A look decided from answers is never healed again, so a page changes at most once, and every tile of it goes by the one look until then (`page_look` returns the kept look as before).

use super::{HEAL_AFTER, ImagesAsked, LookSource, Looks};
use crate::engine::run::{OpenDoc, as_an_added_files, drawable_by, job_ended_with, seen_replaced};
use crate::engine::{Shared, lock, signal_ops};
use crate::paper::PageLook;
use crate::tiles::Outcome;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What [`heal`] came to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Healed {
    /// Nothing was asked: the page isn't kept for want of an answer, or its time isn't yet, or the file's worker isn't running (it's never started for this, and the page's next ask is [`HEAL_AFTER`] on).
    NotDue,
    /// The Mac was too busy to say: every healing page of the file waits for its next time, and a step made at most this one walk.
    Busy,
    /// The job went stale on the way: the clock is as it was.
    Stale,
    /// The images were told. The look that was kept is lifted out (it's this), and the page has none until the preview drawn for it decides it from that picture ([`Looks::put_back`] if that draw doesn't land).
    Asked(Arc<PageLook>),
}

/// **Asks a page kept for want of an answer again, if its time has come** at `now`, and only of a worker that's running (a heal never starts one, so an idle file costs nothing). A busy answer moves every healing page of the file to `now + HEAL_AFTER`, so while the Mac stays busy one walk a file is made per [`HEAL_AFTER`], whatever is drawn between and however many pages wait. **It draws nothing:** no small picture is drawn for the look, which is decided from the page's preview, the one picture the swap draws anyway (`at_rest`), so a heal holds the thread for at most the one walk's deadline.
pub(super) fn heal(doc: &mut impl LookSource, looks: &mut Looks, page: u32, generation: u64, now: Instant) -> Healed {
    if doc.take_image_places_cleared() {
        // What was decided from the places is no longer to stand: asked again as any page is, by its next draw.
        looks.clear();
    }
    if !looks.due(page, now) {
        return Healed::NotDue;
    }
    let Some(kept) = looks.decided.get(&page).cloned() else {
        looks.healing.remove(&page);
        return Healed::NotDue;
    };
    if !doc.worker_alive() {
        looks.push_back(page, now);
        return Healed::NotDue;
    }
    let asked = doc.image_boxes(page, generation);
    if doc.is_stale(generation) {
        return Healed::Stale;
    }
    if asked == ImagesAsked::Busy {
        for due in looks.healing.values_mut() {
            *due = now + HEAL_AFTER;
        }
        return Healed::Busy;
    }
    // The answer is the host's to keep; the look is decided by the draw that comes next.
    looks.decided.remove(&page);
    looks.healing.remove(&page);
    looks.tries.remove(&page);
    Healed::Asked(kept)
}

/// How long till the soonest page in `pages` kept for want of an answer is asked again, `Duration::ZERO` if one's due; `None` if none waits or no renderer is open for them (`looks_of` gives a file's looks, if one is). The thread that draws wakes by then.
pub(in crate::engine) fn next_heal<'a>(pages: &[(PathBuf, u32)], looks_of: impl Fn(&Path) -> Option<&'a Looks>, now: Instant) -> Option<Duration> {
    pages
        .iter()
        .filter_map(|(file, page)| looks_of(file)?.healing.get(page))
        .map(|due| due.saturating_duration_since(now))
        .min()
}

/// **One step of healing, between the thread's turns:** for one due page in view of the tab showing on dark paper, by the renderer already open for its file with its worker running (neither is started for this), asked again, and if the images are told, its preview drawn again by the new look and swapped in. Says whether it did anything that makes the thread come round again; a step makes at most one walk.
///
/// In order, before anything is asked: a preview must be at hand to draw (zoomed out past one, or no surface, and the page's next ask is pushed back by [`HEAL_AFTER`]), and the job must be one the ordinary path would draw (`drawable_by`: a withheld tab's pages, a changed added file, a renderer keyed to another tab). Refused, the old look's tiles are dropped and nothing is drawn.
///
/// The swap: the preview is drawn by `do_job`, which decides the look from that picture, and when it's drawn (and only then) every drawn thing of that file page on dark paper in every held tab is dropped under one hold of the page view's lock as it's put on the bare page, so the dimmed page turns into its real look in one step: no blank frame and never two looks at once (`tiles/relook.rs`). A draw that didn't land leaves the old tiles and the old look, asked again after [`HEAL_AFTER`].
///
/// **The bound:** a walk made here can't be cancelled: while the Mac stays starved it holds this thread for one walk's deadline, at most once per [`HEAL_AFTER`] per file, and when the Mac is quiet it takes milliseconds. The preview is an ordinary draw of the page.
pub(in crate::engine) fn at_rest(shared: &Shared, renderers: &mut HashMap<PathBuf, OpenDoc>) -> bool {
    let wanted = crate::tiles::relook::on_dark_paper(&mut lock(&shared.reader));
    let Some((pages, generation)) = wanted else { return false };
    let now = Instant::now();
    for (file, page) in pages {
        let Some(open) = renderers.get_mut(&file) else { continue };
        if !open.looks.due(page, now) {
            continue;
        }
        let planned = crate::tiles::relook::stand_in_plan(&mut lock(&shared.reader), &file, page);
        let Some(job) = planned.and_then(crate::tiles::JobPlan::ready) else {
            open.looks.push_back(page, now);
            continue;
        };
        let Some(own) = drawable_by(shared, open, &job) else {
            // What the ordinary path refuses isn't drawn from the open file, nor asked of it: what was drawn of the page goes, as it was a look the refused page can't be shown by.
            open.looks.push_back(page, now);
            let mut reader = lock(&shared.reader);
            crate::tiles::relook::bare(&mut reader, &file, page);
            let has_ops = reader.host.has_ops();
            drop(reader);
            told(shared, has_ops);
            return true;
        };
        let healed = heal(&mut open.doc, &mut open.looks, page, generation, now);
        if !ends_the_step(&healed) {
            continue;
        }
        // Replaced while it was asked: the look may be of either file, and none of it is kept or swapped. The next job takes the replacement up.
        if let Healed::Asked(kept) = healed
            && !seen_replaced(open)
        {
            draw_anew(shared, open, job, own, kept, now);
        }
        return true;
    }
    false
}

/// Whether a step of healing ends with this: a walk was made (busy, stale or told), and one step makes at most one, as a walk can hold the thread for a page deadline.
pub(super) fn ends_the_step(healed: &Healed) -> bool {
    !matches!(healed, Healed::NotDue)
}

/// A page's images were told: its preview drawn by the new look and swapped in with the old tiles' going, as [`at_rest`] says, or the old look kept as it was if the draw didn't land.
fn draw_anew(shared: &Shared, open: &mut OpenDoc, job: crate::tiles::Job, own: bool, kept: Arc<PageLook>, now: Instant) {
    {
        let mut reader = lock(&shared.reader);
        reader.work.note_started(&job, None);
    }
    let outcome = crate::engine::run::job_on(shared, open, &job, own);
    let outcome = if own { outcome } else { as_an_added_files(outcome, &job) };
    landed(shared, open, job, kept, outcome, now);
}

/// What the preview's draw came to: on [`Outcome::Drawn`] (and only then) the old tiles go as it lands, else nothing is dropped and the kept look is kept again, asked after [`HEAL_AFTER`] (`LookLater`, a failed page, a cancelled job alike: the page is still the old look's).
pub(super) fn landed(shared: &Shared, open: &mut OpenDoc, job: crate::tiles::Job, kept: Arc<PageLook>, outcome: Outcome, now: Instant) {
    if seen_replaced(open) || open.replaced {
        // Another file's picture: nothing is dropped or put, and the job ends as cancelled. The next job takes the replacement up (`open.replaced`).
        job_ended_with(shared, job, Outcome::Cancelled, false, |_| {});
        return;
    }
    let (file, source_page) = (job.file.clone(), job.source_page);
    if matches!(outcome, Outcome::Drawn { .. }) {
        job_ended_with(shared, job, outcome, false, |reader| {
            crate::tiles::relook::drop_look(reader, &file, source_page)
        });
    } else {
        open.looks.put_back(source_page, kept, now);
        job_ended_with(shared, job, outcome, false, |_| {});
    }
}

/// What's been put on the host's queue is signalled, and whoever waits for the view is woken.
fn told(shared: &Shared, has_ops: bool) {
    if has_ops {
        signal_ops(shared);
    }
    shared.settled.notify_all();
}
