//! What the gate costs at every launch: the gate (`gate::anything_there`, which is `gate::at`'s yes or no) with nothing there, the gate with a pending record at the survey's cap of 8,192 artifacts, the
//! updater's look, and the live heap bytes once the gate has answered (a counting allocator: this macOS keeps a freed block in the process footprint).
//! Ignored: a measurement, run on a release build with `cargo test --release -p recto-control --test gate_cost -- --ignored --nocapture`, and the load
//! said with the numbers. One assertion holds in every run: nothing is held after the gate returns (rule 1).
#![cfg(target_os = "macos")]

use recto_control::removal_store::barrier::fixture::rig_at_the_cap;
use recto_control::removal_store::gate;
use recto_testpdf::TempDir;
use std::time::{Duration, Instant};

#[allow(unsafe_code)]
mod live {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicIsize, Ordering};

    struct Counting;

    static LIVE: AtomicIsize = AtomicIsize::new(0);

    // SAFETY: forwards to the system allocator, counting live bytes in one atomic.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
            // SAFETY: the caller's layout, passed on.
            unsafe { System.alloc(layout) }
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
            // SAFETY: the caller's layout, passed on.
            unsafe { System.alloc_zeroed(layout) }
        }

        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
            // SAFETY: the caller's block and sizes, passed on.
            unsafe { System.realloc(ptr, layout, new_size) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
            // SAFETY: given back as it was given.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    pub fn now() -> isize {
        LIVE.load(Ordering::Relaxed)
    }
}

fn median(mut runs: Vec<Duration>) -> Duration {
    runs.sort();
    runs[runs.len() / 2]
}

fn time(mut work: impl FnMut()) -> Duration {
    median(
        (0..9)
            .map(|_| {
                let began = Instant::now();
                for _ in 0..20 {
                    work();
                }
                began.elapsed() / 20
            })
            .collect(),
    )
}

#[test]
#[ignore = "a measurement: release build, load said with the numbers"]
fn what_the_gate_costs() {
    let empty = TempDir::new("recto-gate-cost-empty");
    let held = TempDir::new("recto-gate-cost-pending");
    let r = rig_at_the_cap(8_188);
    {
        let (_held, _progress) = r.begin_held(held.path());
    }
    let size = std::fs::metadata(held.path().join("removal/pending.v1")).unwrap().len();

    let before = live::now();
    let nothing = gate::anything_there(empty.path());
    let pending = gate::anything_there(held.path());
    let after = live::now();
    assert_eq!((nothing, pending), (false, true));
    assert_eq!(after, before, "live heap bytes held by the gate after it returned");

    println!("record: {size} bytes, {} artifacts", r.surveyed.inventory().artifacts().len());
    println!(
        "gate (anything_there), nothing there: {:?}",
        time(|| std::hint::black_box(gate::anything_there(empty.path())))
    );
    println!(
        "gate (anything_there), pending at the cap: {:?}",
        time(|| std::hint::black_box(gate::anything_there(held.path())))
    );
    println!(
        "anything_there (the updater's look), nothing there: {:?}",
        time(|| std::hint::black_box(recto_update::removal_record::anything_there(empty.path())))
    );
    println!(
        "anything_there (the updater's look), a record there: {:?}",
        time(|| std::hint::black_box(recto_update::removal_record::anything_there(held.path())))
    );
}
