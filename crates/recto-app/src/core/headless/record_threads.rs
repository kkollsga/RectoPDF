//! **The record's threads as the app ends** (R3.7 S5). The launch's read, a late try at the record's lock, a late keep's read and the sleep files' writer each
//! run on a thread of their own, holding the record's keeper and not the app. When the app ends (a process ending: the kit's kill) with one of them still
//! under way, it goes on to find the app gone: nothing it does after the end reaches the disk or the host, and the end still waits for it, so no thread runs
//! on into whatever comes next.

use super::late_lock::{a_saved_session, the_copy_before};
use crate::lifecycle::keeping::TRY_EVERY_MS;
use crate::testkit::headless::{Disk, HeadlessApp};
use recto_platform::slow::{Seam, gate};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long the record's keeper, the sleeper and the host's listener may take to go once the app's end has returned: the end already waits for the threads
/// that hold them, so this is only the moment a last reference is dropped, far inside it.
const PARTS_GO_WITHIN: Duration = Duration::from_secs(5);

/// Every file under the state folder, byte for byte: the record, its journals and sleep files, `updates.json` and the lock.
fn state_folder(disk: &Disk) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(folder: &Path, found: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                walk(&entry.path(), found);
            } else if let Ok(bytes) = std::fs::read(entry.path()) {
                found.insert(entry.path(), bytes);
            }
        }
    }
    let mut found = BTreeMap::new();
    walk(&disk.paths().root, &mut found);
    found
}

/// A thread of the record's, held at its seam, as the app it's for is about to end.
struct Held {
    case: &'static str,
    app: HeadlessApp,
    seam: Seam,
    /// The copy before, still keeping the record, for a try that doesn't get it.
    before: Option<recto_control::record_store::RecordStore>,
}

/// The launch's read of the saved state, held at its seam: the window went up without it.
fn the_launchs_read() -> Held {
    let app = a_saved_session().builder().hold(&[Seam::State]).start();
    Held {
        case: "the launch's read",
        app,
        seam: Seam::State,
        before: None,
    }
}

/// A late try at the record's lock, held on its thread between its try (which didn't get it: the copy before still has it) and its word to the session thread.
fn a_late_try() -> Held {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    gate::hold(&[Seam::RecordLock]);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the try held on its thread", || app.waiting(Seam::RecordLock) > 0);
    Held {
        case: "a late try",
        app,
        seam: Seam::RecordLock,
        before: Some(before),
    }
}

/// A late try that got the record's lock, held on its thread before its word to the session thread: as the app ends, the word goes to a queue that's been
/// abandoned, and the thread, waiting to hear whether to read, must hear nothing and end.
fn a_late_try_with_the_lock() -> Held {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    gate::hold(&[Seam::RecordLock]);
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the try held with the lock", || app.waiting(Seam::RecordLock) > 0);
    assert!(app.core().saver.store.holds_the_record(), "the try has the lock");
    Held {
        case: "a late try with the lock",
        app,
        seam: Seam::RecordLock,
        before: None,
    }
}

/// A late keep's read of the saved session, held inside it on the try's thread, the lock in hand.
fn a_late_keeps_read() -> Held {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    gate::hold(&[Seam::State]);
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the late keep's read held", || app.waiting(Seam::State) > 0);
    Held {
        case: "a late keep's read",
        app,
        seam: Seam::State,
        before: None,
    }
}

/// The sleep files' writer, held writing a tab's file: the system asked for memory back, and the tabs off screen went dormant.
fn the_sleep_writer() -> Held {
    let disk = Disk::new();
    let docs: Vec<_> = (0..3).map(|at| disk.pdf(&format!("t{at}.pdf"), 2)).collect();
    let app = disk.builder().hold(&[Seam::SleepWrite]).start();
    for doc in &docs {
        app.open(doc);
        app.settle();
    }
    app.core().qa_session_verb("memory_pressure", "warning").expect("a QA verb").expect("pressure");
    app.wait_until("a sleep file held mid-write", || app.waiting(Seam::SleepWrite) > 0);
    Held {
        case: "the sleep files' writer",
        app,
        seam: Seam::SleepWrite,
        before: None,
    }
}

/// **Failure: a thread of the record's that's still under way as the app ends does something after it, or keeps the app alive.** Holding the app, it kept
/// the whole app alive past the end and the kit's end waited on it in vain; holding only the record's keeper, it could still step the app's keeping of the
/// record ("not kept", a claim) and set its notice after the end, or wait forever on a word the abandoned queue kept. Each of five, held at its seam as the
/// app's killed, then let through: the launch's read, a late try without the lock and one with it, a late keep's read, and the sleep files' writer.
///
/// **What this test itself fails on:** how the app keeps the record and its notice changing after the end (the keeper's own check of the store's let-go);
/// the record's keeper or the sleeper still held once the end has returned, or the host's listener (a thread that didn't end, or a cycle); and the kit's end
/// itself, which waits for those threads and fails at its limit. **What holds the disk half:** the state folder (the record, its journals and sleep files,
/// `updates.json`, the lock) byte for byte as it was at the end is held by the store's let-go (`RecordStore::let_go`: it writes nothing after) and the
/// updater's `here()` (nothing written for an ended app); this test sees it, and would see a write that went round them.
#[test]
fn a_thread_of_the_records_held_as_the_app_ends_writes_nothing_after_it() {
    for held in [
        the_launchs_read as fn() -> Held,
        a_late_try,
        a_late_try_with_the_lock,
        a_late_keeps_read,
        the_sleep_writer,
    ] {
        let Held { case, app, seam, before } = held();
        let host = Arc::downgrade(&app.core().listener);
        let parts = app.core().on_disk.downgrade();
        // What the app was told last, looked at as it ended and again once its threads are gone: kept for the look only, and let go before the end's checks.
        let (life, told) = (Arc::clone(&app.core().life), Arc::clone(&app.core().host));
        let said = || (life.keeping(), crate::lock(&told.notice).clone());
        let mut at_end = None;
        let disk = app.kill_holding(seam, |disk| at_end = Some((state_folder(disk), said())));
        let (at_end, said_at_end) = at_end.expect("looked at as the app ended");
        assert_eq!(
            said(),
            said_at_end,
            "{case}: the app was told something after its end (how it keeps the record, or its notice)"
        );
        drop((life, told));
        let began = std::time::Instant::now();
        while host.strong_count() > 0 {
            assert!(
                began.elapsed() < PARTS_GO_WITHIN,
                "{case}: something still holds the way to the host after the app's end"
            );
            std::thread::sleep(Duration::from_millis(5)); // ratchet: poll
        }
        let now = state_folder(&disk);
        assert!(
            now == at_end,
            "{case}: written after the app ended: {:?}",
            now.keys()
                .chain(at_end.keys())
                .filter(|path| at_end.get(*path) != now.get(*path))
                .collect::<Vec<_>>()
        );
        parts.wait_until_let_go(PARTS_GO_WITHIN);
        drop(before);
    }
}

/// **Failure: a part of the app outlives it,** held by something it holds (the keeper by the host's session or the updater's hooks, the sleeper by its own
/// writer's start), so its threads' handles, its store and its queue stay for the life of the process. After a launch, tabs opened, one gone dormant with
/// its sleep file written by the queue's writer, and a quit: neither the record's keeper nor the sleeper is held by anything.
#[test]
fn the_records_keeper_and_the_sleeper_go_with_the_app() {
    let disk = Disk::new();
    let docs: Vec<_> = (0..3).map(|at| disk.pdf(&format!("t{at}.pdf"), 2)).collect();
    let app = disk.start();
    for doc in &docs {
        app.open(doc);
        app.settle();
    }
    app.core().qa_session_verb("memory_pressure", "warning").expect("a QA verb").expect("pressure");
    app.settle();
    let parts = app.core().on_disk.downgrade();
    drop(app.quit());
    parts.wait_until_let_go(PARTS_GO_WITHIN);
}
