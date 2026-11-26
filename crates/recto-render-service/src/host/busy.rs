//! **A start or an open the Mac was too busy for.** Like a draw it says nothing about the page or the file, so it isn't remembered and whoever asked asks again; a worker that hangs looks the same, so each kind is bounded by one number, [`MAX_BUSY`] in a row. Starts (a worker that doesn't say hello) and opens (one that does, but doesn't open the file) have a run each, and past it the failure is real: a start becomes `Unavailable`, which is remembered for `open_retry_pause` (so asks inside it fail at once instead of each waiting out another start) and isn't counted as the Mac being busy, so nobody who watches the count takes it for a busy Mac; an open is the file's own failure. A run ends when a worker says hello (starts) or opens the file (opens), and the starts' run also when the remembered `Unavailable` runs out or `forget_failures` is asked.
//!
//! A draw or a read of a page the Mac was too busy to run the worker for is held apart the same way, and a page's busy draws, or busy reads, in a row are bounded by [`MAX_BUSY`] too.

use super::{DocRenderer, JobFailure, KillReason, RenderFailure, TileResult, other_result};

/// Busy reads, or busy draws, of a page in a row past which it's remembered as failed after all: a worker that hangs looks starved too, and this bounds what a hostile page can cost to that many deadlines.
pub const MAX_BUSY: u8 = 3;

/// What `too_busy` was before a job was asked of the worker. Taken with [`before`], and the only thing [`draw_stopped`] and [`size_stopped`] take to know a kill was a starved one, so a caller can't leave it out: a stopped worker is told from a starved one by the count moving past it.
#[derive(Clone, Copy)]
pub(super) struct Before(u32);

impl Before {
    /// Whether a worker was stopped for lack of the processor since this was taken.
    pub(super) fn moved(self, doc: &DocRenderer) -> bool {
        doc.stats.too_busy > self.0
    }
}

/// The count to hold before a job whose stop is to go through [`draw_stopped`] or [`size_stopped`].
pub(super) fn before(doc: &DocRenderer) -> Before {
    Before(doc.stats.too_busy)
}

/// Whether a job's worker was stopped for lack of the processor rather than for anything of the page, and the page's run of such stops is still short of [`MAX_BUSY`]. Counts the stop. Not remembered, so whoever asked asks again; nothing's retried here, as that would hold this thread for another deadline while the one who asked can lose interest.
fn starved(doc: &mut DocRenderer, page: u32, reason: KillReason, before: Before) -> bool {
    if !matches!(reason, KillReason::Deadline) || !before.moved(doc) {
        return false;
    }
    let memory = doc.remember(page);
    memory.busy_draws = memory.busy_draws.saturating_add(1);
    memory.busy_draws < MAX_BUSY
}

/// Whether a walk over a page's images (dark paper's or a click's) failed because the Mac was too busy to run its worker: a start it couldn't make in time, or a kill at the deadline with the processor not given ([`starved`]). Such a walk is not an answer: nothing's remembered, so whoever asked asks again, until [`MAX_BUSY`] in a row, after which it's the failure it looked like.
pub(super) fn walk_stopped<T>(doc: &mut DocRenderer, page: u32, reply: &Result<T, JobFailure>, before: Before) -> bool {
    match reply {
        Err(JobFailure::Failed(RenderFailure::TooBusyToStart)) => true,
        Err(JobFailure::Stopped(reason)) => starved(doc, page, *reason, before),
        _ => false,
    }
}

/// A draw's, or what asks about a page the way a draw does (its traits, the reading of its contents), worker was stopped for `reason`. One that ran out of time with the Mac too busy to give its worker the processor isn't remembered ([`RenderFailure::TooBusy`]); past [`MAX_BUSY`] in a row it's remembered after all, at every scale.
pub(super) fn draw_stopped(doc: &mut DocRenderer, page: u32, from_scale: f32, reason: KillReason, before: Before) -> RenderFailure {
    if starved(doc, page, reason, before) {
        return RenderFailure::TooBusy { page };
    }
    doc.give_up_drawing(page, from_scale, reason)
}

/// The same for a page's size, which is remembered in its own place.
pub(super) fn size_stopped(doc: &mut DocRenderer, page: u32, reason: KillReason, before: Before) -> RenderFailure {
    if starved(doc, page, reason, before) {
        return RenderFailure::TooBusy { page };
    }
    doc.remember(page).size = Some(reason);
    RenderFailure::PageReadFailed { page, reason }
}

/// A draw that landed ends a run of busy draws.
pub(super) fn landed(doc: &mut DocRenderer, page: u32) {
    if let Some(memory) = doc.remembered.iter_mut().find(|memory| memory.page == page) {
        memory.busy_draws = 0;
    }
}

/// A job that landed with `read` ends a run of busy draws.
pub(super) fn landed_with<T>(doc: &mut DocRenderer, page: u32, read: T) -> T {
    landed(doc, page);
    read
}

/// A draw's result for the caller. One that landed ends a run of busy draws.
pub(super) fn drew(doc: &mut DocRenderer, page: u32, result: Result<TileResult, JobFailure>) -> Result<TileResult, RenderFailure> {
    if matches!(result, Ok(TileResult::Done(_))) {
        landed(doc, page);
    }
    other_result(result)
}

/// A read of a page's text ran out of time with the Mac too busy to give its worker the processor: not remembered, until [`MAX_BUSY`] in a row. The failure to give back either way.
pub(super) fn read_stopped(doc: &mut DocRenderer, page: u32, reason: KillReason) -> RenderFailure {
    let memory = doc.remember(page);
    memory.busy_reads = memory.busy_reads.saturating_add(1);
    if memory.busy_reads >= MAX_BUSY {
        memory.text = Some(reason);
    }
    RenderFailure::PageReadFailed { page, reason }
}

/// A read that landed ends a run of busy reads.
pub(super) fn read_landed<T>(doc: &mut DocRenderer, page: u32, read: T) -> T {
    if let Some(memory) = doc.remembered.iter_mut().find(|memory| memory.page == page) {
        memory.busy_reads = 0;
    }
    read
}

/// Busy starts in a row ([`start_stopped`]) and busy opens in a row ([`open_stopped`]), each bounded by [`MAX_BUSY`]. A worker that said hello ends the starts ([`started`]), and so does the next start after the remembered start failure has run out, or `forget_failures`; one that opened the file ends the opens ([`opened`]). At the bound a start is `Unavailable`, remembered for a moment and not counted as busy; an open is its own failure, counted as the file's.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Runs {
    pub(super) starts: u8,
    pub(super) opens: u8,
    /// When the remembered start failure runs out: the next start is asked of the Mac afresh, with its busy grace again.
    remembered_until: Option<std::time::Instant>,
}

/// One more busy start or open in a run: whether the run is still short of [`MAX_BUSY`].
fn within_bound(run: &mut u8) -> bool {
    *run = run.saturating_add(1);
    *run < MAX_BUSY
}

/// A worker didn't say hello in time, with the Mac too busy to give it the processor: [`RenderFailure::TooBusyToStart`], counted as the Mac being busy and not remembered, so whoever asked asks again. A worker that hangs looks the same, so past [`MAX_BUSY`] in a row it's `Unavailable` after all: not counted as busy (so nobody who watches the count takes it for a busy Mac) and remembered for a moment, as an open past the bound is, so asks within `open_retry_pause` fail at once instead of each waiting out another start.
pub(super) fn start_stopped(doc: &mut DocRenderer) -> RenderFailure {
    // No file has reached the worker, so what kept it from saying hello isn't the file's doing.
    let runs = &mut doc.left_out.busy_runs;
    if runs.remembered_until.take_if(|at| std::time::Instant::now() >= *at).is_some() {
        runs.starts = 0;
    }
    if within_bound(&mut doc.left_out.busy_runs.starts) {
        doc.stats.too_busy += 1;
        RenderFailure::TooBusyToStart
    } else {
        doc.note_open_failure(RenderFailure::Unavailable);
        doc.left_out.busy_runs.remembered_until = Some(std::time::Instant::now() + doc.policy.open_retry_pause);
        RenderFailure::Unavailable
    }
}

/// A worker that said hello ends the run of busy starts: it started, whatever came of the file after.
pub(super) fn started(doc: &mut DocRenderer) {
    doc.left_out.busy_runs.starts = 0;
}

/// A worker that opened the file ends the run of busy opens.
pub(super) fn opened(doc: &mut DocRenderer) {
    doc.left_out.busy_runs.opens = 0;
}

/// An open's worker was stopped for `reason`, which `failed` says. One that had no processor, `starved`, isn't the file's doing: it counts as the Mac being busy and not towards stopping the document, and is [`RenderFailure::TooBusyToStart`] until [`MAX_BUSY`] opens in a row, counted apart from starts, when it's `failed` after all, and counted as the file's.
pub(super) fn open_stopped(doc: &mut DocRenderer, failed: RenderFailure, starved: bool) -> RenderFailure {
    if !starved {
        return failed;
    }
    if within_bound(&mut doc.left_out.busy_runs.opens) {
        doc.left_out.busy = doc.left_out.busy.saturating_add(1);
        doc.stats.too_busy += 1;
        RenderFailure::TooBusyToStart
    } else {
        // Past the bound it's the file's, as `failed` says: it counts towards stopping the document and isn't a pause.
        failed
    }
}
