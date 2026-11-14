//! **The file's highlights a tab has taken out and not saved, from the host's side** (review of R4.3, S2). A worker holds one set at a time, for the
//! document it has open; a renderer can draw for more than one tab (the same file is one tab's own and another's added pages), so each job says
//! whose view it draws ([`HiddenWanted::draw_for`]), and the set goes to the worker ahead of the job, on the same channel, only when it's another
//! than the worker holds: a tab and generation, or none. A worker started afresh holds none, so it's sent again after a restart.
//!
//! **What it costs.** A renderer with no set holds `None`, and a worker one key: nothing is sent while both are none.

use super::{KillReason, Stop, Watch, Worker};
use recto_proto::Kind;
use recto_proto::hidden::{HiddenAnnots, HiddenTaken};
use std::sync::Arc;

/// The set the next job draws with, and what a worker last said of one.
#[derive(Debug, Default)]
pub struct HiddenWanted {
    /// The tab the next job is for, and its set; `None` for none.
    want: Option<(u32, Arc<HiddenAnnots>)>,
    /// How many of the last set's highlights were for pages the document hasn't.
    unmatched: u32,
}

impl HiddenWanted {
    /// **Whose view the next jobs draw:** `tab`'s, with its set, or none. Nothing is sent here: the set goes ahead of the next job, if the worker
    /// holds another.
    pub fn draw_for(&mut self, tab: u32, set: Option<Arc<HiddenAnnots>>) {
        self.want = set.filter(|set| !set.is_empty()).map(|set| (tab, set));
    }

    /// How many of the last set's highlights the worker said were for pages the document hasn't.
    pub fn unmatched(&self) -> u32 {
        self.unmatched
    }

    fn key(&self) -> Option<(u32, u64)> {
        self.want.as_ref().map(|(tab, set)| (*tab, set.generation))
    }
}

/// Brings a worker's set to the one wanted, ahead of a job. A worker that answers with anything but the set it was sent is stopped, as for any
/// answer that isn't one.
pub(super) fn bring(worker: &mut Worker, wanted: &mut HiddenWanted, watch: &Watch<'_>) -> Result<(), Stop> {
    let key = wanted.key();
    if worker.hidden_sent == key {
        return Ok(());
    }
    let none = HiddenAnnots::default();
    let set = wanted.want.as_ref().map_or(&none, |(_, set)| set.as_ref());
    let frame = set.encode().map_err(|error| Stop::Killed(KillReason::Rejected(error.0)))?;
    let reply = worker.exchange(&frame, &[], watch)?;
    let taken = match reply.kind {
        Kind::HiddenTaken => HiddenTaken::decode(&reply.payload).map_err(|error| Stop::Killed(KillReason::Rejected(error.0)))?,
        _ => return Err(Stop::Killed(KillReason::Rejected("the wrong kind of reply to a hidden set"))),
    };
    if taken.generation != set.generation || taken.unmatched as usize > set.len() {
        return Err(Stop::Killed(KillReason::Rejected("a hidden set's answer for another set")));
    }
    wanted.unmatched = taken.unmatched;
    worker.hidden_sent = key;
    Ok(())
}
