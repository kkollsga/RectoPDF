//! **What the window's calls into the app's life take, held by a test** (R3.7, S3; AGENTS.md: "the UI thread never takes the session's lock"): each call the
//! UI thread makes of the life (whether the state's read, whether the window takes input or is made again, whether it may be shown and what its first showing
//! had, a sheet, the person nearby or touching it) answers while a thread of the test holds the session's lock and the saver's gate, and while a late try at
//! the record's lock holds that lock on its own thread ([`Seam::RecordLock`]). So none of them takes either lock, or waits on the try's thread or the store's
//! file lock, as it moves. And each answers while QA's `record_report` is held where it reads the store ([`a_report_reads_the_store`]), as a writer
//! holding the store's own lock across a flush to a slow disk holds it: the store's mutexes are private to `recto-control`, and a hold of them from the
//! start would stop the report at its first read of the store, before anything the window takes, so the report is held at the read that comes after.

use super::late_lock::{a_saved_session, the_copy_before};
use crate::lifecycle::keeping::TRY_EVERY_MS;
use crate::testkit::headless::HeadlessApp;
use recto_platform::slow::Seam;
use std::sync::Arc;
use std::time::Duration;

/// How long a call may take with the locks held: far past what a lock-free call takes on a starved machine, and short of forever, which is what a call
/// waiting on a held lock takes.
const LIMIT: Duration = Duration::from_secs(5);

/// The window's calls into the life, in the order a launch makes them, each as the bridge makes it. Says the first that didn't answer within [`LIMIT`].
fn the_windows_life_calls_answer(app: &HeadlessApp) -> Result<(), String> {
    let core = Arc::clone(app.core());
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("test-window".into())
        .spawn(move || {
            let calls: [(&str, &dyn Fn()); 9] = [
                ("state_read", &|| {
                    let _ = core.life.state_read();
                }),
                ("window_interactive", &|| {
                    let _ = core.life.window_interactive();
                }),
                ("may_remake_window", &|| {
                    let _ = core.life.may_remake_window();
                }),
                ("ready_to_show", &|| {
                    let _ = core.life.ready_to_show(core.page_checks());
                }),
                ("window_shown", &|| {
                    core.life.record_window_shown(false, 1, "ready", &core.engine, core.page_checks())
                }),
                ("sheet_changed open", &|| core.life.sheet_changed(true)),
                ("sheet_changed closed", &|| core.life.sheet_changed(false)),
                ("note_nearby", &|| core.life.note_nearby()),
                ("note_input", &|| core.note_input()),
            ];
            for (name, call) in calls {
                call();
                if said.send(name).is_err() {
                    return;
                }
            }
        })
        .expect("a thread");
    let mut answered = "none yet";
    for _ in 0..9 {
        answered = heard
            .recv_timeout(LIMIT)
            .map_err(|_| format!("the call after {answered} waited on a lock the test holds"))?;
    }
    Ok(())
}

/// **A report held where it reads the store, in a test that holds reports** ([`hold_reports`]): `record_report` calls this just before it reads what the
/// store wrote, and with reports held it waits here until they're let go, as it would on a store whose writer is flushing to a slow disk. Nothing else
/// calls it, and outside a test it isn't compiled.
pub(crate) fn a_report_reads_the_store() {
    let mut held = REPORTS.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if !held.0 {
        return;
    }
    held.1 += 1;
    REPORTS.1.notify_all();
    while held.0 {
        held = REPORTS.1.wait(held).unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

/// Whether reports are held at the store, and how many have come to it: the test's process's own (each test is one).
static REPORTS: (std::sync::Mutex<(bool, u32)>, std::sync::Condvar) = (std::sync::Mutex::new((false, 0)), std::sync::Condvar::new());

/// Reports held at the store (`true`) or let go; a report that's come to it is told.
fn hold_reports(hold: bool) {
    REPORTS.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).0 = hold;
    REPORTS.1.notify_all();
}

/// Waits, at most [`LIMIT`], for a report to be held at the store.
fn a_report_is_held() -> bool {
    let held = REPORTS.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (held, _) = REPORTS
        .1
        .wait_timeout_while(held, LIMIT, |held| held.1 == 0)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.1 > 0
}

/// The session's lock and the saver's gate, held by a thread of the test until the returned sender is dropped.
fn hold_the_session_and_the_gate(app: &HeadlessApp) -> std::sync::mpsc::Sender<()> {
    let core = Arc::clone(app.core());
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (holding, held) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("test-holder".into())
        .spawn(move || {
            let _gate = crate::lock(&core.saver.gate);
            core.host.session.with_session(|_| {
                let _ = holding.send(());
                let _ = released.recv();
            });
        })
        .expect("a thread");
    held.recv_timeout(LIMIT).expect("the session's lock and the saver's gate taken");
    release
}

/// Failure (R3.7, S3, a guard before the life's calls move): a call the window makes on every input or at its launch takes the session's lock, the saver's
/// gate, or waits on a late try at the record's lock, so the window stalls for as long as a long command, a write to a slow disk or a sleeping disk holds them.
/// With an app that keeps the record and one whose late try is held on its thread with the lock, each call answers while the test holds both locks.
#[test]
fn the_windows_life_calls_take_neither_the_sessions_lock_nor_the_savers_gate() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("a.pdf", 3);
    app.open(&file);
    app.settle();
    let held = hold_the_session_and_the_gate(&app);
    assert_eq!(the_windows_life_calls_answer(&app), Ok(()), "an app that keeps the record");
    drop(held);
    drop(app.quit());

    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().hold(&[Seam::RecordLock]).start();
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the try held with the record's lock", || app.waiting(Seam::RecordLock) > 0);
    let held = hold_the_session_and_the_gate(&app);
    assert_eq!(the_windows_life_calls_answer(&app), Ok(()), "a late try holding the record's lock");
    drop(held);
    recto_platform::slow::gate::open(Seam::RecordLock);
    app.settle();
    drop(app.quit());
}

/// Failure (the review of R3.7's life slice, 2026-11-14): QA's `record_report` built its whole answer holding the first showing's lock, and the answer reads
/// the store, whose writer holds the store's own lock across a flush to the disk. So with a report waiting there, the window shown (`record_window_shown`,
/// which writes the first showing) waited on the disk on the UI thread. With a report held where it reads the store, each of the window's calls answers.
#[test]
fn the_windows_life_calls_answer_while_a_report_waits_on_the_store() {
    let app = HeadlessApp::start();
    let file = app.disk().pdf("a.pdf", 3);
    app.open(&file);
    app.settle();
    hold_reports(true);
    let core = Arc::clone(app.core());
    let reporter = std::thread::Builder::new()
        .name("test-report".into())
        .spawn(move || core.record_report())
        .expect("a thread");
    assert!(a_report_is_held(), "the report held where it reads the store");
    let answered = the_windows_life_calls_answer(&app);
    hold_reports(false);
    let report = reporter.join().expect("the report");
    assert_eq!(answered, Ok(()), "with a report waiting on the store");
    assert!(report["wrote_bytes"].is_u64(), "and the report answers once the store does: {report}");
    drop(app.quit());
}
