//! The transaction record's decoder against a record that lies about how many artifacts it holds, counted in heap bytes (its own binary, with a counting
//! allocator: the process footprint can't tell what was asked for from what was freed on this macOS).
//!
//! Failure caught: `decode_barrier_within` sizes its vectors from the record's own claimed count before checking the claim against the cap and against the bytes
//! there are. A hostile record, re-signed so its checksum holds (anyone who can write the file can), then asks the allocator for gigabytes in the app's
//! startup path (the gate decodes the record at every launch), which is rule 6: no allocation from a count the input made.

use recto_core::removal::{
    Ack, Actor, ArtifactKey, BarrierFacts, Binding, CONFIRM_LIMITS, Class, Confirmations, Decoded, Effect, FileVersion, Format, Found, Kinds, LEDGER_LIMITS,
    Limits, MAX_BARRIER_BYTES, Owner, Pin, PinKind, PlanId, Progress, Request, Store, Survey, TargetKey, TargetMode, Version, decode_barrier_within,
    encode_barrier_within, encode_header, lineage, plan, read_ledger,
};
use recto_core::save::Target;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Instant;

#[allow(unsafe_code)]
mod counted {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Counting;

    thread_local! {
        static ON: Cell<bool> = const { Cell::new(false) };
        static LARGEST: Cell<usize> = const { Cell::new(0) };
    }

    fn asked(size: usize) {
        // A thread on its way out has no counter any more.
        let _ = ON.try_with(|on| {
            if on.get() {
                LARGEST.with(|largest| largest.set(largest.get().max(size)));
            }
        });
    }

    // SAFETY: forwards to the system allocator, noting the largest single request on the thread that makes it while counting is on.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            asked(layout.size());
            // SAFETY: the caller's layout, passed on.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            asked(layout.size());
            // SAFETY: the caller's layout, passed on.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            asked(new_size);
            // SAFETY: the caller's block and sizes, passed on.
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: given back as it was given.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    /// The largest single allocation `work` asked for on this thread.
    pub fn largest_request(work: impl FnOnce()) -> usize {
        LARGEST.with(|largest| largest.set(0));
        ON.with(|on| on.set(true));
        work();
        ON.with(|on| on.set(false));
        LARGEST.with(Cell::get)
    }
}

fn version(inode: u64) -> FileVersion {
    FileVersion {
        volume: Some([3; 16]),
        inode,
        size: 10,
        mtime_ns: 1,
    }
}

/// A real record's bytes, at its barrier, and how many artifacts it holds.
fn record() -> (Vec<u8>, u32) {
    let limits = Limits {
        max_artifacts: 8,
        max_batch: std::num::NonZeroU16::new(4).unwrap(),
    };
    let mut survey = Survey::new(&limits);
    for store in Store::ALL.into_iter().filter(|s| s.enumerated()) {
        survey.mark_complete(store);
    }
    let ledger = read_ledger(&encode_header(1_000), &LEDGER_LIMITS);
    survey.history(&lineage(&ledger, version(5), Path::new("/d/a.pdf"), &LEDGER_LIMITS).unwrap());
    for (store, name, owner) in [
        (Store::Journal, "j1", Owner::Target),
        (Store::WriterTemp, "t1", Owner::Target),
        (Store::SessionRecord, "s1", Owner::Target),
        (Store::Journal, "other", Owner::Unrelated),
    ] {
        survey
            .add(Found {
                store,
                key: ArtifactKey::of(store, name.as_bytes()),
                owner,
                format: Format::Current,
            })
            .unwrap();
    }
    let surveyed = survey.finish().unwrap();
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), TargetMode::Save, false);
    let plan = plan(&request, &surveyed.inventory(), &limits).unwrap();
    let count = surveyed.inventory().artifacts().len() as u32;
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
    let mut confirmations = Confirmations::new(CONFIRM_LIMITS);
    let binding = Binding {
        actor: 0u8,
        version: Version {
            epoch: 1,
            source: recto_platform::FileStamp {
                size: 1,
                mtime_ns: 2,
                device: 3,
                inode: 4,
            },
        },
        target: TargetKey::new(Path::new("/doc.pdf"), Target::Same),
    };
    let now = Instant::now();
    let id = confirmations.offer(binding, &plan, now, PlanId::from_raw(9).unwrap()).unwrap();
    let confirmed = confirmations.take(id, &binding, &plan, now).unwrap();
    let mut progress = Progress::start(&plan, confirmed).unwrap();
    let base = plan.steps().iter().position(|s| s.effect() == Effect::PersistBarrier).unwrap() as u32;
    for i in 0..base {
        progress.acknowledge(&plan, i, Ack::Done).unwrap();
    }
    let encoded = encode_barrier_within(&plan, &surveyed, &limits, &progress, &facts, MAX_BARRIER_BYTES).unwrap();
    (encoded.bytes, count)
}

/// One artifact's bytes in the record's body (`recto_core::removal`'s own constant is private; the check below fails loudly if it moves).
const ARTIFACT_LEN: usize = 49;

/// The record with its claimed artifact count changed and its checksum made again, so only the count is a lie.
fn claiming(real: &[u8], count: u32, claimed: u32) -> Vec<u8> {
    let mut bytes = real.to_vec();
    let body_len = u32::from_le_bytes(bytes[20..24].try_into().unwrap()) as usize;
    let at = body_len - 32 - count as usize * ARTIFACT_LEN - 4;
    assert_eq!(
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()),
        count,
        "the count is where the constant says"
    );
    bytes[at..at + 4].copy_from_slice(&claimed.to_le_bytes());
    let sha: [u8; 32] = Sha256::digest(&bytes[..body_len - 32]).into();
    bytes[body_len - 32..body_len].copy_from_slice(&sha);
    bytes
}

#[test]
fn a_record_claiming_more_artifacts_than_it_holds_asks_for_no_memory_for_them() {
    let (real, count) = record();
    assert!(
        matches!(decode_barrier_within(&real, MAX_BARRIER_BYTES), Decoded::Pending(_)),
        "the real one decodes"
    );
    for claimed in [u32::MAX, 1 << 24, 9] {
        let bytes = claiming(&real, count, claimed);
        let mut decoded = None;
        let largest = counted::largest_request(|| decoded = Some(decode_barrier_within(&bytes, MAX_BARRIER_BYTES)));
        assert!(matches!(decoded, Some(Decoded::Damaged)), "claimed {claimed}: {decoded:?}");
        assert!(
            largest <= 64 * 1024 + bytes.len(),
            "claimed {claimed}: the largest single request was {largest} bytes for a record of {}",
            bytes.len()
        );
    }
}
