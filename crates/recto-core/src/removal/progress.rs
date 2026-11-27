//! Walking a `Plan`: one acknowledgment per step, and what a crash at any point means.

use super::*;

/// What the carrier reports for the step it just did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    Done,
    /// Finished but not known to be on disk. On a durable step this is a failure.
    DoneNotDurable,
    Failed,
}

/// What a person may be told, decided only by the acknowledged prefix and never by a guess.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The barrier is not acknowledged: the source and history are unchanged, and cancelling is safe.
    /// A candidate file may have been written; it is not the source or its history.
    Unchanged,
    /// Old history is being retired; only forward completion is possible.
    HistoryRetiring,
    /// The point of no return passed but the sanitized file is not known to be installed: an
    /// install that was not read back is in doubt too, since the read-back is what shows it.
    ReplacementInDoubt,
    /// Installed and read back; the session and leases still need cleaning up. Lasts until the last step is
    /// acknowledged, so a failed invalidation, lease release or barrier retirement stays here.
    SanitizedCleanupPending,
    /// Every step of the plan is acknowledged.
    Complete,
    /// Clearing leftovers only: at least one act has run (a save moved to the Trash, a copy deleted, or one that failed part way), so it is never
    /// "nothing changed", and not every step is acknowledged. What is left is found by looking again.
    PartlyCleared,
}

impl Outcome {
    pub const fn nothing_written(self) -> bool {
        matches!(self, Outcome::Unchanged)
    }

    pub const fn may_cancel(self) -> bool {
        matches!(self, Outcome::Unchanged)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderError {
    /// The progress was made for another plan: its digest differs.
    WrongPlan,
    /// An acknowledgment for a step other than the next one.
    OutOfOrder,
    /// An earlier failure stopped the walk; resume from a new `Progress`.
    Stopped,
    Finished,
    /// The barrier is not acknowledged, so there is nothing to resume.
    NotResumable,
}

/// How many steps are acknowledged, as the only record of where the transaction is, bound to one plan by its digest. It starts only
/// from a `Confirmed`, which a confirmation of exactly that plan makes. Not `Clone`: one confirmation is one walk, and a copy could
/// be walked twice.
#[derive(Debug, PartialEq, Eq)]
pub struct Progress {
    digest: PlanDigest,
    acked: u32,
    stopped: bool,
    /// Leftovers only: an act has run, or may have part way. Never reset.
    acted: bool,
}

/// Where a transaction record says a walk stood: the plan's digest and the acknowledged count. Only decoding a record makes one (`removal::barrier`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    digest: PlanDigest,
    acked: u32,
}

impl Checkpoint {
    pub(super) const fn new(digest: PlanDigest, acked: u32) -> Self {
        Self { digest, acked }
    }

    pub const fn acked(&self) -> u32 {
        self.acked
    }
}

/// The index of the first step with `effect`. Every plan contains each of the markers once.
pub(super) fn index_of(plan: &Plan, effect: Effect) -> u32 {
    plan.steps.iter().position(|s| s.effect == effect).map_or(u32::MAX, |i| i as u32)
}

impl Progress {
    /// Begins the walk of `plan`, consuming the confirmation of it. A confirmation of another plan is `WrongPlan`. A stopped leftovers walk is
    /// never offered again: the caller surveys again first, and the person confirms what is left.
    pub fn start(plan: &Plan, confirmed: Confirmed) -> Result<Self, OrderError> {
        if confirmed.digest() != plan.digest {
            return Err(OrderError::WrongPlan);
        }
        Ok(Self {
            digest: plan.digest,
            acked: 0,
            stopped: false,
            acted: false,
        })
    }

    /// Continue from a decoded transaction record, only once the barrier is in its acknowledged prefix.
    ///
    /// Trust boundary: resume is not a confirmation. A `Checkpoint` is made only by decoding a record the app itself wrote after a confirmed walk
    /// passed the barrier (`barrier::decode_barrier`), never from a request, an agent or a file the app didn't write. This function only
    /// checks that it fits the plan; it cannot tell where it came from.
    pub fn resume(plan: &Plan, checkpoint: &Checkpoint) -> Result<Self, OrderError> {
        if checkpoint.digest != plan.digest {
            return Err(OrderError::WrongPlan);
        }
        // Nothing in a leftovers plan is a point of no return that a record could hold: the next run looks again and the person confirms what remains.
        if plan.request.scope() == Scope::Leftovers {
            return Err(OrderError::NotResumable);
        }
        if checkpoint.acked as usize > plan.steps.len() || checkpoint.acked <= index_of(plan, Effect::PersistBarrier) {
            return Err(OrderError::NotResumable);
        }
        Ok(Self::at(checkpoint))
    }

    /// The progress a checkpoint stands at, without the resume checks: for reading an outcome from a record that is not yet resumable.
    pub(super) fn at(checkpoint: &Checkpoint) -> Self {
        Self {
            digest: checkpoint.digest,
            acked: checkpoint.acked,
            stopped: false,
            acted: false,
        }
    }

    pub(super) fn digest(&self) -> PlanDigest {
        self.digest
    }

    /// The step to do next, or none when finished, stopped or made for another plan.
    pub fn next(&self, plan: &Plan) -> Option<Step> {
        if self.stopped || self.digest != plan.digest {
            return None;
        }
        plan.steps.get(self.acked as usize).copied()
    }

    pub fn acknowledge(&mut self, plan: &Plan, index: u32, ack: Ack) -> Result<Outcome, OrderError> {
        if self.digest != plan.digest {
            return Err(OrderError::WrongPlan);
        }
        if self.stopped {
            return Err(OrderError::Stopped);
        }
        if self.acked as usize == plan.steps.len() {
            return Err(OrderError::Finished);
        }
        if index != self.acked {
            return Err(OrderError::OutOfOrder);
        }
        let effect = plan.steps[index as usize].effect;
        let failed = match ack {
            Ack::Done => false,
            Ack::DoneNotDurable => effect.needs_durable(),
            Ack::Failed => true,
        };
        // A copy deleted part way is changed though the step failed; a move to the Trash is whole or not at all, and one that finished but was not
        // synced (`DoneNotDurable`) did happen.
        self.acted |= match effect {
            Effect::Trash(_) => ack != Ack::Failed,
            Effect::Purge(_) => true,
            _ => false,
        };
        if failed {
            self.stopped = true;
        } else {
            self.acked += 1;
        }
        self.outcome(plan)
    }

    /// The outcome of the acknowledged prefix alone; `WrongPlan` for a plan other than this one's.
    pub fn outcome(&self, plan: &Plan) -> Result<Outcome, OrderError> {
        if self.digest != plan.digest {
            return Err(OrderError::WrongPlan);
        }
        if plan.request.scope() == Scope::Leftovers {
            // No barrier to read here (a removal's markers are not in this plan, and `index_of` would say "unchanged" for ever).
            return Ok(if self.acked as usize >= plan.steps.len() {
                Outcome::Complete
            } else if self.acted {
                Outcome::PartlyCleared
            } else {
                Outcome::Unchanged
            });
        }
        Ok(if self.acked <= index_of(plan, Effect::PersistBarrier) {
            Outcome::Unchanged
        } else if self.acked <= index_of(plan, Effect::MarkNoReturn) {
            Outcome::HistoryRetiring
        } else if self.acked <= index_of(plan, Effect::ReadBackTarget) {
            Outcome::ReplacementInDoubt
        } else if (self.acked as usize) < plan.steps.len() {
            Outcome::SanitizedCleanupPending
        } else {
            Outcome::Complete
        })
    }

    /// Every step of the plan is acknowledged.
    pub fn succeeded(&self, plan: &Plan) -> bool {
        !self.stopped && self.digest == plan.digest && self.acked as usize == plan.steps.len()
    }

    pub fn acked(&self) -> u32 {
        self.acked
    }
}
