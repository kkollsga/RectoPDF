//! A hostile transaction record (`<state>/removal/pending.v1`, `recto_core::removal::decode_barrier`): whatever the bytes, decoding never panics,
//! a `Pending` rebuilds a plan whose digest is the one the record carries, its count and layout fit the file and its plan, it decodes the same twice,
//! and a record made by `encode_barrier` decodes to what it was made from. Records with the checksum made good are tried too, so a changed body gets
//! past it.

use recto_core::removal::{
    Ack, Actor, ArtifactKey, BarrierFacts, Binding, CONFIRM_LIMITS, Class, Confirmations, Decoded, Effect, FileVersion, Format, Found, Kinds, LEDGER_LIMITS,
    Limits, Owner, Pin, PinKind, PlanId, Progress, Request, Store, Survey, TargetKey, TargetMode, Version, decode_barrier, decode_barrier_within,
    encode_barrier, encode_header, lineage, plan, read_ledger,
};
use recto_core::save::Target;
use recto_platform::FileStamp;
use sha2::{Digest, Sha256};
use std::num::NonZeroU16;
use std::path::Path;
use std::time::Instant;

const SMALL: usize = 600;

fn version(inode: u64) -> FileVersion {
    FileVersion {
        volume: Some([1; 16]),
        inode,
        size: 10,
        mtime_ns: 3,
    }
}

/// A record of a removal of `n` journals and a temp and a session record, walked to the barrier, with `slots` of its slots written.
fn record(n: u8, slots: u32) -> Vec<u8> {
    let limits = Limits {
        max_artifacts: u32::from(n) + 4,
        max_batch: NonZeroU16::new(2).expect("not zero"),
    };
    let mut survey = Survey::new(&limits);
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        survey.mark_complete(store);
    }
    let ledger = read_ledger(&encode_header(1_000), &LEDGER_LIMITS);
    survey.history(&lineage(&ledger, version(5), Path::new("/d/a.pdf"), &LEDGER_LIMITS).expect("a lineage"));
    let mut add = |store: Store, name: String, owner: Owner| {
        survey
            .add(Found {
                store,
                key: ArtifactKey::of(store, name.as_bytes()),
                owner,
                format: Format::Current,
            })
            .expect("under the limit");
    };
    for i in 0..n {
        add(Store::Journal, format!("j{i}"), Owner::Target);
    }
    add(Store::WriterTemp, "t".into(), Owner::Target);
    add(Store::SessionRecord, "s".into(), Owner::Target);
    add(Store::Journal, "other".into(), Owner::Unrelated);
    let surveyed = survey.finish().expect("a survey");
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).expect("a kind"), TargetMode::Save, false);
    let p = plan(&request, &surveyed.inventory(), &limits).expect("a plan");
    let mut confirmations = Confirmations::new(CONFIRM_LIMITS);
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
    let id = confirmations.offer(binding, &p, now, PlanId::from_raw(9).expect("not zero")).expect("offered");
    let confirmed = confirmations.take(id, &binding, &p, now).expect("taken");
    let mut progress = Progress::start(&p, confirmed).expect("its own plan");
    let barrier = p.steps().iter().position(|s| s.effect() == Effect::PersistBarrier).expect("a barrier");
    for i in 0..barrier {
        progress.acknowledge(&p, i as u32, Ack::Done).expect("in order");
    }
    let pins = surveyed
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
        .collect();
    let facts = BarrierFacts {
        target: "/d/a.pdf".into(),
        source: version(5),
        candidate_name: ".a.pdf.new-1".into(),
        candidate: version(6),
        candidate_sha: [9; 32],
        roots: vec![[1; 16]],
        actor: Actor::Person,
        pins,
    };
    let encoded = encode_barrier(&p, &surveyed, &limits, &progress, &facts).expect("a record");
    let mut bytes = encoded.bytes;
    for k in 0..slots {
        if let Some((at, slot)) = encoded.layout.slot(encoded.layout.base() + 1 + k) {
            bytes[at as usize..at as usize + 32].copy_from_slice(&slot);
        }
    }
    bytes
}

/// Valid records: none to a few artifacts, no slot, one, a gap and all.
pub fn seeds() -> Vec<Vec<u8>> {
    vec![record(0, 0), record(1, 1), record(3, 2), record(2, 5), record(6, 0)]
}

/// The record's checksum made right for whatever its header says, so a changed body is read.
fn resigned(data: &[u8]) -> Option<Vec<u8>> {
    let body_len = u32::from_le_bytes(data.get(20..24)?.try_into().ok()?) as usize;
    if body_len < 64 || body_len > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    let sha: [u8; 32] = Sha256::digest(&out[..body_len - 32]).into();
    out[body_len - 32..body_len].copy_from_slice(&sha);
    Some(out)
}

fn read(data: &[u8]) {
    for decoded in [decode_barrier(data), decode_barrier_within(data, SMALL)] {
        let Decoded::Pending(rec) = decoded else { continue };
        assert!(data.len() <= SMALL.max(2 << 20));
        assert_eq!(
            rec.plan().digest().as_bytes(),
            &data[42..74],
            "a pending record rebuilt another plan than it recorded"
        );
        let base = rec.layout().base();
        assert!(
            rec.acked() >= base && (rec.acked() as usize) < rec.plan().steps().len(),
            "a count outside the plan"
        );
        assert_eq!(rec.layout().len(), data.len() as u64, "the layout does not fit the file");
        assert_eq!(rec.resume().is_ok(), rec.acked() > base, "resumable only past the barrier");
        let Decoded::Pending(again) = decode_barrier_within(data, data.len()) else {
            panic!("a record read once was not read again");
        };
        assert_eq!((again.acked(), again.body_sha()), (rec.acked(), rec.body_sha()));
    }
}

/// The check libFuzzer, the smoke run and the regression tests share.
pub fn check(data: &[u8]) {
    read(data);
    if let Some(good) = resigned(data) {
        read(&good);
    }
    // A made record is one that reads, to what it was made from.
    if let Some(&n) = data.first() {
        let bytes = record(n % 12, u32::from(data.get(1).copied().unwrap_or(0) % 6));
        let Decoded::Pending(rec) = decode_barrier(&bytes) else {
            panic!("a record made by the encoder did not read");
        };
        assert_eq!(rec.facts().target, Path::new("/d/a.pdf"));
    }
}
