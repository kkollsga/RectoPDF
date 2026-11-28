//! The generator's hold, in process: the runners are injected, and every ordering is forced by a gate, never slept for.

use super::super::*;
use crate::indexing::NotQuiet;
use recto_agent::blocks::BlocksAsk;
use recto_index::generated::GeneratedEntry;
use std::sync::Condvar;
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

fn must_succeed() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn sample() -> Generated {
    Generated {
        pages: 3,
        entries: vec![GeneratedEntry {
            depth: 0,
            page: 1,
            place: 10,
            title: "One".into(),
        }],
        ..Generated::default()
    }
}

fn blocks_ask(path: &Path, number: u8) -> BlocksAsk {
    BlocksAsk {
        path: path.into(),
        key: key(number),
        corpus_root: "/nowhere".into(),
        grant: recto_index::seal::grant_for(&recto_platform::find_key::FindKey::for_tests(1), key(number)).unwrap(),
    }
}

/// Opens once, for whichever thread is parked on it.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    moved: Condvar,
    entered: AtomicBool,
}

impl Gate {
    fn wait(&self) {
        self.entered.store(true, Ordering::SeqCst);
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.moved.wait(open).unwrap();
        }
    }

    fn let_through(&self) {
        *self.open.lock().unwrap() = true;
        self.moved.notify_all();
    }
}

/// Stands in for a walled helper that ignores its stop: the blocks job parks until the gate opens.
struct ParkedBlocks(Arc<Gate>);

impl blocking::Runner for ParkedBlocks {
    fn run(&self, _job: &BlocksAsk, _stop: &AtomicBool, begun: &dyn Fn()) -> (blocking::Outcome, Value) {
        begun();
        self.0.wait();
        (blocking::Outcome::AlreadyMade(Default::default()), json!({}))
    }
}

/// Stands in for a helper that made contents.
struct MakesContents;

impl Runner for MakesContents {
    fn run(&self, _job: &GenJob, _stop: &AtomicBool) -> (Outcome, Value) {
        (Outcome::Ready(Box::new(sample())), json!({}))
    }
}

/// Failure: a hold that can be taken twice lets two walks each think it's the one that's quiet, or a dropped hold stays on, so no contents or blocks are made again until the app restarts. A second hold is refused, dropping lets asks be taken again, and the drop starts nothing by itself.
#[test]
fn dropping_the_hold_lets_asks_run_again_and_a_second_hold_is_refused() {
    let runs = Arc::new(AtomicU32::new(0));
    let generator = Generator::with_runner(Path::new("/nowhere"), Box::new(Counts(Arc::clone(&runs))));
    let hold = generator.hold_for_removal(must_succeed()).unwrap();
    assert_eq!(generator.hold_for_removal(must_succeed()).unwrap_err(), NotQuiet::AlreadyHeld);
    generator.want(Path::new("/nowhere/1.pdf"), key(1), 3);
    assert_eq!(generator.state(key(1)), GenState::Idle);
    drop(hold);
    assert!(!generator.0.lock().working, "dropping the hold started a thread");
    generator.want(Path::new("/nowhere/1.pdf"), key(1), 3);
    wait_for(|| runs.load(Ordering::SeqCst) == 1, || "the ask after the hold didn't run".into());
}

/// Failure: the hold returns as soon as the stop is set, and a contents job that has ended but not yet written its result writes it after the walk has gone on to delete the contents (the stop is read before the write, so one set in between doesn't stop it). The hold returns only once the thread has finished, the write included.
#[test]
fn a_contents_job_between_its_end_and_its_write_keeps_the_hold() {
    let temp = recto_testpdf::TempPdf::new("held.pdf", &recto_testpdf::text_lines(&["A line of text"]));
    let (pdf, folder) = (temp.0.clone(), temp.folder().join("generated"));
    let job_key = CorpusKey::of_file(&pdf).unwrap();
    let generator = Generator::with_runner(&folder, Box::new(MakesContents));
    let gate = Arc::new(Gate::default());
    let parked = Arc::clone(&gate);
    generator.between_end_and_write(move || parked.wait());
    generator.want(&pdf, job_key, 1);
    wait_for(|| gate.entered.load(Ordering::SeqCst), || "the job never reached its write".into());
    assert!(generated::read(&folder, &pdf, job_key).is_none(), "written already, so this proves nothing");

    assert_eq!(
        generator.hold_for_removal(Instant::now()).unwrap_err(),
        NotQuiet::StillRunning { building: true, uses: 0 }
    );
    gate.let_through();
    let hold = generator.hold_for_removal(must_succeed()).expect("quiet once the job has written");
    assert!(
        generated::read(&folder, &pdf, job_key).is_some(),
        "the hold returned before the contents were on disk"
    );
    drop(hold);
}

/// Failure: a held generator takes an ask from one kind of asker, so a removal's purge finds contents or blocks written again behind it: a person's ask for contents, an agent's, or an agent's for blocks. None of them is taken, and nothing runs.
#[test]
fn a_held_generator_takes_no_job_from_a_person_or_an_agent() {
    let runs = Arc::new(AtomicU32::new(0));
    let gate = Arc::new(Gate::default());
    let generator = Generator::with_runners(
        Path::new("/nowhere"),
        Box::new(Counts(Arc::clone(&runs))),
        Box::new(ParkedBlocks(Arc::clone(&gate))),
    );
    let hold = generator.hold_for_removal(must_succeed()).unwrap();
    generator.want_as_person(Path::new("/nowhere/1.pdf"), key(1), 3);
    generator.want(Path::new("/nowhere/2.pdf"), key(2), 3);
    generator.want_job(By::Agent, Job::Blocks(blocks_ask(Path::new("/nowhere/3.pdf"), 3)));
    for (what, number) in [(What::Contents, 1), (What::Contents, 2), (What::Blocks, 3)] {
        assert_eq!(generator.state_of(what, key(number)), GenState::Idle, "{what:?} {number} was taken");
    }
    assert!(!generator.0.lock().working, "a thread started");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "a runner was called");
    assert!(!gate.entered.load(Ordering::SeqCst), "a blocks runner was called");
    drop(hold);
}

/// Failure: a helper that never exits (a blocks job that ignores its stop stands for it) holds the walk for ever, or is taken for quiet at the bound. At the bound the hold is refused, says a job is still there, and lets go; once the job has ended it's had.
#[test]
fn a_blocks_job_that_ignores_its_stop_is_refused_at_the_bound() {
    let gate = Arc::new(Gate::default());
    let generator = Generator::with_runners(Path::new("/nowhere"), Box::new(MakesContents), Box::new(ParkedBlocks(Arc::clone(&gate))));
    generator.want_job(By::Agent, Job::Blocks(blocks_ask(Path::new("/nowhere/3.pdf"), 3)));
    wait_for(|| gate.entered.load(Ordering::SeqCst), || "the blocks job never started".into());
    assert_eq!(
        generator.hold_for_removal(Instant::now()).unwrap_err(),
        NotQuiet::StillRunning { building: true, uses: 0 }
    );
    assert!(!generator.0.lock().on_hold, "a refused hold stayed on");
    gate.let_through();
    assert!(generator.hold_for_removal(must_succeed()).is_ok());
}
