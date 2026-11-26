//! The leftovers scope: what is planned, in what order, what the digest binds, and what a person may be told after each step.

use super::*;
use crate::removal::plan;
use crate::removal::tests::confirmed;

/// Surveys `items` the way an adapter does (a name, an owner), with the three stores marked complete, plus `extra` marked as well.
fn surveyed(items: &[(Store, &str, Owner)]) -> Surveyed {
    let mut survey = Survey::new(&DESIGN_LIMITS);
    for store in STORES {
        survey.mark_complete(store);
    }
    for (store, name, owner) in items {
        survey
            .add(Found {
                store: *store,
                key: ArtifactKey::of(*store, name.as_bytes()),
                owner: *owner,
                format: Format::Current,
            })
            .unwrap();
    }
    survey.finish().unwrap()
}

fn plan_of(items: &[(Store, &str, Owner)]) -> Plan {
    plan(&Request::leftovers(), &surveyed(items).inventory(), &DESIGN_LIMITS).unwrap()
}

fn effects(p: &Plan) -> Vec<Effect> {
    p.steps().iter().map(Step::effect).collect()
}

/// A temp, one copy of each kind, and something that is not this scope's at all.
const SOME: [(Store, &str, Owner); 4] = [
    (Store::WriterTemp, ".report.pdf.tmp1", Owner::Target),
    (Store::UpdateCopy, "update/before", Owner::Target),
    (Store::LegacySession, "session.v3", Owner::Unrelated),
    (Store::Journal, "journal-a", Owner::Target),
];

/// Catches the confirmed digest not covering which file an artifact is: the person is shown "1 unfinished save", the file is replaced by another one
/// (a save of the same document by another process, a different temp), the count is the same, and the other file is moved to the Trash. With ids
/// and counts only, the two plans are equal. Also that listing order never changes a plan.
#[test]
fn a_different_temp_with_equal_counts_changes_the_digest() {
    let a = plan_of(&[(Store::WriterTemp, ".report.pdf.tmp1", Owner::Target)]);
    let b = plan_of(&[(Store::WriterTemp, ".report.pdf.tmp2", Owner::Target)]);
    assert_eq!(a.steps(), b.steps());
    assert_eq!(a.lines(), b.lines());
    assert_ne!(a.digest(), b.digest());
    let mut reversed = SOME;
    reversed.reverse();
    assert_eq!(plan_of(&SOME).digest(), plan_of(&reversed).digest());
}

/// Catches the owner or the format of an artifact being left out of the digest, so that the same file proven someone else's would still pass a
/// confirmation of it as the target's.
#[test]
fn a_changed_owner_changes_the_digest() {
    let mine = plan_of(&[(Store::UpdateCopy, "update/before", Owner::Target)]);
    let other = plan_of(&[(Store::UpdateCopy, "update/before", Owner::Unrelated)]);
    assert_ne!(mine.digest(), other.digest());
}

/// Catches a leftovers plan reaching any store outside its three, or a step of the full removal: a journal, a backup, the record and the source
/// are never named, and there is no candidate, no barrier and no install.
#[test]
fn only_the_three_stores_are_planned_and_in_the_trash_first_order() {
    let p = plan_of(&SOME);
    assert_eq!(
        effects(&p),
        [
            Effect::TakeLeases,
            Effect::Trash(Store::WriterTemp),
            Effect::Purge(Store::UpdateCopy),
            Effect::Purge(Store::LegacySession),
            Effect::VerifyRetired,
            Effect::ReleaseLeases,
        ]
    );
    assert_eq!(p.request().scope(), Scope::Leftovers);
    let named: usize = p.steps().iter().map(|s| p.artifacts(s).len()).sum();
    assert_eq!(named, 3);
    assert_eq!(p.line(Store::WriterTemp).disposition, Disposition::Trashed);
    assert_eq!(p.line(Store::LegacySession).disposition, Disposition::Purged);
    for store in Store::ALL.into_iter().filter(|s| !STORES.contains(s)) {
        let line = p.line(store);
        assert_eq!(
            (line.disposition, line.acted, line.kept, line.untouched),
            (Disposition::NoneFound, 0, 0, 0),
            "{store:?}"
        );
    }
}

/// Catches an unfinished save of another document, beside this one, being moved to the Trash with this one's.
#[test]
fn a_temp_of_another_document_is_left_alone_but_every_copy_is_cleared() {
    let p = plan_of(&[
        (Store::WriterTemp, ".other.pdf.tmp1", Owner::Unrelated),
        (Store::WriterTemp, ".report.pdf.tmp1", Owner::Target),
        (Store::LegacySession, "session.v2", Owner::Unrelated),
        (Store::LegacySession, "session.damaged", Owner::SaveAsSource),
    ]);
    let temp = p.line(Store::WriterTemp);
    assert_eq!((temp.acted, temp.untouched), (1, 1));
    let copies = p.line(Store::LegacySession);
    assert_eq!((copies.acted, copies.untouched), (2, 0));
}

/// Catches guessing: a copy or a temp nobody could place is not cleared, and not skipped either, but refuses; and a store that was not searched
/// is not "nothing to clear". An unplaceable journal is not this scope's business.
#[test]
fn what_cannot_be_placed_or_was_not_searched_refuses_only_in_the_three_stores() {
    let unknown = surveyed(&[(Store::UpdateCopy, "update/before", Owner::Unknown)]);
    assert_eq!(
        plan(&Request::leftovers(), &unknown.inventory(), &DESIGN_LIMITS).unwrap_err(),
        Refusal::Incomplete(Store::UpdateCopy)
    );
    let shared = surveyed(&[(Store::WriterTemp, "t", Owner::Shared)]);
    assert_eq!(
        plan(&Request::leftovers(), &shared.inventory(), &DESIGN_LIMITS).unwrap_err(),
        Refusal::Shared(Store::WriterTemp)
    );
    assert!(
        plan(
            &Request::leftovers(),
            &surveyed(&[(Store::Journal, "j", Owner::Unknown)]).inventory(),
            &DESIGN_LIMITS
        )
        .is_ok()
    );
    for skipped in STORES {
        let mut survey = Survey::new(&DESIGN_LIMITS);
        for store in STORES.into_iter().filter(|s| *s != skipped) {
            survey.mark_complete(store);
        }
        let got = plan(&Request::leftovers(), &survey.finish().unwrap().inventory(), &DESIGN_LIMITS);
        assert_eq!(got.unwrap_err(), Refusal::Incomplete(skipped));
    }
}

/// Catches the cap on what outside input may make the plan hold: at the limit it plans, one more refuses.
#[test]
fn the_artifact_cap_holds_at_the_limit_plus_one() {
    let limits = Limits {
        max_artifacts: 2,
        ..DESIGN_LIMITS
    };
    let mut survey = Survey::new(&limits);
    for store in STORES {
        survey.mark_complete(store);
    }
    for n in 0..2 {
        let found = Found {
            store: Store::WriterTemp,
            key: ArtifactKey::of(Store::WriterTemp, &[n]),
            owner: Owner::Target,
            format: Format::Current,
        };
        survey.add(found).unwrap();
    }
    let surveyed = survey.finish().unwrap();
    assert!(plan(&Request::leftovers(), &surveyed.inventory(), &limits).is_ok());
    let one_less = Limits {
        max_artifacts: 1,
        ..DESIGN_LIMITS
    };
    assert_eq!(plan(&Request::leftovers(), &surveyed.inventory(), &one_less).unwrap_err(), Refusal::TooLarge);
}

/// Catches a confirmation of one scope starting the other: a person who confirmed "clear leftovers" must not start a removal with it, nor the
/// reverse, even over an empty inventory where every step list could otherwise be compared by length.
#[test]
fn a_confirmation_of_one_scope_cannot_start_the_other() {
    let leftovers = plan_of(&[]);
    let removal = crate::removal::tests::ok(&[]);
    assert_ne!(leftovers.digest(), removal.digest());
    assert_eq!(Progress::start(&removal, confirmed(&leftovers)).unwrap_err(), OrderError::WrongPlan);
    assert_eq!(Progress::start(&leftovers, confirmed(&removal)).unwrap_err(), OrderError::WrongPlan);
}

fn start(p: &Plan) -> Progress {
    Progress::start(p, confirmed(p)).unwrap()
}

/// Catches the removal's own outcome rule being read for this plan. It reads "nothing was changed" for ever when the plan has no barrier, and a
/// person would be told so after a save went to the Trash and a copy was deleted. Walks every prefix: nothing is "unchanged" once an act was
/// acknowledged, and it is complete only at the end.
#[test]
fn after_the_first_act_it_never_says_nothing_changed() {
    let p = plan_of(&SOME);
    let first_act = p
        .steps()
        .iter()
        .position(|s| matches!(s.effect(), Effect::Trash(_) | Effect::Purge(_)))
        .unwrap();
    let mut progress = start(&p);
    assert_eq!(progress.outcome(&p), Ok(Outcome::Unchanged));
    for (i, step) in p.steps().iter().enumerate() {
        let outcome = progress.acknowledge(&p, i as u32, Ack::Done).unwrap();
        let last = i + 1 == p.steps().len();
        let expected = if last {
            Outcome::Complete
        } else if i >= first_act {
            Outcome::PartlyCleared
        } else {
            Outcome::Unchanged
        };
        assert_eq!(outcome, expected, "after {:?}", step.effect());
    }
    assert!(progress.succeeded(&p));
}

/// Catches a copy that failed half way being reported as untouched. A deletion can stop with some of the copy gone, so a failed `Purge` counts as
/// done something; a move to the Trash is whole or nothing, so a failed one leaves "nothing changed" true. Both stop the walk.
#[test]
fn a_failed_purge_counts_as_an_act_and_a_failed_move_does_not() {
    let p = plan_of(&SOME);
    let at = |effect| p.steps().iter().position(|s| s.effect() == effect).unwrap() as u32;

    let mut moved = start(&p);
    moved.acknowledge(&p, 0, Ack::Done).unwrap();
    assert_eq!(moved.acknowledge(&p, at(Effect::Trash(Store::WriterTemp)), Ack::Failed), Ok(Outcome::Unchanged));
    assert_eq!(moved.acknowledge(&p, 2, Ack::Done), Err(OrderError::Stopped));

    // Only a copy, so nothing before it has acted.
    let q = plan_of(&[(Store::UpdateCopy, "update/before", Owner::Target)]);
    let mut deleted = start(&q);
    deleted.acknowledge(&q, 0, Ack::Done).unwrap();
    assert_eq!(deleted.acknowledge(&q, 1, Ack::Failed), Ok(Outcome::PartlyCleared));

    // A trash whose move finished but was not synced happened, and stops the walk.
    let mut unsynced = start(&p);
    unsynced.acknowledge(&p, 0, Ack::Done).unwrap();
    assert_eq!(unsynced.acknowledge(&p, 1, Ack::DoneNotDurable), Ok(Outcome::PartlyCleared));
}

/// Catches a leftovers walk being picked up from a stored count. Nothing here has a barrier a record could hold; the next run looks again.
#[test]
fn it_is_never_resumable() {
    let p = plan_of(&SOME);
    for acked in 0..=p.steps().len() as u32 {
        assert_eq!(Progress::resume(&p, p.digest(), acked), Err(OrderError::NotResumable), "acked {acked}");
    }
}

const GOLDEN_LEFTOVERS_DIGEST: &str = "af9c3ee4b8f0aa6befa5709c3f1005813e5c8f64f7803e131783b1189ac6967e";

/// Pins the bytes of a plan that carries keys, which the removal's golden (made without keys, as core's pure tests are) cannot: the tag, the scope,
/// and each artifact's store, owner, format and key. A change here is deliberate, and a digest saved before it no longer matches.
#[test]
fn the_digest_with_keys_is_pinned() {
    let d = plan_of(&SOME).digest();
    let hex: String = d.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, GOLDEN_LEFTOVERS_DIGEST);
}
