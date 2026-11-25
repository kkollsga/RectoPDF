//! Confirmations: a pending plan kept for the person who was shown it, until they confirm it, it goes stale or it expires.
//!
//! Owned by the caller, capped, with no global, no clock of its own and no thread. Core only compares actors for equality; who may ask
//! for what is stated where the command is (D5), not here. A plan is kept as its digest and a hash of its target, never a path.

use super::*;
use recto_platform::FileStamp;
use sha2::{Digest, Sha256};
use std::num::NonZeroU8;
use std::path::Path;
use std::time::{Duration, Instant};

/// What may be pending at once and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfirmLimits {
    pub max_pending: NonZeroU8,
    pub ttl: Duration,
}

/// Design limits, not measurements: 8 pending plans, 10 minutes.
pub const CONFIRM_LIMITS: ConfirmLimits = ConfirmLimits {
    max_pending: match NonZeroU8::new(8) {
        Some(n) => n,
        None => unreachable!(),
    },
    ttl: Duration::from_secs(600),
};

/// Names one pending plan. A counter from 1, never reused within a store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanId(u64);

impl PlanId {
    pub fn get(self) -> u64 {
        self.0
    }

    pub fn from_raw(raw: u64) -> Self {
        Self(raw)
    }
}

/// What the document was when the plan was made: the caller's own change counter and the source file's stamp.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub epoch: u64,
    pub source: FileStamp,
}

/// Where the sanitized file goes, as a hash: equal only for the same kind of target over the same source and output paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetKey {
    mode: TargetMode,
    hash: [u8; 32],
}

impl TargetKey {
    /// `SHA-256("recto-removal-target/1\0" ‖ kind u8 ‖ len-prefixed source bytes ‖ len-prefixed output bytes)`; the output is empty for `Same`.
    pub fn new(source: &Path, target: crate::save::Target<'_>) -> Self {
        use crate::save::Target;
        let (mode, kind, out) = match target {
            Target::Same => (TargetMode::Save, 0u8, None),
            Target::NewFile(p) => (TargetMode::SaveAs, 1, Some(p)),
            Target::Replace(p) => (TargetMode::SaveAs, 2, Some(p)),
        };
        let mut h = Sha256::new();
        h.update(b"recto-removal-target/1\0");
        h.update([kind]);
        for path in [Some(source), out] {
            let bytes = path.map_or(&[][..], |p| p.as_os_str().as_encoded_bytes());
            h.update((bytes.len() as u64).to_le_bytes());
            h.update(bytes);
        }
        Self {
            mode,
            hash: h.finalize().into(),
        }
    }

    pub fn mode(&self) -> TargetMode {
        self.mode
    }
}

/// Everything a confirmation is bound to besides the plan itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding<A> {
    pub actor: A,
    pub version: Version,
    pub target: TargetKey,
}

/// Why a plan was not kept or not confirmed. Never carries input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmRefusal {
    Unknown,
    Expired,
    OtherActor,
    OtherTarget,
    Stale,
    PlanChanged,
    /// Full of other actors' plans, or out of ids.
    Full,
    /// The target's mode is not the one the plan was made for.
    Mismatched,
}

/// Proof that a person confirmed exactly one plan. Not `Clone`, and made only here: `Progress::start` consumes it.
#[derive(Debug)]
pub struct Confirmed {
    pub(super) digest: PlanDigest,
}

impl Confirmed {
    pub fn digest(&self) -> PlanDigest {
        self.digest
    }
}

struct Pending<A> {
    id: u64,
    binding: Binding<A>,
    digest: PlanDigest,
    created: Instant,
}

/// The plans waiting for a yes. Allocates nothing until the first offer; at most `max_pending` entries.
pub struct Confirmations<A> {
    limits: ConfirmLimits,
    next: u64,
    pending: Vec<Pending<A>>,
}

impl<A: Copy + Eq> Confirmations<A> {
    pub fn new(limits: ConfirmLimits) -> Self {
        Self {
            limits,
            next: 1,
            pending: Vec::new(),
        }
    }

    fn expired(&self, p: &Pending<A>, now: Instant) -> bool {
        now.saturating_duration_since(p.created) >= self.limits.ttl
    }

    /// Keeps `plan` for `binding.actor`. A new plan for the same actor and target replaces the old one. When full, the asker's own
    /// oldest plan goes; if the asker has none, nobody else's is pushed out and this is `Full`.
    pub fn offer(&mut self, binding: Binding<A>, plan: &Plan, now: Instant) -> Result<PlanId, ConfirmRefusal> {
        if binding.target.mode() != plan.request().mode() {
            return Err(ConfirmRefusal::Mismatched);
        }
        if self.next == u64::MAX {
            return Err(ConfirmRefusal::Full);
        }
        let ttl = self.limits.ttl;
        self.pending.retain(|p| now.saturating_duration_since(p.created) < ttl);
        self.pending
            .retain(|p| !(p.binding.actor == binding.actor && p.binding.target == binding.target));
        if self.pending.len() >= usize::from(self.limits.max_pending.get()) {
            let own = self.pending.iter().position(|p| p.binding.actor == binding.actor);
            self.pending.remove(own.ok_or(ConfirmRefusal::Full)?);
        }
        let id = self.next;
        self.next += 1;
        self.pending.push(Pending {
            id,
            binding,
            digest: plan.digest(),
            created: now,
        });
        Ok(PlanId(id))
    }

    /// Confirms a plan. Checked in order: unknown id, another actor (nothing changes and nothing else is said), expired, another
    /// target, a changed document, a changed plan. Every refusal after the actor check removes the entry; success removes it too.
    pub fn take(&mut self, id: PlanId, binding: &Binding<A>, plan: &Plan, now: Instant) -> Result<Confirmed, ConfirmRefusal> {
        let at = self.pending.iter().position(|p| p.id == id.0).ok_or(ConfirmRefusal::Unknown)?;
        let p = &self.pending[at];
        if p.binding.actor != binding.actor {
            return Err(ConfirmRefusal::OtherActor);
        }
        let verdict = if self.expired(p, now) {
            Err(ConfirmRefusal::Expired)
        } else if p.binding.target != binding.target {
            Err(ConfirmRefusal::OtherTarget)
        } else if p.binding.version != binding.version {
            Err(ConfirmRefusal::Stale)
        } else if p.digest != plan.digest() {
            Err(ConfirmRefusal::PlanChanged)
        } else {
            Ok(Confirmed { digest: p.digest })
        };
        self.pending.remove(at);
        verdict
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests;
