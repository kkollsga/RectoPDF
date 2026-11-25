use super::*;
use crate::removal::tests::request;
use std::num::NonZeroU16;

fn found(store: Store, name: &str, owner: Owner) -> Found {
    Found {
        store,
        key: ArtifactKey::of(store, name.as_bytes()),
        owner,
        format: Format::Current,
    }
}

fn limits(max: u32) -> Limits {
    Limits {
        max_artifacts: max,
        max_batch: NonZeroU16::new(2).unwrap(),
    }
}

fn all_complete(survey: &mut Survey) {
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        survey.mark_complete(store);
    }
}

fn surveyed(items: &[Found]) -> Result<Surveyed, Refusal> {
    let mut survey = Survey::new(&DESIGN_LIMITS);
    all_complete(&mut survey);
    for f in items {
        survey.add(*f).unwrap();
    }
    survey.finish()
}

fn planned(s: &Surveyed) -> Plan {
    plan(&request(TargetMode::Save, false), &s.inventory(), &DESIGN_LIMITS).unwrap()
}

fn target_ids(p: &Plan) -> Vec<Vec<u32>> {
    p.steps()
        .iter()
        .filter(|s| matches!(s.effect(), Effect::Purge(_) | Effect::Rewrite(_)))
        .map(|s| p.artifacts(s).iter().map(|id| id.0).collect())
        .collect()
}

/// Failure: the order a folder lists its files in (which differs between two runs on one disk) reaches the digest, so a plan a person confirmed
/// is refused as `PlanChanged` when it is taken, though nothing changed.
#[test]
fn a_survey_in_any_order_gives_the_same_plan() {
    let items = [
        found(Store::Journal, "a", Owner::Target),
        found(Store::Journal, "b", Owner::Unrelated),
        found(Store::Backup, "c", Owner::Target),
        found(Store::Backup, "d", Owner::Unrelated),
        found(Store::PageIds, "e", Owner::Target),
        found(Store::PageIds, "f", Owner::Target),
    ];
    let mut reversed = items;
    reversed.reverse();
    let mut shuffled = items;
    shuffled.swap(0, 4);
    shuffled.swap(1, 3);
    let first = planned(&surveyed(&items).unwrap());
    for other in [reversed, shuffled] {
        let second = planned(&surveyed(&other).unwrap());
        assert_eq!(first.steps(), second.steps());
        assert_eq!(first.digest(), second.digest());
    }
}

/// Failure: a file another tab wrote between the offer and the take changes which number names which file, so the target's Purge step would
/// name a different file than the person was shown.
#[test]
fn another_documents_artifact_never_renumbers_the_targets() {
    let mine = [
        found(Store::Journal, "a", Owner::Target),
        found(Store::Journal, "b", Owner::Target),
        found(Store::Backup, "c", Owner::Target),
    ];
    let lowest_target = mine.iter().map(|f| f.key).min().unwrap();
    let theirs = (0..1000)
        .map(|i| found(Store::Journal, &format!("other-{i}"), Owner::Unrelated))
        .min_by_key(|f| f.key)
        .unwrap();
    assert!(theirs.key < lowest_target, "the test needs an unrelated key that sorts first");
    let before = surveyed(&mine).unwrap();
    let with = surveyed(&[theirs, mine[0], mine[1], mine[2]]).unwrap();
    let (a, b) = (planned(&before), planned(&with));
    assert_eq!(target_ids(&a), target_ids(&b));
    let mut all = target_ids(&a).concat();
    all.sort_unstable();
    assert_eq!(all, [1, 2, 3], "the target's ids are 1..=T");
    assert_eq!(b.line(Store::Journal).untouched, 1);
    for id in 1..=3 {
        assert_eq!(before.key(ArtifactId(id)), with.key(ArtifactId(id)));
    }
}

/// Failure: one file is found twice (a listing that overlaps itself) and is acted on twice, or its second owner overrides its first.
#[test]
fn a_repeated_key_is_inconsistent() {
    let twice = [found(Store::Backup, "x", Owner::Target), found(Store::Backup, "x", Owner::Unrelated)];
    assert_eq!(surveyed(&twice).unwrap_err(), Refusal::Inconsistent);
    let apart = [found(Store::Backup, "x", Owner::Target), found(Store::Journal, "x", Owner::Target)];
    assert_ne!(apart[0].key, apart[1].key, "the same name in two stores is two keys");
    assert!(surveyed(&apart).is_ok());
}

/// Failure: the cap truncates the survey and the rest reads as "none found", so a purge leaves the files past the cap behind.
#[test]
fn the_survey_refuses_at_its_limit_plus_one_and_stays_refused() {
    let mut survey = Survey::new(&limits(3));
    for name in ["a", "b", "c"] {
        survey.add(found(Store::Journal, name, Owner::Target)).unwrap();
    }
    assert_eq!(survey.add(found(Store::Journal, "d", Owner::Target)), Err(Refusal::TooLarge));
    assert_eq!(survey.add(found(Store::Journal, "e", Owner::Target)), Err(Refusal::TooLarge));
    assert_eq!(survey.finish().unwrap_err(), Refusal::TooLarge);
}

/// Failure: an adapter that failed silently, so its store was never marked, reads as "none found" and the plan claims it purged everything.
#[test]
fn a_store_not_marked_complete_refuses_the_plan() {
    let mut survey = Survey::new(&DESIGN_LIMITS);
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != Store::Backup) {
        survey.mark_complete(store);
    }
    let missing = survey.finish().unwrap();
    assert_eq!(
        plan(&request(TargetMode::Save, false), &missing.inventory(), &DESIGN_LIMITS).unwrap_err(),
        Refusal::Incomplete(Store::Backup)
    );
    let mut survey = Survey::new(&DESIGN_LIMITS);
    all_complete(&mut survey);
    assert!(plan(&request(TargetMode::Save, false), &survey.finish().unwrap().inventory(), &DESIGN_LIMITS).is_ok());
}

/// Failure: purge acts on the wrong file, because the number a plan step names does not map back to the key of the file it was made for.
#[test]
fn every_planned_id_maps_back_to_its_key() {
    let items = [
        found(Store::Journal, "a", Owner::Target),
        found(Store::Backup, "b", Owner::Target),
        found(Store::Backup, "c", Owner::Unrelated),
    ];
    let s = surveyed(&items).unwrap();
    let p = planned(&s);
    let mut seen = Vec::new();
    for step in p.steps() {
        for id in p.artifacts(step) {
            let (store, key) = s.key(*id).unwrap();
            let want = items.iter().find(|f| f.key == key).unwrap();
            assert_eq!((store, want.owner), (want.store, Owner::Target));
            seen.push(key);
        }
    }
    assert_eq!(seen.len(), 2);
    assert_eq!(s.key(ArtifactId(0)), None);
    assert_eq!(s.key(ArtifactId(4)), None);
    assert!(s.key(ArtifactId(3)).is_some());
}

/// Failure: a survey holds per-artifact bytes the memory budget did not count. Thirty-five bytes a found artifact (a store, a key, two codes).
#[test]
fn a_found_artifact_is_thirty_five_bytes() {
    assert_eq!(std::mem::size_of::<Found>(), 35);
    assert_eq!(std::mem::size_of::<ArtifactKey>(), 32);
}
