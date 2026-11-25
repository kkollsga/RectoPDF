//! **A draw the Mac was too busy to run the worker for.** Like a read of a page's text, it says nothing about the page, so it isn't remembered and the one who asked asks again; a worker that hangs looks the same, so a page's busy draws, or busy reads, in a row are bounded by one number.

use super::{DocRenderer, JobFailure, KillReason, RenderFailure, TileResult, other_result};

/// Busy reads, or busy draws, of a page in a row past which it's remembered as failed after all: a worker that hangs looks starved too, and this bounds what a hostile page can cost to that many deadlines.
pub(super) const MAX_BUSY: u8 = 3;

/// A draw's worker was stopped for `reason`; `busy` is `too_busy` from before it. One that ran out of time with the Mac too busy to give its worker the processor isn't remembered ([`RenderFailure::TooBusy`]), and whoever asked asks again, as a read is (`read_characters`). Past [`MAX_BUSY`] in a row it's remembered after all. Nothing's retried here: that would hold this thread for another deadline, and the one who asked can lose interest meanwhile.
pub(super) fn draw_stopped(doc: &mut DocRenderer, page: u32, from_scale: f32, reason: KillReason, busy: u32) -> RenderFailure {
    if matches!(reason, KillReason::Deadline) && doc.stats.too_busy > busy {
        let memory = doc.remember(page);
        memory.busy_draws = memory.busy_draws.saturating_add(1);
        if memory.busy_draws < MAX_BUSY {
            return RenderFailure::TooBusy { page };
        }
    }
    doc.give_up_drawing(page, from_scale, reason)
}

/// A draw's result for the caller. One that landed ends a run of busy draws.
pub(super) fn drew(doc: &mut DocRenderer, page: u32, result: Result<TileResult, JobFailure>) -> Result<TileResult, RenderFailure> {
    if matches!(result, Ok(TileResult::Done(_)))
        && let Some(memory) = doc.remembered.iter_mut().find(|memory| memory.page == page)
    {
        memory.busy_draws = 0;
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
