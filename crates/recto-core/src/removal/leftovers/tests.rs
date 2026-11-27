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

/// A temp, and a copy of each kind.
const SOME: [(Store, &str, Owner); 3] = [
    (Store::WriterTemp, ".report.pdf.tmp1", Owner::Target),
    (Store::UpdateCopy, "update/before", Owner::Target),
    (Store::LegacySession, "session.v3", Owner::Unrelated),
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

/// Catches the owner of an artifact being left out of the digest, so that the same file proven someone else's would still pass a
/// confirmation of it as the target's.
#[test]
fn a_changed_owner_changes_the_digest() {
    let mine = plan_of(&[(Store::UpdateCopy, "update/before", Owner::Target)]);
    let other = plan_of(&[(Store::UpdateCopy, "update/before", Owner::Unrelated)]);
    assert_ne!(mine.digest(), other.digest());
}

/// Catches a leftovers plan with a step of the full removal: there is no candidate, no barrier and no install, and the order is the Trash first,
/// then the update copy, then the legacy session.
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

/// Catches a temp the adapter marked to leave (still being written, no Trash there: `Unrelated`) being moved to the Trash with the crashed ones.
#[test]
fn an_unrelated_temp_is_left_alone_but_every_copy_is_cleared() {
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
/// is not "nothing to clear".
#[test]
fn what_cannot_be_placed_or_was_not_searched_refuses() {
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
    for skipped in STORES {
        let mut survey = Survey::new(&DESIGN_LIMITS);
        for store in STORES.into_iter().filter(|s| *s != skipped) {
            survey.mark_complete(store);
        }
        let got = plan(&Request::leftovers(), &survey.finish().unwrap().inventory(), &DESIGN_LIMITS);
        assert_eq!(got.unwrap_err(), Refusal::Incomplete(skipped));
    }
}

/// Catches a store outside the three being hashed into the digest or counted against the cap: a journal written between the check and the clear
/// would make a confirmed plan stale. A survey for this scope that holds one is a bug in the caller and refuses, whatever its owner, and before
/// the cap or the coverage is looked at.
#[test]
fn an_artifact_of_another_store_refuses_before_anything_else() {
    for store in Store::ALL.into_iter().filter(|s| !STORES.contains(s)) {
        for owner in [Owner::Target, Owner::Unrelated, Owner::Unknown, Owner::Shared] {
            let mut survey = Survey::new(&DESIGN_LIMITS);
            survey
                .add(Found {
                    store,
                    key: ArtifactKey::of(store, b"x"),
                    owner,
                    format: Format::Current,
                })
                .unwrap();
            // Not even marked complete: the refusal is the first thing said.
            let got = plan(&Request::leftovers(), &survey.finish().unwrap().inventory(), &DESIGN_LIMITS);
            assert_eq!(got.unwrap_err(), Refusal::Inconsistent, "{store:?} {owner:?}");
        }
    }
}

/// Catches a move batch: every temp has a step of its own, however many there are, so a failed move has moved nothing.
#[test]
fn every_temp_is_a_step_of_its_own() {
    let temps: Vec<(Store, String, Owner)> = (0..5).map(|n| (Store::WriterTemp, format!(".r.pdf.tmp{n}"), Owner::Target)).collect();
    let items: Vec<(Store, &str, Owner)> = temps.iter().map(|(s, n, o)| (*s, n.as_str(), *o)).collect();
    let p = plan_of(&items);
    let moves: Vec<usize> = p
        .steps()
        .iter()
        .filter(|s| matches!(s.effect(), Effect::Trash(_)))
        .map(|s| p.artifacts(s).len())
        .collect();
    assert_eq!(moves, [1; 5]);
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

/// Catches a failed step being told as "nothing changed" after an earlier step acted, or a copy that failed half way (a deletion can stop with
/// some of it gone) being told as untouched. Given one temp a step (`every_temp_is_a_step_of_its_own` holds that), a failed move has moved
/// nothing: with three temps, the first moved and the second failing is "partly cleared", and the first failing is not.
#[test]
fn a_failed_purge_counts_as_an_act_and_a_failed_move_does_not() {
    let three = [
        (Store::WriterTemp, ".r.pdf.tmp1", Owner::Target),
        (Store::WriterTemp, ".r.pdf.tmp2", Owner::Target),
        (Store::WriterTemp, ".r.pdf.tmp3", Owner::Target),
    ];
    let p = plan_of(&three);
    assert!(p.steps()[1..=3].iter().all(|s| s.effect() == Effect::Trash(Store::WriterTemp)));
    let mut first_fails = start(&p);
    first_fails.acknowledge(&p, 0, Ack::Done).unwrap();
    assert_eq!(first_fails.acknowledge(&p, 1, Ack::Failed), Ok(Outcome::Unchanged));
    assert_eq!(first_fails.acknowledge(&p, 2, Ack::Done), Err(OrderError::Stopped));

    let mut second_fails = start(&p);
    second_fails.acknowledge(&p, 0, Ack::Done).unwrap();
    second_fails.acknowledge(&p, 1, Ack::Done).unwrap();
    assert_eq!(second_fails.acknowledge(&p, 2, Ack::Failed), Ok(Outcome::PartlyCleared));

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
        assert_eq!(
            Progress::resume(&p, &Checkpoint::new(p.digest(), acked)),
            Err(OrderError::NotResumable),
            "acked {acked}"
        );
    }
}

const GOLDEN_LEFTOVERS_DIGEST: &str = "7b1269718f37fd2fcc6de9fe3992e27a5f41558711df04af930cc81c51100e40";

/// Pins the bytes of a plan that carries keys, which the removal's golden (made without keys, as core's pure tests are) cannot: the tag, the scope,
/// and each artifact's store, owner, format and key. A change here is deliberate, and a digest saved before it no longer matches.
#[test]
fn the_digest_with_keys_is_pinned() {
    let d = plan_of(&SOME).digest();
    let hex: String = d.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, GOLDEN_LEFTOVERS_DIGEST);
}
