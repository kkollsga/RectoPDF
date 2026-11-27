use super::*;
use crate::removal::tests::{confirmed, request};
use std::num::NonZeroU16;
use std::path::Path;

pub(super) fn small(max: u32) -> Limits {
    Limits {
        max_artifacts: max,
        max_batch: NonZeroU16::new(2).unwrap(),
    }
}

fn found(store: Store, name: &str, owner: Owner) -> Found {
    Found {
        store,
        key: ArtifactKey::of(store, name.as_bytes()),
        owner,
        format: Format::Current,
    }
}

fn version(inode: u64) -> FileVersion {
    FileVersion {
        volume: Some([1; 16]),
        inode,
        size: 10,
        mtime_ns: 3,
    }
}

/// A survey of a whole history and every store searched, holding `items`.
fn survey(items: &[Found], limits: &Limits) -> Surveyed {
    let mut survey = Survey::new(limits);
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        survey.mark_complete(store);
    }
    let ledger = read_ledger(&encode_header(1_000), &LEDGER_LIMITS);
    let lineage = lineage(&ledger, version(5), Path::new("/d/a.pdf"), &LEDGER_LIMITS).unwrap();
    survey.history(&lineage);
    for f in items {
        survey.add(*f).unwrap();
    }
    survey.finish().unwrap()
}

/// What a record needs for each artifact: a pin exactly where the target's file is acted on.
pub(super) fn pins_for(surveyed: &Surveyed) -> Vec<Option<Pin>> {
    surveyed
        .inventory()
        .artifacts()
        .iter()
        .map(|a| {
            (a.owner == Owner::Target && matches!(a.store.class(), Class::Purge | Class::Rewrite | Class::Trash)).then_some(Pin {
                root: 0,
                inode: 100 + u64::from(a.id.0),
                kind: PinKind::File,
            })
        })
        .collect()
}

pub(super) fn facts_for(surveyed: &Surveyed) -> BarrierFacts {
    BarrierFacts {
        target: "/d/a.pdf".into(),
        source: version(5),
        candidate_name: ".a.pdf.new-1".into(),
        candidate: version(6),
        candidate_sha: [9; 32],
        roots: vec![[1; 16]],
        actor: Actor::Person,
        pins: pins_for(surveyed),
    }
}

pub(super) struct Rig {
    pub surveyed: Surveyed,
    pub limits: Limits,
    pub plan: Plan,
    pub progress: Progress,
    pub facts: BarrierFacts,
}

/// A removal of a journal, a writer's temporary and a session record, with another document's journal beside them, walked to the barrier.
pub(super) fn rig() -> Rig {
    rig_of(&[
        found(Store::Journal, "j1", Owner::Target),
        found(Store::WriterTemp, "t1", Owner::Target),
        found(Store::SessionRecord, "s1", Owner::Target),
        found(Store::Journal, "other", Owner::Unrelated),
    ])
}

pub(super) fn rig_of(items: &[Found]) -> Rig {
    let limits = small(items.len().max(4) as u32);
    let surveyed = survey(items, &limits);
    let plan = super::super::plan(&request(TargetMode::Save, false), &surveyed.inventory(), &limits).unwrap();
    let mut progress = Progress::start(&plan, confirmed(&plan)).unwrap();
    for i in 0..index_of(&plan, Effect::PersistBarrier) {
        progress.acknowledge(&plan, i, Ack::Done).unwrap();
    }
    let facts = facts_for(&surveyed);
    Rig {
        surveyed,
        limits,
        plan,
        progress,
        facts,
    }
}

impl Rig {
    pub(super) fn encode(&self) -> Result<Encoded, BarrierRefusal> {
        encode_barrier(&self.plan, &self.surveyed, &self.limits, &self.progress, &self.facts)
    }
}

fn pending(bytes: &[u8]) -> Box<Recorded> {
    match decode_barrier(bytes) {
        Decoded::Pending(r) => r,
        other => panic!("expected a pending record, got {other:?}"),
    }
}

/// Puts the sha of a patched record right again, so a test reaches the checks after it.
fn resign(bytes: &mut [u8]) {
    let body_len = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
    let sha: [u8; 32] = Sha256::digest(&bytes[..body_len - 32]).into();
    bytes[body_len - 32..body_len].copy_from_slice(&sha);
}

fn put_slot(bytes: &mut [u8], layout: &Layout, acked: u32) {
    let (at, slot) = layout.slot(acked).unwrap();
    bytes[at as usize..at as usize + 32].copy_from_slice(&slot);
}

/// Catches a record that loses part of the plan: what is decoded must be the plan that was encoded, step for step, and walk on from the next one.
#[test]
fn it_round_trips_and_resumes_at_the_next_step() {
    let r = rig();
    let mut e = r.encode().unwrap();
    assert_eq!(e.bytes.len() as u64, e.layout.len());
    let base = e.layout.base();
    assert_eq!(base, index_of(&r.plan, Effect::PersistBarrier));
    let rec = pending(&e.bytes);
    assert_eq!(rec.acked(), base, "no slot is written yet");
    assert_eq!(rec.outcome(), Outcome::Unchanged);
    assert_eq!(rec.resume().unwrap_err(), OrderError::NotResumable);
    assert_eq!(rec.facts(), &r.facts);
    assert_eq!(rec.plan().digest(), r.plan.digest());
    assert_eq!(rec.plan().steps(), r.plan.steps());
    assert_eq!(rec.layout(), e.layout);

    put_slot(&mut e.bytes, &e.layout, base + 1);
    let rec = pending(&e.bytes);
    assert_eq!(rec.acked(), base + 1);
    let (plan, progress) = rec.resume().unwrap();
    assert_eq!(progress.next(&plan), Some(r.plan.steps()[base as usize + 1]));
    assert_eq!(rec.outcome(), Outcome::HistoryRetiring);
}

/// Pins the bytes of a small record, so a change to the format or to what it holds is seen and decided.
#[test]
fn a_small_record_has_this_digest() {
    let e = rig().encode().unwrap();
    let sha: String = Sha256::digest(&e.bytes).iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(sha, GOLDEN_RECORD, "len {}", e.bytes.len());
}

const GOLDEN_RECORD: &str = "78f22a4b008047b50f33c86d6e333eb8c43b003ee9990f19b472c0588ba472c0";

/// Catches a damaged record being taken for another: any one byte changed, and any cut, is never a panic, never a different plan and never a
/// higher acknowledged count than the record had.
#[test]
fn no_flip_or_cut_gives_a_higher_count_or_another_plan() {
    let r = rig();
    let mut e = r.encode().unwrap();
    let base = e.layout.base();
    put_slot(&mut e.bytes, &e.layout, base + 1);
    put_slot(&mut e.bytes, &e.layout, base + 2);
    let whole = e.bytes.clone();
    for at in 0..whole.len() {
        for mask in [0x01u8, 0xFF] {
            let mut bytes = whole.clone();
            bytes[at] ^= mask;
            if let Decoded::Pending(rec) = decode_barrier(&bytes) {
                assert!(rec.acked() <= base + 2, "flip at {at}");
                assert_eq!(rec.plan().digest(), r.plan.digest(), "flip at {at}");
            }
        }
    }
    for len in 0..whole.len() {
        assert!(!matches!(decode_barrier(&whole[..len]), Decoded::Pending(_)), "cut at {len}");
    }
    assert!(matches!(decode_barrier(&whole), Decoded::Pending(_)));
}

/// Catches a later version's record being read, or called damaged and cleared.
#[test]
fn a_later_version_is_too_new_and_a_bad_magic_is_damaged() {
    let mut bytes = rig().encode().unwrap().bytes;
    bytes[16..20].copy_from_slice(&2u32.to_le_bytes());
    assert!(matches!(decode_barrier(&bytes), Decoded::TooNew));
    bytes[16..20].copy_from_slice(&0u32.to_le_bytes());
    assert!(matches!(decode_barrier(&bytes), Decoded::Damaged));
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    bytes[0] ^= 1;
    assert!(matches!(decode_barrier(&bytes), Decoded::Damaged));
}

/// Catches the total cap being off by one, in the encoder and in the decoder.
#[test]
fn the_total_size_cap_holds_at_the_limit_and_one_over() {
    let r = rig();
    let len = r.encode().unwrap().bytes.len();
    let at = |cap| encode_barrier_within(&r.plan, &r.surveyed, &r.limits, &r.progress, &r.facts, cap);
    assert!(at(len).is_ok());
    assert_eq!(at(len - 1).unwrap_err(), BarrierRefusal::TooLarge);
    let bytes = r.encode().unwrap().bytes;
    assert!(matches!(decode_barrier_within(&bytes, len), Decoded::Pending(_)));
    assert!(matches!(decode_barrier_within(&bytes, len - 1), Decoded::Damaged));
}

/// Catches a path, a name or a set of roots over its cap being written (and then refused when read, with a transaction already past the barrier).
#[test]
fn path_name_and_roots_caps_hold_at_the_limit_and_one_over() {
    let r = rig();
    let path = |n: usize| format!("/{}", "a".repeat(n - 1));
    for (len, ok) in [(4_096, true), (4_097, false)] {
        let mut f = r.facts.clone();
        f.target = path(len).into();
        let got = encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f);
        assert_eq!(got.is_ok(), ok, "path {len}");
        if let Ok(e) = got {
            assert!(matches!(decode_barrier(&e.bytes), Decoded::Pending(_)));
        }
    }
    for (len, ok) in [(255, true), (256, false)] {
        let mut f = r.facts.clone();
        f.candidate_name = "n".repeat(len).into();
        let got = encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f);
        assert_eq!(got.is_ok(), ok, "name {len}");
        if let Ok(e) = got {
            assert!(matches!(decode_barrier(&e.bytes), Decoded::Pending(_)));
        }
    }
    for (n, ok) in [(8u8, true), (9, false)] {
        let mut f = r.facts.clone();
        f.roots = (1..=n).map(|i| [i; 16]).collect();
        let got = encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f);
        assert_eq!(got.is_ok(), ok, "roots {n}");
        if let Ok(e) = got {
            assert!(matches!(decode_barrier(&e.bytes), Decoded::Pending(_)));
        }
    }
}

/// Catches a name that leaves its folder, an empty one, and roots that are zero or repeated.
#[test]
fn bad_names_and_roots_are_refused() {
    let r = rig();
    for name in ["", ".", "..", "a/b", "a\0b"] {
        let mut f = r.facts.clone();
        f.candidate_name = name.into();
        assert_eq!(
            encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f).unwrap_err(),
            BarrierRefusal::BadName,
            "{name:?}"
        );
    }
    for roots in [vec![[0; 16]], vec![[1; 16], [1; 16]]] {
        let mut f = r.facts.clone();
        f.roots = roots;
        assert_eq!(
            encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f).unwrap_err(),
            BarrierRefusal::BadRoots
        );
    }
    let mut f = r.facts.clone();
    f.target = "relative.pdf".into();
    assert_eq!(
        encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f).unwrap_err(),
        BarrierRefusal::BadPath
    );
}

/// Catches a target's file with no pin being recorded (a later purge would find it by name alone), and a pin on a file that isn't to be acted on.
#[test]
fn pins_sit_on_exactly_the_files_that_are_acted_on() {
    let r = rig();
    let artifacts = r.surveyed.inventory().artifacts().to_vec();
    let target = artifacts.iter().position(|a| a.owner == Owner::Target).unwrap();
    let other = artifacts.iter().position(|a| a.owner == Owner::Unrelated).unwrap();
    let mut f = r.facts.clone();
    f.pins[target] = None;
    assert_eq!(
        encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f).unwrap_err(),
        BarrierRefusal::BadPins
    );
    let mut f = r.facts.clone();
    f.pins[other] = f.pins[target];
    assert_eq!(
        encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f).unwrap_err(),
        BarrierRefusal::BadPins
    );
    let mut f = r.facts.clone();
    f.pins[target].as_mut().unwrap().root = 1;
    assert_eq!(
        encode_barrier(&r.plan, &r.surveyed, &r.limits, &r.progress, &f).unwrap_err(),
        BarrierRefusal::BadPins
    );
}

/// Catches a record for a leftovers walk, which has no barrier a restart could take up, and one for a walk that isn't at the barrier or for another plan.
#[test]
fn only_a_removal_at_its_barrier_of_its_own_plan_is_recorded() {
    let r = rig();
    let left = survey(&[found(Store::WriterTemp, "t1", Owner::Target)], &r.limits);
    let leftovers = super::super::plan(&Request::leftovers(), &left.inventory(), &r.limits).unwrap();
    let progress = Progress::start(&leftovers, confirmed(&leftovers)).unwrap();
    assert_eq!(
        encode_barrier(&leftovers, &left, &r.limits, &progress, &facts_for(&left)).unwrap_err(),
        BarrierRefusal::Leftovers
    );

    let fresh = Progress::start(&r.plan, confirmed(&r.plan)).unwrap();
    assert_eq!(
        encode_barrier(&r.plan, &r.surveyed, &r.limits, &fresh, &r.facts).unwrap_err(),
        BarrierRefusal::NotAtBarrier
    );

    // Another survey of the same size: a different file under the same counts is another plan.
    let other = rig_of(&[
        found(Store::Journal, "x1", Owner::Target),
        found(Store::WriterTemp, "x2", Owner::Target),
        found(Store::SessionRecord, "x3", Owner::Target),
        found(Store::Journal, "other", Owner::Unrelated),
    ]);
    assert_eq!(
        encode_barrier(&r.plan, &other.surveyed, &r.limits, &r.progress, &r.facts).unwrap_err(),
        BarrierRefusal::WrongPlan
    );
    assert_eq!(
        encode_barrier(&r.plan, &r.surveyed, &r.limits, &other.progress, &r.facts).unwrap_err(),
        BarrierRefusal::NotAtBarrier
    );
}

/// Catches a claimed artifact count being allocated for, and one past the cap being taken: both are damaged, not a crash or a huge buffer.
#[test]
fn a_claimed_count_is_checked_before_it_is_used() {
    let r = rig();
    let e = r.encode().unwrap();
    let body_len = u32::from_le_bytes(e.bytes[20..24].try_into().unwrap()) as usize;
    let count_at = body_len - 32 - r.surveyed.inventory().artifacts().len() * ARTIFACT_LEN - 4;
    let mut bytes = e.bytes.clone();
    bytes[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    resign(&mut bytes);
    assert!(matches!(decode_barrier(&bytes[..200.min(bytes.len())]), Decoded::Damaged));
    assert!(matches!(decode_barrier(&bytes), Decoded::Damaged));
    // One artifact over the limits the record itself states.
    let mut bytes = e.bytes.clone();
    let n = r.surveyed.inventory().artifacts().len() as u32;
    let limit_at = FIXED + 4;
    bytes[limit_at..limit_at + 4].copy_from_slice(&(n - 1).to_le_bytes());
    resign(&mut bytes);
    assert!(matches!(decode_barrier(&bytes), Decoded::Damaged));
    let mut bytes = e.bytes.clone();
    bytes[limit_at..limit_at + 4].copy_from_slice(&n.to_le_bytes());
    resign(&mut bytes);
    assert!(matches!(decode_barrier(&bytes), Decoded::Pending(_)), "at the limit it holds");
}

/// Catches a slot gap being refused (a power loss can reorder plain syncs: a later slot may be there while an earlier one isn't), and a slot
/// that is whole but in the wrong place or of another plan being trusted.
#[test]
fn slots_may_have_gaps_but_not_be_misplaced_or_foreign() {
    let r = rig();
    let e = r.encode().unwrap();
    let base = e.layout.base();
    let mut gap = e.bytes.clone();
    put_slot(&mut gap, &e.layout, base + 1);
    put_slot(&mut gap, &e.layout, base + 3);
    assert_eq!(pending(&gap).acked(), base + 3, "the highest valid slot is the count");

    let (_, slot) = e.layout.slot(base + 2).unwrap();
    let mut moved = e.bytes.clone();
    let at = e.layout.slots_at as usize;
    moved[at..at + 32].copy_from_slice(&slot);
    assert!(matches!(decode_barrier(&moved), Decoded::Damaged), "a slot for step 2 in place 0");

    let other = rig_of(&[
        found(Store::Journal, "x1", Owner::Target),
        found(Store::WriterTemp, "x2", Owner::Target),
        found(Store::SessionRecord, "x3", Owner::Target),
        found(Store::Journal, "other", Owner::Unrelated),
    ]);
    let foreign = other.encode().unwrap().layout;
    let (_, slot) = foreign.slot(base + 1).unwrap();
    let mut bytes = e.bytes.clone();
    bytes[at..at + 32].copy_from_slice(&slot);
    assert!(matches!(decode_barrier(&bytes), Decoded::Damaged), "a slot of another plan");

    let mut torn = e.bytes.clone();
    put_slot(&mut torn, &e.layout, base + 1);
    torn[at + 9] ^= 0xFF;
    assert_eq!(pending(&torn).acked(), base, "a torn slot is ignored");
}

/// Catches the sizes the memory rule counts growing unseen.
#[test]
fn the_values_are_small() {
    assert_eq!(std::mem::size_of::<Checkpoint>(), 36);
    assert_eq!(std::mem::size_of::<Pin>(), 16);
    assert_eq!(std::mem::size_of::<Layout>(), 28);
}

/// Catches a record at the design caps passing the file's cap: 8,192 artifacts in steps of 128 must fit.
#[test]
fn a_record_at_the_design_limits_is_under_the_cap() {
    let items: Vec<Found> = (0..8_192).map(|i| found(Store::Journal, &format!("j{i}"), Owner::Target)).collect();
    let limits = DESIGN_LIMITS;
    let surveyed = survey(&items, &limits);
    let plan = super::super::plan(&request(TargetMode::Save, false), &surveyed.inventory(), &limits).unwrap();
    let mut progress = Progress::start(&plan, confirmed(&plan)).unwrap();
    for i in 0..index_of(&plan, Effect::PersistBarrier) {
        progress.acknowledge(&plan, i, Ack::Done).unwrap();
    }
    let facts = facts_for(&surveyed);
    let e = encode_barrier(&plan, &surveyed, &limits, &progress, &facts).unwrap();
    assert!(e.bytes.len() < MAX_BARRIER_BYTES / 2, "{} bytes", e.bytes.len());
    assert!(matches!(decode_barrier(&e.bytes), Decoded::Pending(_)));
}
