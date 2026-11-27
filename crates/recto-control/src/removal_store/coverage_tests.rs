//! The gate that says every store a removal enumerates has a survey, and what a plan over real adapters does. When a store is added and nobody writes its survey
//! the removal would read it as "nothing found"; here it fails the build instead.

use super::caches::tests::{OURS, World};
use super::{survey_backups, survey_caches, survey_journals, survey_legacy_sessions, survey_library, survey_record, survey_update_copy, survey_writer_temps};
use recto_core::removal::{
    DESIGN_LIMITS, Disposition, Kinds, LEDGER_LIMITS, Limits, Lineage, Plan, Refusal, Request, Store, Survey, Surveyed, TargetMode, lineage, plan,
};
use std::num::NonZeroU16;

type Runner = fn(&World, &Lineage<'_>, TargetMode, &mut Survey) -> Result<(), Refusal>;

/// Every survey a removal runs: its name, how it is run over a state folder, and the stores it marks complete.
const ADAPTERS: [(&str, Runner, &[Store]); 8] = [
    (
        "backups",
        |w, l, m, s| survey_backups(&w.dir.join("backups.noindex"), l, m, s),
        &[Store::Backup],
    ),
    (
        "journals",
        |w, l, m, s| survey_journals(&w.dir, l, m, s),
        &[Store::Journal, Store::JournalVariant, Store::PageIds, Store::SleepFile, Store::KeptPages],
    ),
    ("legacy sessions", |w, l, m, s| survey_legacy_sessions(&w.dir, l, m, s), &[Store::LegacySession]),
    ("session record", |w, l, m, s| survey_record(&w.dir, l, m, s), &[Store::SessionRecord]),
    ("update copy", |w, l, m, s| survey_update_copy(&w.dir, l, m, s), &[Store::UpdateCopy]),
    ("writer temps", |_, l, _, s| survey_writer_temps(l, s), &[Store::WriterTemp]),
    ("caches", |w, l, m, s| survey_caches(&w.corpus, &w.contents, l, &w.target(), m, s), &OURS),
    (
        "library",
        |_, _, _, s| {
            survey_library(s);
            Ok(())
        },
        &[Store::Library],
    ),
];

fn limits() -> Limits {
    Limits {
        max_artifacts: 4096,
        max_batch: NonZeroU16::new(4).unwrap(),
    }
}

/// Runs every adapter but `left_out`, over `w` with the first document only in the ledger (or both).
fn surveyed(w: &World, both: bool, left_out: Option<usize>) -> Result<Surveyed, Refusal> {
    let mut ledger = w.ledger();
    if !both {
        ledger.links.truncate(1);
    }
    let lineage = lineage(&ledger, crate::removal_store::testing::version(7), &w.mine, &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits());
    survey.history(&lineage);
    for (at, (_, run, _)) in ADAPTERS.iter().enumerate() {
        if left_out != Some(at) {
            run(w, &lineage, TargetMode::Save, &mut survey)?;
        }
    }
    survey.finish()
}

fn plan_of(s: &Surveyed) -> Result<Plan, Refusal> {
    plan(
        &Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), TargetMode::Save, false),
        &s.inventory(),
        &DESIGN_LIMITS,
    )
}

/// Failure: a store is added to `Store` (or one starts to be enumerated) and no survey is written for it, so a removal passes with it unlooked-at, or one of
/// these is written and marks a store it doesn't list. Each adapter, run over an empty state, marks exactly the stores it names; together they are every
/// enumerated store, once.
#[test]
fn every_enumerated_store_has_a_survey() {
    let mut covered = Vec::new();
    for (name, run, stores) in ADAPTERS {
        let w = World::new();
        let ledger = w.ledger();
        let lineage = lineage(&ledger, crate::removal_store::testing::version(7), &w.mine, &LEDGER_LIMITS).unwrap();
        let mut survey = Survey::new(&limits());
        survey.history(&lineage);
        run(&w, &lineage, TargetMode::Save, &mut survey).unwrap();
        let inventory = survey.finish().unwrap();
        let marked: Vec<Store> = Store::ALL.into_iter().filter(|s| inventory.inventory().complete().contains(*s)).collect();
        let mut wanted = stores.to_vec();
        wanted.sort_by_key(|s| Store::ALL.iter().position(|a| a == s));
        assert_eq!(marked, wanted, "{name} marks what it says it does, over an empty state");
        covered.extend(stores);
    }
    let enumerated: Vec<Store> = Store::ALL.into_iter().filter(|s| s.enumerated()).collect();
    let mut once = covered.clone();
    once.sort_by_key(|s| Store::ALL.iter().position(|a| a == s));
    once.dedup();
    assert_eq!(once, enumerated, "the surveys cover exactly the enumerated stores");
    assert_eq!(covered.len(), enumerated.len(), "no store is surveyed by two adapters");
}

/// Failure: the real adapters, together, refuse a state that holds only the target's own material (so no removal could ever pass), or plan without finding it
/// (a corpus and contents of the target that are passed over). A plan is made, and it names the target's corpus and its contents.
#[test]
fn a_state_with_only_the_targets_material_plans() {
    let w = World::new();
    let key = w.index(&w.mine);
    w.keep_contents(&w.mine, key);
    let p = plan_of(&surveyed(&w, false, None).unwrap()).expect("a plan");
    assert_eq!(p.line(Store::CorpusText).acted, 1);
    assert_eq!(p.line(Store::GeneratedContents).acted, 1);
    assert_eq!(p.line(Store::CorpusAliases).acted, 2, "the notes named for its path and its inode");
    assert_eq!(p.line(Store::CorpusText).disposition, Disposition::Purged);
}

/// Failure: a survey left out of a removal is read as "nothing there", so the plan is made without it. Each adapter left out makes the plan `Incomplete`, naming
/// the first of its stores.
#[test]
fn each_survey_left_out_refuses_with_its_store() {
    let w = World::new();
    let key = w.index(&w.mine);
    w.keep_contents(&w.mine, key);
    for (at, (name, _, stores)) in ADAPTERS.iter().enumerate() {
        let first = Store::ALL.into_iter().find(|s| stores.contains(s)).unwrap();
        let refused = plan_of(&surveyed(&w, false, Some(at)).unwrap()).unwrap_err();
        assert_eq!(refused, Refusal::Incomplete(first), "{name} left out");
    }
}

/// Failure: the refusal changes by accident. Today another document's Find text, which nothing can prove is another's, refuses the removal, and names the
/// corpus. Decided by the owner 2026-11-27 (a confirmed removal clears every document's index); this pin changes when D2e/D5 plan the clear-all.
#[test]
fn another_documents_find_text_refuses_the_removal() {
    let w = World::new();
    for path in [&w.mine, &w.theirs] {
        let key = w.index(path);
        w.keep_contents(path, key);
    }
    let refused = plan_of(&surveyed(&w, true, None).unwrap()).unwrap_err();
    assert_eq!(refused, Refusal::Incomplete(Store::CorpusText));
}
