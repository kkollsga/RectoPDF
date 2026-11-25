//! **A draw the Mac was too busy to run the worker for.** Like a read of a page's text, it says nothing about the page, so it isn't remembered and the one who asked asks again; a worker that hangs looks the same, so a page's busy draws, or busy reads, in a row are bounded by one number.

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
