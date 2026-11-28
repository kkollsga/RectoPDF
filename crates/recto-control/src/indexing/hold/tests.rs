//! The indexer's hold, in process: the runner is injected, and every ordering is forced by a gate, never slept for.

use super::super::*;
use std::sync::Condvar;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

fn key(number: u8) -> CorpusKey {
    CorpusKey::from_hex(&format!("{number:02x}").repeat(32)).unwrap()
}

fn job(number: u8) -> IndexJob {
    IndexJob {
        path: PathBuf::from(format!("/nowhere/{number}.pdf")),
        key: key(number),
    }
}

/// Waits, up to 10 s, for `what` to be true, and fails with `saw` when it isn't.
fn wait_for(what: impl Fn() -> bool, saw: impl Fn() -> String) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !what() {
        assert!(Instant::now() < deadline, "{}", saw());
        std::thread::yield_now();
    }
}

/// Counts how many times it was run.
struct Counts(Arc<AtomicU32>);

impl Runner for Counts {
    fn run(&self, _job: &IndexJob, _root: &Path, _find_key: &FindKey, _started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value {
        self.0.fetch_add(1, Ordering::SeqCst);
        json!({ "state": "ready" })
    }
}

/// Failure: a panic outside the job's own `catch_unwind` (here the notifier's) ends the queue's thread with `working` still true, so no ask starts a thread again for the rest of the session, and a hold would wait out its whole time. After the thread dies, the next ask runs.
#[test]
fn a_queue_thread_that_panics_outside_its_job_lets_the_next_ask_run() {
    let runs = Arc::new(AtomicU32::new(0));
    let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Counts(Arc::clone(&runs))));
    let first = Arc::new(AtomicBool::new(true));
    indexer.attach(Arc::new(move |_events| {
        if first.swap(false, Ordering::SeqCst) {
            panic!("a notifier that fails once");
        }
    }));
    indexer.ask(job(1));
    wait_for(|| !indexer.0.lock().working, || "the thread that panicked is still counted as working".into());
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "the job whose start was announced ran after the notifier panicked"
    );
    indexer.ask(job(2));
    wait_for(
        || runs.load(Ordering::SeqCst) == 1,
        || format!("the next ask didn't run: {} runs", runs.load(Ordering::SeqCst)),
    );
}

fn must_succeed() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

/// A stop box that says when it's dropped, which is what stops a real helper (its stdin closes).
struct Flag(Arc<AtomicBool>);

impl Drop for Flag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Stands in for a helper that **ignores its stop**: it parks until the test opens the gate, as a helper that's slow to see its stdin close does.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    moved: Condvar,
    entered: AtomicBool,
    returned: AtomicU32,
    stop_dropped: Arc<AtomicBool>,
}

impl Gate {
    fn let_through(&self) {
        *self.open.lock().unwrap() = true;
        self.moved.notify_all();
    }
}

struct Parks(Arc<Gate>);

impl Runner for Parks {
    fn run(&self, _job: &IndexJob, _root: &Path, _find_key: &FindKey, started: &mut dyn FnMut(Box<dyn Any + Send>)) -> Value {
        let gate = &self.0;
        started(Box::new(Flag(Arc::clone(&gate.stop_dropped))));
        gate.entered.store(true, Ordering::SeqCst);
        let mut open = gate.open.lock().unwrap();
        while !*open {
            open = gate.moved.wait(open).unwrap();
        }
        gate.returned.fetch_add(1, Ordering::SeqCst);
        json!({ "state": "ready" })
    }
}

/// An indexer with a build parked in it, which ignores its stop.
fn parked() -> (Indexer, Arc<Gate>) {
    let gate = Arc::new(Gate::default());
    let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Parks(Arc::clone(&gate))));
    indexer.ask(job(1));
    wait_for(|| gate.entered.load(Ordering::SeqCst), || "the build never started".into());
    (indexer, gate)
}

/// Failure: the hold returns as soon as the stop is sent, so a helper that is slow to see its stdin close, or ignores it, is still writing the corpus when the walk goes on to delete it. It returns only once the build has ended, and a bound that passes first is refused with what was still going.
#[test]
fn a_hold_returns_only_after_the_running_build_has_ended() {
    let (indexer, gate) = parked();
    let refused = indexer.hold_for_removal(Instant::now());
    assert_eq!(refused.unwrap_err(), NotQuiet::StillRunning { building: true, uses: 0 });
    assert!(gate.stop_dropped.load(Ordering::SeqCst), "the build wasn't told to stop");
    gate.let_through();
    let hold = indexer.hold_for_removal(must_succeed()).expect("quiet once the build has ended");
    assert_eq!(gate.returned.load(Ordering::SeqCst), 1, "the hold returned before the build did");
    drop(hold);
}

/// Failure: a hold that was refused stays on, so the indexer builds nothing for the rest of the session though no removal is under way. A refusal lets go, and the next ask runs.
#[test]
fn a_refused_hold_lets_go_and_the_next_ask_runs() {
    let (indexer, gate) = parked();
    assert!(indexer.hold_for_removal(Instant::now()).is_err());
    assert!(!indexer.0.lock().on_hold, "a refused hold stayed on");
    gate.let_through();
    wait_for(|| !indexer.0.lock().working, || "the build never ended".into());
    indexer.ask(job(2));
    wait_for(
        || gate.returned.load(Ordering::SeqCst) == 2,
        || format!("the next ask didn't run: {} builds", gate.returned.load(Ordering::SeqCst)),
    );
}

/// Failure: a held indexer starts something on its own, a build, a sweep, a key use or an agent's folder, so a removal's purge finds the corpus written again behind it. After the hold, none of the starters does anything: nothing runs, nothing's queued, no thread starts, no key is handed out, and no mark or claim is written.
#[test]
fn a_held_indexer_starts_nothing() {
    let temp = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-held-");
    let root = temp.join("corpus.noindex");
    let paths = corpus::CorpusPaths::new(&root, key(1));
    for dir in [root.as_path(), paths.dir.parent().unwrap(), &paths.dir, &paths.app] {
        recto_platform::local::ensure_private_dir(dir).unwrap();
    }
    let runs = Arc::new(AtomicU32::new(0));
    let indexer = Indexer::with_runner(&root, Box::new(Counts(Arc::clone(&runs))));
    let hold = indexer.hold_for_removal(must_succeed()).expect("an indexer with nothing going is quiet");

    indexer.ask(job(1));
    Indexing::want(&indexer, Path::new("/nowhere/2.pdf"), key(2));
    // A sweep is due on a fresh indexer: asking for one starts the thread.
    assert!(indexer.0.lock().sweep_due);
    indexer.note_asked();
    assert!(indexer.find_key().is_none(), "a held indexer handed out the key");
    assert!(indexer.find_key_in_hand().is_none(), "a held indexer handed out the key it had in hand");
    Indexing::made_agent_folder(&indexer, key(1));

    assert_eq!(indexer.load(), (0, 0), "something was queued or running");
    {
        let inner = indexer.0.lock();
        assert!(!inner.working, "a thread started");
        assert!(inner.sweep_due, "the sweep ran");
        assert_eq!(inner.uses, 0, "a use of the key was counted");
        assert!(inner.agent_folders.is_empty(), "an agent's folder was claimed");
    }
    assert!(!paths.agent.join(".session").exists(), "a claim file was made");
    assert!(!corpus::agent_folders_may_be_there(&root), "the agents' mark was written");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the runner was called");

    // The same calls, with no hold, do all of it: the above is the hold's doing.
    drop(hold);
    Indexing::made_agent_folder(&indexer, key(1));
    assert!(paths.agent.join(".session").exists() && corpus::agent_folders_may_be_there(&root));
    assert!(indexer.find_key().is_some());
}

/// Failure: a corpus read or an agent export that is under way when the walk asks for the hold isn't waited for, so its write of `app/used` or its pages lands after the walk has deleted the folder. A use of the key that is out keeps the hold from returning, and once it's dropped the hold is had.
#[test]
fn a_key_use_out_keeps_the_hold_from_returning() {
    let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Counts(Arc::new(AtomicU32::new(0)))));
    let using = indexer.find_key().expect("the key, from a thread that may wait");
    assert_eq!(
        indexer.hold_for_removal(Instant::now()).unwrap_err(),
        NotQuiet::StillRunning { building: false, uses: 1 }
    );
    drop(using);
    assert!(indexer.hold_for_removal(Instant::now()).is_ok(), "the use ended and the hold wasn't had");
}

/// Failure: the hold keeps this session's own agent folders locked, so the purge's probe reads them as another session's and stops the removal as busy; or it sets them aside by a rename the plan had pinned a different folder for. The claims are let go and nothing is moved.
#[test]
fn the_hold_lets_go_of_this_sessions_claims_and_moves_nothing() {
    let temp = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-claims-");
    let root = temp.join("corpus.noindex");
    let paths = corpus::CorpusPaths::new(&root, key(1));
    for dir in [root.as_path(), paths.dir.parent().unwrap(), &paths.dir, &paths.app] {
        recto_platform::local::ensure_private_dir(dir).unwrap();
    }
    let indexer = Indexer::with_runner(&root, Box::new(Counts(Arc::new(AtomicU32::new(0)))));
    Indexing::made_agent_folder(&indexer, key(1));
    let session = paths.agent.join(".session");
    let free = || recto_platform::local::open_private_file(&session).unwrap().try_lock().is_ok();
    assert!(!free(), "the session's claim was never made, so this proves nothing");

    let hold = indexer.hold_for_removal(must_succeed()).expect("quiet");
    assert!(free(), "the purge's probe would find this session's own claim held");
    assert!(paths.agent.is_dir(), "the agent's folder was moved");
    let aside = std::fs::read_dir(&paths.dir)
        .unwrap()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("agent.gone"));
    assert!(!aside, "a folder was set aside");
    drop(hold);
}

/// Failure: a hold that can be taken twice lets two walks each think it's the one that's quiet, or a dropped hold stays on, so the indexer builds nothing again until the app restarts. A second hold is refused, dropping lets asks run again, and the drop restarts nothing by itself.
#[test]
fn dropping_the_hold_lets_asks_run_again_and_a_second_hold_is_refused() {
    let runs = Arc::new(AtomicU32::new(0));
    let indexer = Indexer::with_runner(Path::new("/nowhere"), Box::new(Counts(Arc::clone(&runs))));
    let hold = indexer.hold_for_removal(must_succeed()).unwrap();
    assert_eq!(indexer.hold_for_removal(must_succeed()).unwrap_err(), NotQuiet::AlreadyHeld);
    indexer.ask(job(1));
    assert_eq!(indexer.load(), (0, 0));
    drop(hold);
    assert!(!indexer.0.lock().working, "dropping the hold started a thread");
    indexer.ask(job(1));
    wait_for(|| runs.load(Ordering::SeqCst) == 1, || "the ask after the hold didn't run".into());
}
