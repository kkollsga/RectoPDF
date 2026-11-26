//! The leftovers plan: `Scope::Leftovers`, the smallest removal anyone can run end to end.
//!
//! Owner, 2026-11-26: leftover unfinished saves go to the Trash and the session copies kept for going back are deleted whole, as a confirmed
//! step of its own, before any removal. So this plan covers three stores only, and nothing else is ever planned: unfinished saves beside the
//! document (`WriterTemp`, moved to the Trash, never deleted), and the copies of the session kept for going back to an earlier version
//! (`UpdateCopy`, then `LegacySession`, deleted whole). Pure: it names no path and opens nothing. The adapters that survey and act hold the proof
//! that an artifact is what it says; this decides only what is planned, in what order, and what is reported.
//!
//! Step order: leases, the Trash first (a move can be put back), then the update copy (`update/before`, whose restore must never find half a
//! copy), then the legacy session copies, then `VerifyRetired` and `ReleaseLeases`. There is no barrier and no candidate: no step can be
//! resumed, because the next run looks again and the person confirms what remains (`Progress::resume` is `NotResumable` here).

use super::plan::{Builder, check_unique, finish};
use super::*;

/// The stores this scope acts on, in the order it acts.
pub(super) const STORES: [Store; 3] = [Store::WriterTemp, Store::UpdateCopy, Store::LegacySession];

fn in_scope(store: Store) -> bool {
    STORES.contains(&store)
}

/// An artifact of `store` this scope acts on. A temp is only the target's: another document's unfinished save beside it is not ours. The two
/// copies are the app's own, kept whole whatever document they came from, so their owner does not matter (an unplaceable one still refuses).
fn acted_on(store: Store, owner: Owner) -> bool {
    match store {
        Store::WriterTemp => owner == Owner::Target,
        _ => matches!(owner, Owner::Target | Owner::SaveAsSource | Owner::Unrelated),
    }
}

pub(super) fn build_plan(request: &Request, inventory: &Inventory<'_>, limits: &Limits) -> Result<Plan, Refusal> {
    let artifacts = inventory.artifacts();
    if let Some(store) = STORES.into_iter().find(|s| !inventory.complete().contains(*s)) {
        return Err(Refusal::Incomplete(store));
    }
    for a in artifacts {
        if a.store.class() == Class::Fixed {
            return Err(Refusal::Inconsistent);
        }
        // Everything outside the three stores is not this scope's business: an unknown owner of a journal does not stop a clear of leftovers.
        if !in_scope(a.store) {
            continue;
        }
        match a.owner {
            Owner::Unknown => return Err(Refusal::Incomplete(a.store)),
            Owner::Shared => return Err(Refusal::Shared(a.store)),
            _ => {}
        }
    }
    check_unique(artifacts)?;
    let batch = usize::from(limits.max_batch.get());
    let mut b = Builder {
        steps: Vec::new(),
        ids: Vec::with_capacity(artifacts.len()),
    };
    b.push(Effect::TakeLeases);
    for store in STORES {
        let effect = if store.class() == Class::Trash {
            Effect::Trash(store)
        } else {
            Effect::Purge(store)
        };
        let ids = artifacts.iter().filter(|a| a.store == store && acted_on(store, a.owner)).map(|a| a.id);
        b.push_ids(effect, ids, batch);
    }
    b.push(Effect::VerifyRetired);
    b.push(Effect::ReleaseLeases);
    let lines = lines(artifacts);
    Ok(finish(request, b, lines, inventory, false))
}

/// The report: only the three stores have anything in it. Every other row says "none found" with no counts, since this scope looked at nothing
/// else (it must not claim, for instance, that the source was "replaced").
fn lines(artifacts: &[Artifact]) -> [StoreLine; Store::COUNT] {
    Store::ALL.map(|store| {
        let (mut acted, mut untouched) = (0, 0);
        if in_scope(store) {
            for a in artifacts.iter().filter(|a| a.store == store) {
                if acted_on(store, a.owner) {
                    acted += 1;
                } else {
                    untouched += 1;
                }
            }
        }
        let disposition = match (acted, store.class()) {
            (0, _) => Disposition::NoneFound,
            (_, Class::Trash) => Disposition::Trashed,
            _ => Disposition::Purged,
        };
        StoreLine {
            store,
            disposition,
            acted,
            kept: 0,
            untouched,
        }
    })
}

#[cfg(test)]
mod tests;
