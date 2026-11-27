//! Hostile surveys for the leftovers plan (`recto_core::removal`, `Scope::Leftovers`): whatever the adapters found, in whatever order, the plan
//! never names a store outside its three or an artifact it could not place, is the same under any listing order, changes when a file is another
//! file, and a walk of it under any acknowledgments never says "nothing changed" once something was done and never resumes.
//!
//! Input: byte 0 says which of the three stores were searched (bits 0 to 2), byte 1 how many artifacts (0 to 24), then four bytes each (store, owner,
//! format, name); the bytes after are the carrier's acknowledgments, one a step.

use recto_core::removal::{
    Ack, Actor, ArtifactKey, BarrierFacts, BarrierRefusal, Binding, CONFIRM_LIMITS, Confirmations, DESIGN_LIMITS, Disposition, Effect, FileVersion, Format,
    Found, Kinds, Outcome, Owner, PlanId, Progress, Refusal, Request, Scope, Store, Survey, Surveyed, TargetKey, TargetMode, Version, encode_barrier, plan,
};
use recto_core::save::Target;
use recto_platform::FileStamp;
use std::path::Path;
use std::time::Instant;

/// The three stores a leftovers plan may act on.
const SCOPE: [Store; 3] = [Store::WriterTemp, Store::UpdateCopy, Store::LegacySession];

fn owner(byte: u8) -> Owner {
    [Owner::Target, Owner::SaveAsSource, Owner::Unrelated, Owner::Shared, Owner::Unknown][usize::from(byte % 5)]
}

/// One byte per field, and `seeds` write the same layout.
fn decode(data: &[u8]) -> (u8, Vec<Found>, &[u8]) {
    let (mask, count) = (data.first().copied().unwrap_or(0), usize::from(data.get(1).copied().unwrap_or(0) % 25));
    let body = data.get(2..).unwrap_or_default();
    let (records, acks) = body.split_at((count * 4).min(body.len()));
    let found = records
        .as_chunks::<4>()
        .0
        .iter()
        .map(|r| {
            let store = Store::ALL[usize::from(r[0]) % Store::COUNT];
            Found {
                store,
                key: ArtifactKey::of(store, &[r[3]]),
                owner: owner(r[1]),
                format: if r[2] & 1 == 0 { Format::Current } else { Format::Foreign },
            }
        })
        .collect();
    (mask, found, acks)
}

fn survey(mask: u8, found: impl Iterator<Item = Found>) -> Option<Surveyed> {
    let mut survey = Survey::new(&DESIGN_LIMITS);
    for (i, store) in SCOPE.into_iter().enumerate() {
        if mask >> i & 1 == 1 {
            survey.mark_complete(store);
        }
    }
    for f in found {
        survey.add(f).ok()?;
    }
    survey.finish().ok()
}

fn digest_of(mask: u8, found: &[Found]) -> Option<[u8; 32]> {
    let surveyed = survey(mask, found.iter().copied())?;
    plan(&Request::leftovers(), &surveyed.inventory(), &DESIGN_LIMITS)
        .ok()
        .map(|p| *p.digest().as_bytes())
}

fn confirmed(p: &recto_core::removal::Plan) -> recto_core::removal::Confirmed {
    let mut store = Confirmations::new(CONFIRM_LIMITS);
    let binding = Binding {
        actor: 0u8,
        version: Version {
            epoch: 1,
            source: FileStamp {
                size: 1,
                mtime_ns: 2,
                device: 3,
                inode: 4,
            },
        },
        target: TargetKey::new(Path::new("/doc.pdf"), Target::Same),
    };
    let now = Instant::now();
    let id = store
        .offer(binding, p, now, PlanId::from_raw(9).expect("not zero"))
        .expect("a leftovers plan is offered for the document itself");
    store.take(id, &binding, p, now).expect("and taken back as it was")
}

/// Valid inputs that reach the plan, a refusal, a changed key and a walk.
pub fn seeds() -> Vec<Vec<u8>> {
    let record = |store: Store, owner: u8, name: u8| [Store::ALL.iter().position(|s| *s == store).unwrap() as u8, owner, 0, name];
    let with = |mask: u8, records: &[[u8; 4]], acks: &[u8]| {
        let mut v = vec![mask, records.len() as u8];
        v.extend(records.iter().flatten());
        v.extend(acks);
        v
    };
    vec![
        with(7, &[], &[]),
        with(
            7,
            &[
                record(Store::WriterTemp, 0, 1),
                record(Store::UpdateCopy, 0, 2),
                record(Store::LegacySession, 2, 3),
            ],
            &[0; 8],
        ),
        with(
            7,
            &[record(Store::WriterTemp, 0, 1), record(Store::Journal, 4, 2), record(Store::Backup, 3, 3)],
            &[0, 0, 2, 0, 0],
        ),
        with(3, &[record(Store::WriterTemp, 0, 1)], &[]),
        with(
            7,
            &[
                record(Store::WriterTemp, 0, 1),
                record(Store::WriterTemp, 0, 2),
                record(Store::WriterTemp, 0, 3),
            ],
            &[0, 0, 2, 0],
        ),
        with(7, &[record(Store::UpdateCopy, 4, 1)], &[]),
        with(
            7,
            &[
                record(Store::LegacySession, 0, 1),
                record(Store::LegacySession, 1, 2),
                record(Store::UpdateCopy, 0, 4),
            ],
            &[0, 0, 0, 1],
        ),
    ]
}

/// The check libFuzzer, the smoke run and the regression tests share.
pub fn check(data: &[u8]) {
    let (mask, found, acks) = decode(data);
    let Some(surveyed) = survey(mask, found.iter().copied()) else { return };
    let outside = found.iter().any(|f| !SCOPE.contains(&f.store));
    // The removal's own plan over the same survey never panics, whatever it says.
    let _ = plan(
        &Request::new(Kinds::from_bits(Kinds::METADATA).expect("a kind"), TargetMode::Save, false),
        &surveyed.inventory(),
        &DESIGN_LIMITS,
    );
    let made = plan(&Request::leftovers(), &surveyed.inventory(), &DESIGN_LIMITS);
    // A store outside the three is refused first, whatever else is true of the survey.
    if outside {
        assert_eq!(made.err(), Some(Refusal::Inconsistent), "a store outside the scope was not refused first");
        return;
    }
    let Ok(p) = made else {
        return;
    };
    assert_eq!(p.request().scope(), Scope::Leftovers);

    // Only the three stores, only artifacts that were placed, and nothing a removal does.
    for step in p.steps() {
        match step.effect() {
            Effect::TakeLeases | Effect::VerifyRetired | Effect::ReleaseLeases => assert!(p.artifacts(step).is_empty()),
            Effect::Trash(s) | Effect::Purge(s) => {
                assert!(SCOPE.contains(&s), "a store outside the scope was planned");
                if matches!(step.effect(), Effect::Trash(_)) {
                    assert_eq!(p.artifacts(step).len(), 1, "a move step holds more than one move");
                }
                assert!(p.artifacts(step).len() <= usize::from(DESIGN_LIMITS.max_batch.get()));
            }
            other => panic!("a step of a removal in a leftovers plan: {other:?}"),
        }
    }
    for line in p.lines() {
        if !SCOPE.contains(&line.store) {
            assert_eq!((line.disposition, line.acted, line.kept, line.untouched), (Disposition::NoneFound, 0, 0, 0));
        }
    }
    let named: usize = p.steps().iter().map(|s| p.artifacts(s).len()).sum();
    let acted: u32 = SCOPE.into_iter().map(|s| p.line(s).acted).sum();
    assert_eq!(named as u32, acted, "the report and the steps agree on what is acted on");
    for f in &found {
        if SCOPE.contains(&f.store) {
            assert!(!matches!(f.owner, Owner::Unknown | Owner::Shared), "an artifact nobody could place was planned");
        }
    }

    // The same set in any listing order is one plan; another file in place of one is another.
    let base = *p.digest().as_bytes();
    let mut reversed = found.clone();
    reversed.reverse();
    assert_eq!(digest_of(mask, &reversed), Some(base), "the order artifacts were listed in changed the plan");
    if let Some(first) = found.first() {
        let mut other = found.clone();
        other[0].key = ArtifactKey::of(first.store, b"another file");
        if let Some(changed) = digest_of(mask, &other) {
            assert_ne!(changed, base, "a different file with the same counts kept the digest");
        }
    }

    // A walk under any acknowledgments: a destructive Done is never "nothing changed", and nothing resumes.
    let mut progress = Progress::start(&p, confirmed(&p)).expect("its own confirmation starts it");
    let mut destructive = false;
    for (i, step) in p.steps().iter().enumerate() {
        let ack = match acks.get(i).copied().unwrap_or(0) % 3 {
            0 => Ack::Done,
            1 => Ack::DoneNotDurable,
            _ => Ack::Failed,
        };
        let outcome = progress.acknowledge(&p, i as u32, ack).expect("the next step in order");
        let act = matches!(step.effect(), Effect::Trash(_) | Effect::Purge(_));
        destructive |= act && ack != Ack::Failed;
        if destructive {
            assert_ne!(outcome, Outcome::Unchanged, "nothing changed was said after a destructive step");
        }
        assert_eq!(outcome == Outcome::Complete, progress.succeeded(&p));
        // Nothing a record could hold: whatever the walk has done, no record of it is made.
        let facts = BarrierFacts {
            target: "/doc.pdf".into(),
            source: FileVersion {
                volume: None,
                inode: 1,
                size: 1,
                mtime_ns: 1,
            },
            candidate_name: "c".into(),
            candidate: FileVersion {
                volume: None,
                inode: 2,
                size: 1,
                mtime_ns: 1,
            },
            candidate_sha: [0; 32],
            roots: Vec::new(),
            actor: Actor::Person,
            pins: Vec::new(),
        };
        assert_eq!(
            encode_barrier(&p, &surveyed, &DESIGN_LIMITS, &progress, &facts).err(),
            Some(BarrierRefusal::Leftovers),
            "a leftovers walk was recorded"
        );
        if ack == Ack::Failed || (ack == Ack::DoneNotDurable && step.effect().needs_durable()) {
            assert!(progress.next(&p).is_none(), "a failure did not stop the walk");
            return;
        }
    }
    assert!(progress.succeeded(&p));
}
