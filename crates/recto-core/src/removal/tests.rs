use super::*;

const LIMITS: Limits = Limits {
    max_artifacts: 4,
    max_batch: match NonZeroU16::new(2) {
        Some(n) => n,
        None => unreachable!(),
    },
};

fn kinds() -> Kinds {
    Kinds::from_bits(Kinds::METADATA).unwrap()
}

fn request(mode: TargetMode, signed: bool) -> Request {
    Request::new(kinds(), mode, signed)
}

fn complete() -> StoreSet {
    Store::ALL.into_iter().filter(|s| s.enumerated()).fold(StoreSet::empty(), StoreSet::with)
}

fn art(id: u32, store: Store, owner: Owner) -> Artifact {
    Artifact {
        id: ArtifactId(id),
        store,
        owner,
        format: Format::Current,
    }
}

fn make(mode: TargetMode, signed: bool, artifacts: &[Artifact], limits: &Limits) -> Result<Plan, Refusal> {
    plan(&request(mode, signed), &Inventory::new(complete(), artifacts), limits)
}

fn ok(artifacts: &[Artifact]) -> Plan {
    make(TargetMode::Save, false, artifacts, &DESIGN_LIMITS).unwrap()
}

fn effects(p: &Plan) -> Vec<Effect> {
    p.steps().iter().map(Step::effect).collect()
}

fn position(p: &Plan, e: Effect) -> usize {
    effects(p).iter().position(|x| *x == e).unwrap()
}

/// Walks a plan acknowledging every step; used for outcome sweeps.
fn progress_after(p: &Plan, acked: u32) -> Progress {
    let mut pr = Progress::start(p);
    for i in 0..acked {
        pr.acknowledge(p, i, Ack::Done).unwrap();
    }
    pr
}

/// Catches a plan that says "nothing to remove" for a store nobody searched.
#[test]
fn missing_coverage_refuses_rather_than_planning_nothing() {
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        let mut set = StoreSet::empty();
        for s in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != store) {
            set = set.with(s);
        }
        let got = plan(&request(TargetMode::Save, false), &Inventory::new(set, &[]), &DESIGN_LIMITS);
        assert_eq!(got.unwrap_err(), Refusal::Incomplete(store));
    }
    assert!(make(TargetMode::Save, false, &[], &DESIGN_LIMITS).is_ok());
}

/// Catches guessing that an artifact of unproven ownership is another document's.
#[test]
fn unknown_owner_refuses_scope_incomplete() {
    let got = make(TargetMode::Save, false, &[art(1, Store::Journal, Owner::Unknown)], &DESIGN_LIMITS);
    assert_eq!(got.unwrap_err(), Refusal::Incomplete(Store::Journal));
}

/// Catches purging or rewriting something another document's history still uses.
#[test]
fn shared_artifact_refuses_in_every_store() {
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        let got = make(TargetMode::Save, false, &[art(1, store, Owner::Shared)], &DESIGN_LIMITS);
        assert_eq!(got.unwrap_err(), Refusal::Shared(store));
    }
}

/// Catches another document's files, or the Save As source, being named by a step.
#[test]
fn unrelated_and_source_artifacts_never_appear_in_steps() {
    let inv = [
        art(1, Store::Journal, Owner::Target),
        art(2, Store::Journal, Owner::Unrelated),
        art(3, Store::Backup, Owner::SaveAsSource),
        art(4, Store::SleepFile, Owner::Target),
    ];
    let p = make(TargetMode::SaveAs, false, &inv, &LIMITS).unwrap();
    let mut named: Vec<u32> = p.steps().iter().flat_map(|s| p.artifacts(s).iter().map(|a| a.0)).collect();
    named.sort_unstable();
    assert_eq!(named, [1, 4]);
    let j = p.line(Store::Journal);
    assert_eq!((j.acted, j.kept, j.untouched), (1, 0, 1));
    let b = p.line(Store::Backup);
    assert_eq!((b.disposition, b.acted, b.kept), (Disposition::NoneFound, 0, 1));
}

/// Catches a store missing from the report, or one the report acts on though it is excluded.
#[test]
fn every_inventory_row_has_exactly_one_line() {
    let p = ok(&[art(1, Store::Journal, Owner::Target)]);
    let stores: Vec<Store> = p.lines().iter().map(|l| l.store).collect();
    assert_eq!(stores, Store::ALL);
    for s in [Store::Logs, Store::Clipboard, Store::External] {
        let l = p.line(s);
        assert!(matches!(l.disposition, Disposition::Excluded(_)));
        assert_eq!((l.acted, l.kept, l.untouched), (0, 0, 0));
        assert!(!effects(&p).iter().any(|e| matches!(e, Effect::Purge(x) | Effect::Rewrite(x) if *x == s)));
    }
    assert_eq!(p.line(Store::Clipboard).disposition, Disposition::Excluded(Exclusion::NotCleared));
    assert_eq!(p.line(Store::Journal).disposition, Disposition::Purged);
    assert_eq!(p.line(Store::SourcePdf).disposition, Disposition::Replaced);
}

/// Catches the cap being off by one, or applied after allocation.
#[test]
fn inventory_at_limit_plans_and_limit_plus_one_refuses() {
    let mut inv: Vec<Artifact> = (0..4).map(|i| art(i, Store::Journal, Owner::Unrelated)).collect();
    assert!(make(TargetMode::Save, false, &inv, &LIMITS).is_ok());
    inv.push(art(4, Store::Journal, Owner::Unrelated));
    assert_eq!(make(TargetMode::Save, false, &inv, &LIMITS).unwrap_err(), Refusal::TooLarge);
}

/// Catches a batch over the cap, or ids reordered or dropped between batches.
#[test]
fn batches_respect_limit_and_preserve_input_order() {
    let inv = [
        art(9, Store::Journal, Owner::Target),
        art(3, Store::Journal, Owner::Target),
        art(7, Store::Journal, Owner::Target),
        art(5, Store::Journal, Owner::Target),
    ];
    let limits = Limits { max_artifacts: 8, ..LIMITS };
    let p = make(TargetMode::Save, false, &inv, &limits).unwrap();
    let batches: Vec<Vec<u32>> = p
        .steps()
        .iter()
        .filter(|s| s.effect() == Effect::Purge(Store::Journal))
        .map(|s| p.artifacts(s).iter().map(|a| a.0).collect())
        .collect();
    assert_eq!(batches, [vec![9, 3], vec![7, 5]]);
    let five: Vec<Artifact> = (0..5).map(|i| art(i, Store::Journal, Owner::Target)).collect();
    let p = make(TargetMode::Save, false, &five, &Limits { max_artifacts: 8, ..LIMITS }).unwrap();
    let sizes: Vec<u16> = p
        .steps()
        .iter()
        .filter(|s| s.effect() == Effect::Purge(Store::Journal))
        .map(Step::len)
        .collect();
    assert_eq!(sizes, [2, 2, 1]);
}

/// Catches a step order that lets cleanup start before the barrier is durable, a writer recreate
/// what was cleared, or the install come before the point of no return.
#[test]
fn crash_states_follow_phase_order() {
    let inv = [
        art(1, Store::Journal, Owner::Target),
        art(2, Store::SessionRecord, Owner::Target),
        art(3, Store::CorpusText, Owner::Target),
    ];
    let p = ok(&inv);
    let e = effects(&p);
    assert!(e.windows(2).all(|w| w[0].phase() <= w[1].phase()));
    assert_eq!(e[0], Effect::TakeLeases);
    assert_eq!(*e.last().unwrap(), Effect::RetireBarrier);
    let candidate = position(&p, Effect::WriteCandidate);
    for w in Writer::ALL {
        let at: Vec<usize> = (0..e.len()).filter(|i| e[*i] == Effect::Quiesce(w)).collect();
        assert_eq!(at.len(), 1);
        assert!(at[0] < candidate);
    }
    let barrier = position(&p, Effect::PersistBarrier);
    let last_purge = e.iter().rposition(|x| matches!(x, Effect::Purge(_) | Effect::Rewrite(_))).unwrap();
    for (i, x) in e.iter().enumerate() {
        if matches!(x, Effect::Purge(_) | Effect::Rewrite(_) | Effect::ClearFindText) {
            assert!(i > barrier);
        }
    }
    assert!(position(&p, Effect::VerifyRetired) > last_purge);
    assert!(position(&p, Effect::InstallTarget) > position(&p, Effect::MarkNoReturn));
}

/// Catches restoring old history after a purge, by a restore entry point that is purged last.
#[test]
fn purge_order_removes_restore_entry_points_first() {
    let inv = [
        art(1, Store::WriterTemp, Owner::Target),
        art(2, Store::Backup, Owner::Target),
        art(3, Store::SleepFile, Owner::Target),
        art(4, Store::Journal, Owner::Target),
    ];
    let limits = Limits { max_artifacts: 16, ..LIMITS };
    let mut inv = inv.to_vec();
    inv.extend([
        art(5, Store::UpdateCopy, Owner::Target),
        art(6, Store::LegacySession, Owner::Target),
        art(7, Store::SessionRecord, Owner::Target),
        art(8, Store::CorpusAliases, Owner::Target),
        art(9, Store::CorpusText, Owner::Target),
        art(10, Store::KeptPages, Owner::Target),
        art(11, Store::PageIds, Owner::Target),
    ]);
    let p = make(TargetMode::Save, false, &inv, &limits).unwrap();
    let at = |e| position(&p, e);
    assert!(at(Effect::Rewrite(Store::UpdateCopy)) < at(Effect::Rewrite(Store::LegacySession)));
    assert!(at(Effect::Rewrite(Store::LegacySession)) < at(Effect::Rewrite(Store::SessionRecord)));
    assert!(at(Effect::Rewrite(Store::SessionRecord)) < at(Effect::ClearFindText));
    assert!(at(Effect::ClearFindText) < at(Effect::Purge(Store::Journal)));
    assert!(at(Effect::Purge(Store::Journal)) < at(Effect::Purge(Store::PageIds)));
    assert!(at(Effect::Purge(Store::PageIds)) < at(Effect::Purge(Store::KeptPages)));
    assert!(at(Effect::Purge(Store::KeptPages)) < at(Effect::Purge(Store::SleepFile)));
    assert!(at(Effect::Purge(Store::SleepFile)) < at(Effect::Purge(Store::Backup)));
    assert!(at(Effect::Rewrite(Store::CorpusAliases)) < at(Effect::Purge(Store::CorpusText)));
}

/// Catches a store silently missing from the step order: a new enumerated store with no step.
#[test]
fn every_enumerated_store_except_library_is_in_the_step_order() {
    let inv: Vec<Artifact> = Store::ALL
        .into_iter()
        .filter(|s| s.enumerated() && s.class() != Class::Unsupported)
        .enumerate()
        .map(|(i, s)| art(i as u32, s, Owner::Target))
        .collect();
    let limits = Limits { max_artifacts: 64, ..LIMITS };
    let p = make(TargetMode::Save, false, &inv, &limits).unwrap();
    for a in &inv {
        let named = p.steps().iter().any(|s| p.artifacts(s).contains(&a.id));
        assert!(named, "{:?} has no step", a.store);
    }
}

/// Catches a crash being described as "nothing written" or "complete" when the acknowledged prefix
/// says otherwise: the contract is stated over every prefix, not copied from the code.
#[test]
fn outcome_for_every_prefix_keeps_the_contract() {
    let p = ok(&[art(1, Store::Journal, Owner::Target)]);
    let n = p.steps().len() as u32;
    let acked_step = |acked: u32, effect: Effect| acked > position(&p, effect) as u32;
    for acked in 0..=n {
        let o = progress_after(&p, acked).outcome(&p);
        if o.nothing_written() {
            for effect in [Effect::PersistBarrier, Effect::InstallTarget] {
                assert!(!acked_step(acked, effect), "{effect:?} acknowledged yet nothing written, acked {acked}");
            }
            assert!(
                p.steps()[..acked as usize]
                    .iter()
                    .all(|s| !matches!(s.effect(), Effect::Purge(_) | Effect::Rewrite(_) | Effect::ClearFindText)),
                "acked {acked}"
            );
        }
        assert_eq!(o.nothing_written(), !acked_step(acked, Effect::PersistBarrier), "acked {acked}");
        assert_eq!(o.may_cancel(), o.nothing_written());
        if o == Outcome::Complete {
            assert_eq!(acked, n, "complete with a step unacknowledged");
        }
        if matches!(o, Outcome::SanitizedCleanupPending | Outcome::Complete) {
            assert!(acked_step(acked, Effect::ReadBackTarget), "installed but not read back, acked {acked}");
        }
    }
}

/// Catches a failed acknowledgment of one of the last three steps being told to a person as
/// `Complete`: the sanitized file is installed, but the invalidation, the leases or the barrier
/// are not finished, so the report may say only that cleanup is pending.
#[test]
fn a_failed_final_step_is_never_complete() {
    let p = ok(&[]);
    for effect in [Effect::PublishInvalidation, Effect::ReleaseLeases, Effect::RetireBarrier] {
        let at = position(&p, effect) as u32;
        let mut pr = progress_after(&p, at);
        let o = pr.acknowledge(&p, at, Ack::Failed).unwrap();
        assert_eq!(o, Outcome::SanitizedCleanupPending, "{effect:?}");
        assert!(!pr.succeeded(&p));
        assert_ne!(pr.outcome(&p), Outcome::Complete);
    }
}

/// Catches success being reported with a step skipped, or an acknowledgment out of order moving state.
#[test]
fn success_requires_every_acknowledgment() {
    let p = ok(&[]);
    let mut pr = Progress::start(&p);
    assert_eq!(pr.acknowledge(&p, 1, Ack::Done), Err(OrderError::OutOfOrder));
    assert_eq!(pr, Progress::start(&p));
    let n = p.steps().len() as u32;
    for i in 0..n {
        assert!(!pr.succeeded(&p));
        assert_eq!(pr.next(&p), Some(p.steps()[i as usize]));
        pr.acknowledge(&p, i, Ack::Done).unwrap();
    }
    assert!(pr.succeeded(&p));
    assert_eq!(pr.next(&p), None);
    assert_eq!(pr.acknowledge(&p, n, Ack::Done), Err(OrderError::Finished));
}

/// Catches "done but not synced" on a durable step counting as done, and a quiesce failing needlessly.
#[test]
fn unsynced_durable_step_stops_but_unsynced_quiesce_advances() {
    let p = ok(&[]);
    let mut pr = progress_after(&p, 1);
    assert!(pr.acknowledge(&p, 1, Ack::DoneNotDurable).is_ok());
    assert_eq!(pr.acked(), 2);
    let barrier = position(&p, Effect::PersistBarrier) as u32;
    let mut pr = progress_after(&p, barrier);
    assert_eq!(pr.acknowledge(&p, barrier, Ack::DoneNotDurable), Ok(Outcome::Unchanged));
    assert_eq!(pr.acked(), barrier);
    assert_eq!(pr.next(&p), None);
    assert_eq!(pr.acknowledge(&p, barrier, Ack::Done), Err(OrderError::Stopped));
    assert!(!pr.succeeded(&p));
}

/// Catches a failed install being reported as unchanged, which would let cancel promise old history.
#[test]
fn install_failure_is_never_nothing_written() {
    let p = ok(&[]);
    let install = position(&p, Effect::InstallTarget) as u32;
    let mut pr = progress_after(&p, install);
    let o = pr.acknowledge(&p, install, Ack::Failed).unwrap();
    assert_eq!(o, Outcome::ReplacementInDoubt);
    assert!(!o.nothing_written() && !o.may_cancel());
}

/// Catches a restart resuming a transaction whose barrier was never acknowledged.
#[test]
fn resume_only_after_barrier() {
    let p = ok(&[]);
    let barrier = position(&p, Effect::PersistBarrier) as u32;
    for acked in 0..=barrier {
        assert_eq!(Progress::resume(&p, acked), Err(OrderError::NotResumable));
    }
    let r = Progress::resume(&p, barrier + 1).unwrap();
    assert_eq!(r.acked(), barrier + 1);
    assert_eq!(r.next(&p), Some(p.steps()[barrier as usize + 1]));
    assert!(Progress::resume(&p, p.steps().len() as u32).is_ok());
    assert_eq!(Progress::resume(&p, p.steps().len() as u32 + 1), Err(OrderError::NotResumable));
}

/// Catches a signed original being rewritten in place, or a Save As touching the source it keeps.
#[test]
fn signed_original_refuses_save_and_save_as_keeps_it() {
    assert_eq!(make(TargetMode::Save, true, &[], &DESIGN_LIMITS).unwrap_err(), Refusal::SignedOriginal);
    let p = make(TargetMode::SaveAs, true, &[art(1, Store::SourcePdf, Owner::Unrelated)], &DESIGN_LIMITS);
    assert_eq!(p.unwrap_err(), Refusal::Inconsistent);
    let p = make(TargetMode::SaveAs, true, &[], &DESIGN_LIMITS).unwrap();
    assert!(p.unsigned_copy_notice());
    assert_eq!(p.line(Store::SourcePdf).disposition, Disposition::KeptAsSource);
    assert!(
        p.steps()
            .iter()
            .all(|s| !matches!(s.effect(), Effect::Purge(Store::SourcePdf) | Effect::Rewrite(Store::SourcePdf)))
    );
    assert!(!ok(&[]).unsigned_copy_notice());
    assert!(!make(TargetMode::SaveAs, false, &[], &DESIGN_LIMITS).unwrap().unsigned_copy_notice());
}

/// Catches a Save As source being deletable by a plan that replaces in place.
#[test]
fn save_as_source_owner_in_save_mode_is_inconsistent() {
    let got = make(TargetMode::Save, false, &[art(1, Store::Backup, Owner::SaveAsSource)], &DESIGN_LIMITS);
    assert_eq!(got.unwrap_err(), Refusal::Inconsistent);
    assert!(make(TargetMode::SaveAs, false, &[art(1, Store::Backup, Owner::SaveAsSource)], &DESIGN_LIMITS).is_ok());
}

/// Catches an adapter claiming material in a store removal does not act on.
#[test]
fn artifact_in_fixed_store_is_inconsistent() {
    for store in Store::ALL.into_iter().filter(|s| !s.enumerated()) {
        let got = make(TargetMode::Save, false, &[art(1, store, Owner::Target)], &DESIGN_LIMITS);
        assert_eq!(got.unwrap_err(), Refusal::Inconsistent, "{store:?}");
    }
}

/// Catches one id acting on, or hiding, two artifacts.
#[test]
fn duplicate_artifact_id_is_inconsistent() {
    let inv = [
        art(1, Store::Journal, Owner::Target),
        art(2, Store::Backup, Owner::Unrelated),
        art(1, Store::Backup, Owner::Unrelated),
    ];
    assert_eq!(make(TargetMode::Save, false, &inv, &DESIGN_LIMITS).unwrap_err(), Refusal::Inconsistent);
}

/// Catches target material in a `.recto` library being planned though nothing can compact it.
#[test]
fn library_material_refuses_unsupported() {
    let got = make(TargetMode::Save, false, &[art(1, Store::Library, Owner::Target)], &DESIGN_LIMITS);
    assert_eq!(got.unwrap_err(), Refusal::Unsupported(Store::Library));
    let p = ok(&[art(1, Store::Library, Owner::Unrelated)]);
    assert_eq!(p.line(Store::Library).disposition, Disposition::NoneFound);
}

/// Catches a container being rewritten blind in a format this build cannot read, and a whole
/// foreign-format file proven the target's being left behind.
#[test]
fn foreign_container_refuses_but_foreign_whole_file_purges() {
    let foreign = |store| Artifact {
        format: Format::Foreign,
        ..art(1, store, Owner::Target)
    };
    let got = make(TargetMode::Save, false, &[foreign(Store::SessionRecord)], &DESIGN_LIMITS);
    assert_eq!(got.unwrap_err(), Refusal::Unsupported(Store::SessionRecord));
    let p = ok(&[foreign(Store::Journal)]);
    assert!(effects(&p).contains(&Effect::Purge(Store::Journal)));
    let unrelated = Artifact {
        format: Format::Foreign,
        ..art(1, Store::SessionRecord, Owner::Unrelated)
    };
    assert!(make(TargetMode::Save, false, &[unrelated], &DESIGN_LIMITS).is_ok());
}

/// Catches an empty request, or one with a kind this build does not know, being accepted.
#[test]
fn kinds_reject_empty_and_unknown_bits() {
    assert_eq!(Kinds::from_bits(0), Err(Refusal::NothingSelected));
    assert_eq!(Kinds::from_bits(8), Err(Refusal::UnknownKind));
    assert_eq!(Kinds::from_bits(0x81), Err(Refusal::UnknownKind));
    let all = Kinds::from_bits(7).unwrap();
    assert!(all.contains(Kinds::ACTIONS) && all.contains(Kinds::METADATA));
    assert!(!Kinds::from_bits(1).unwrap().contains(Kinds::ATTACHMENTS));
}

/// Catches progress made for one plan advancing, or answering outcomes for, a plan of another length.
#[test]
fn progress_from_another_plan_is_refused() {
    let small = ok(&[]);
    let big = ok(&[art(1, Store::Journal, Owner::Target)]);
    let mut pr = Progress::start(&small);
    assert_eq!(pr.acknowledge(&big, 0, Ack::Done), Err(OrderError::WrongPlan));
    assert_eq!(pr.next(&big), None);
    assert!(!pr.succeeded(&big));
}

/// Pins today's sizes so a field added to a capped value is seen, not slipped in.
#[test]
fn values_stay_small() {
    assert_eq!(std::mem::size_of::<Artifact>(), 8);
    assert_eq!(std::mem::size_of::<Step>(), 8);
    assert_eq!(std::mem::size_of::<Store>(), 1);
}
