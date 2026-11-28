//! The generator's hold, in process: the runners are injected, and every ordering is forced by a gate, never slept for.

use super::super::*;
use std::sync::atomic::AtomicU32;

fn key(number: u8) -> CorpusKey {
    CorpusKey::from_hex(&format!("{number:02x}").repeat(32)).unwrap()
}

/// Waits, up to 10 s, for `what` to be true, and fails with `saw` when it isn't.
fn wait_for(what: impl Fn() -> bool, saw: impl Fn() -> String) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !what() {
        assert!(Instant::now() < deadline, "{}", saw());
        std::thread::yield_now();
    }
}

/// Counts how many contents jobs it was given, and makes nothing.
struct Counts(Arc<AtomicU32>);

impl Runner for Counts {
    fn run(&self, _job: &GenJob, _stop: &AtomicBool) -> (Outcome, Value) {
        self.0.fetch_add(1, Ordering::SeqCst);
        (Outcome::Changed, Value::Null)
    }
}

/// Failure: a panic outside the job's own `catch_unwind` (here the notifier's) ends the queue's thread with `working` still true, so no ask starts a thread again for the rest of the session, and a hold would wait out its whole time. After the thread dies, the next ask runs.
#[test]
fn a_queue_thread_that_panics_outside_its_job_lets_the_next_ask_run() {
    let runs = Arc::new(AtomicU32::new(0));
    let generator = Generator::with_runner(Path::new("/nowhere"), Box::new(Counts(Arc::clone(&runs))));
    let first = Arc::new(AtomicBool::new(true));
    generator.attach(Arc::new(move |_events| {
        if first.swap(false, Ordering::SeqCst) {
            panic!("a notifier that fails once");
        }
    }));
    generator.want(Path::new("/nowhere/1.pdf"), key(1), 3);
    wait_for(|| !generator.0.lock().working, || "the thread that panicked is still counted as working".into());
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "the job whose start was announced ran after the notifier panicked"
    );
    generator.want(Path::new("/nowhere/1.pdf"), key(1), 3);
    wait_for(
        || runs.load(Ordering::SeqCst) == 1,
        || format!("the next ask didn't run: {} runs", runs.load(Ordering::SeqCst)),
    );
}
