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
/// from a `Confirmed`, which a confirmation of exactly that plan makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    digest: PlanDigest,
    acked: u32,
    stopped: bool,
}

/// The index of the first step with `effect`. Every plan contains each of the markers once.
fn index_of(plan: &Plan, effect: Effect) -> u32 {
    plan.steps.iter().position(|s| s.effect == effect).map_or(u32::MAX, |i| i as u32)
}

impl Progress {
    /// Begins the walk of `plan`, consuming the confirmation of it. A confirmation of another plan is `WrongPlan`.
    pub fn start(plan: &Plan, confirmed: Confirmed) -> Result<Self, OrderError> {
        if confirmed.digest != plan.digest {
            return Err(OrderError::WrongPlan);
        }
        Ok(Self {
            digest: plan.digest,
            acked: 0,
            stopped: false,
        })
    }

    /// Continue from the persisted acknowledged count of the plan with `digest`, only once the barrier is in that prefix.
    pub fn resume(plan: &Plan, digest: PlanDigest, acked: u32) -> Result<Self, OrderError> {
        if digest != plan.digest {
            return Err(OrderError::WrongPlan);
        }
        if acked as usize > plan.steps.len() || acked <= index_of(plan, Effect::PersistBarrier) {
            return Err(OrderError::NotResumable);
        }
        Ok(Self { digest, acked, stopped: false })
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
