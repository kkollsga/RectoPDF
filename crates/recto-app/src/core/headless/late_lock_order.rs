//! **The record's lock come late, in the orderings the third read found** (2026-11-13): what happens when a try, a give-back, a restore's read and the updater's
//! telling land in an order the ordinary tests in `late_lock` don't make. The bar: in no ordering is anything on disk lost or written over wrongly, and no update
//! is rolled back or begun wrongly. Each ordering is forced with a seam, never a sleep.

use super::late_lock::{
    a_saved_session, a_second_copy_that_started, comes_back_at_the_next_launch, handed_off_with_the_old_copy_holding, not_kept_words, page_of, record_folder,
    state_but_the_launch_ok, tab_names, the_copy_before, tries,
};
use crate::lifecycle::keeping::{Keeping, RESTORING_PATIENCE_MS, TRIES, TRIES_AFTER_UPDATE, TRY_EVERY_MS};
use crate::testkit::headless::HeadlessApp;
use recto_agent::Level;
use recto_platform::slow::Seam;
use std::sync::atomic::Ordering;

/// The clock moved on to the next try, and that try made (and failed).
fn next_try_fails(app: &HeadlessApp) {
    super::late_lock::next_try_fails(app);
}

/// The updater held back for the record's lock: not yet told of the launch.
fn updater_held(app: &HeadlessApp) -> bool {
    crate::lock(&app.core().life.update_facts).held
}

/// Failure (third read, 2026-11-13, the blocker): the last try gets the lock while a person's command is running on its own thread, so the lock is given back
/// and the tries are over; the state settles "not kept" by a path that gave the lock back and returned, so nothing was said and the updater never told: no
/// notice, every update check "still starting" for the run, no launch-ok, and after an update the installer at its two minutes stopped the running new version
/// and went back. The last of an update-opened launch's sixty tries, held with the lock and a person's command under way: the lock is given back and it still
/// settles: the notice shows, the updater hears it's a second copy (no longer held back), the installer sees the new version started (no tab count), and
/// nothing of the record, `updates.json` or the plan changes.
#[test]
fn the_last_try_back_with_a_persons_command_under_way_still_settles() {
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let was = state_but_the_launch_ok(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    crate::core::updates::first_frame_shown(app.core());
    for _ in 1..TRIES_AFTER_UPDATE {
        next_try_fails(&app);
    }
    recto_platform::slow::gate::hold(&[Seam::RecordLock]);
    drop(old_on_its_way_out);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the last try held with the lock", || app.waiting(Seam::RecordLock) > 0);
    app.core().life.persons_running.fetch_add(1, Ordering::SeqCst);
    app.let_through(Seam::RecordLock, 1);
    app.wait_until("settled not kept", || app.core().life.keeping() == Keeping::NotKept);
    app.core().life.persons_running.fetch_sub(1, Ordering::SeqCst);
    a_second_copy_that_started(&app, &plan);
    app.wait_until("the updater no longer held back", || !updater_held(&app));
    assert!(!app.core().saver.store.holds_the_record(), "the lock given back");
    app.core().saver.write_soon();
    app.settle();
    let (_, disk) = app.quit();
    assert!(state_but_the_launch_ok(&disk) == was, "the record, updates.json and the plan as they were");
}

/// Who sends `session restore`.
#[derive(Clone, Copy, Debug)]
enum Restorer {
    Person,
    AgentAtControl,
}

/// Failure (third read, 2026-11-13): after "won't be remembered", the other copy quits and `session restore` takes the lock (the store never let go); the state
/// stayed "not kept" with the store holding the lock, so the copy wrote nothing for the rest of the run, and the restore put the saved notice over "won't be
/// remembered": the person worked on a restored session with no notice and nothing written. On `main` the same command made the copy keep the record. By a
/// person's restore and an agent's at Control alike: the copy keeps the record, the notice goes, it's never "not kept" while the store holds the lock, and a
/// change made then is written and comes back at the next launch.
#[test]
fn a_restore_after_not_kept_keeps_the_record_and_what_follows_is_written() {
    for restorer in [Restorer::Person, Restorer::AgentAtControl] {
        let disk = a_saved_session();
        let other = the_copy_before(&disk);
        let app = disk.builder().another_keeps().start();
        for _ in 1..TRIES {
            next_try_fails(&app);
        }
        app.advance(TRY_EVERY_MS);
        app.wait_until("not kept, and said", || app.reader().note == not_kept_words());
        drop(other);
        match restorer {
            Restorer::Person => app.person(r#"{"cmd":"session_restore"}"#),
            Restorer::AgentAtControl => {
                app.agent(Level::Control, r#"{"cmd":"session_restore"}"#).expect("restored");
            }
        }
        app.settle();
        let keeping = app.core().life.keeping();
        let holds = app.core().saver.store.holds_the_record();
        assert!(!(keeping == Keeping::NotKept && holds), "{restorer:?}: not kept while the store holds the lock");
        assert_eq!((keeping, holds), (Keeping::Keeps, true), "{restorer:?}");
        assert_eq!(app.reader().note, None, "{restorer:?}: \"won't be remembered\" is no longer true, and goes");
        assert_eq!(tab_names(&app), ["a", "b"], "{restorer:?}");
        app.person(r#"{"cmd":"goto","page":3}"#);
        app.settle();
        let (_, disk) = app.quit();
        let again = disk.start();
        assert_eq!(
            page_of(&again, "b.pdf"),
            2,
            "{restorer:?}: the change made after the restore: {}",
            again.session_json()
        );
        drop(again.quit());
    }
}

/// Failure (third read, 2026-11-13): a try back with the lock while a person's command runs gives it back, and the state is "trying, none out" at once, but the
/// give-back was carried out by the try's thread when it woke; with a slow disk the next try came first, found the stale lock taken, and the old thread's
/// give-back then released the lock that try was using: "keeps" with nothing held, so nothing the person did that run was written, silently. The give-back is
/// done before the session thread takes anything else: once the call that decided it is over, the store holds nothing; and the next try keeps the record.
#[test]
fn a_lock_given_back_is_free_before_the_next_try_can_start() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    recto_platform::slow::gate::hold(&[Seam::RecordLock]);
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the try held with the lock", || app.waiting(Seam::RecordLock) > 0);
    app.core().life.persons_running.fetch_add(1, Ordering::SeqCst);
    app.let_through(Seam::RecordLock, 1);
    app.wait_until("given back, for the next try", || tries(&app) == Some(1));
    assert!(app.core().core.flush(crate::testkit::headless::WAIT_LIMIT), "the session thread at rest");
    assert!(
        !app.core().saver.store.holds_the_record(),
        "the lock is free by the time the session thread is at rest"
    );
    app.core().life.persons_running.fetch_sub(1, Ordering::SeqCst);
    recto_platform::slow::gate::open(Seam::RecordLock);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept at the next try", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert!(app.core().saver.store.holds_the_record());
    app.person(r#"{"cmd":"goto","page":3}"#);
    app.settle();
    let (_, disk) = app.quit();
    let again = disk.start();
    assert_eq!(page_of(&again, "b.pdf"), 2, "written: {}", again.session_json());
    drop(again.quit());
}

/// Failure (third read, 2026-11-13, the belt): "keeps the record" with the store holding no lock is a state a writer only skipped: nothing written for the rest
/// of the run, no notice, and the quit counted it as written. Forced here (the lock released under a copy that keeps the record): the next writer writes
/// nothing, the state becomes "not kept" and the person is told it won't be remembered; the record on disk is as it was last written.
#[test]
fn a_copy_that_keeps_the_record_and_finds_no_lock_says_so() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    app.core().saver.write_soon();
    app.settle();
    let on_disk = record_folder(app.disk());
    app.core().saver.store.give_back();
    app.core().saver.write_soon();
    app.settle();
    assert_eq!(app.core().life.keeping(), Keeping::NotKept, "the truth, once a writer finds it");
    assert_eq!(app.reader().note, not_kept_words(), "and told to the person");
    let flushed = app.core().saver.flush(false, crate::testkit::headless::WAIT_LIMIT);
    assert_eq!(flushed.map(|_| ()).map_err(|error| error.code), Err("other_instance"));
    assert_eq!(record_folder(app.disk()), on_disk, "nothing written without the lock");
    drop(app.quit());
}

/// Failure (third read, 2026-11-13, 4a and 4b): a late keep's read of the saved session is slow past its patience. The read's thread had already told the
/// updater of the launch as a copy that keeps the record (the install settled: the plan and the outcome removed, `updates.json` written), and the patience then
/// let the lock go while the read went on writing without it; the first frame then told the installer the new version came up, as the first copy, with no tab
/// count, in a window that showed none of the person's tabs. Now: at the patience the window goes on and the notice shows, the updater hears it's a second copy
/// (the installer sees it started, with no tab count, and the install is left for the next launch that keeps the record), the lock stays with the read's thread
/// until it's out, then goes; the record's folder, `updates.json` and the plan are byte for byte as they were; and the next launch restores and settles.
#[test]
fn a_late_keeps_read_slower_than_its_patience_is_a_second_copy_and_lets_go_only_once_out() {
    use crate::testkit::update_bed::NEW;
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let was = state_but_the_launch_ok(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    crate::core::updates::first_frame_shown(app.core());
    next_try_fails(&app);
    recto_platform::slow::gate::hold(&[Seam::State]);
    drop(old_on_its_way_out);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the saved session's read held inside it", || app.waiting(Seam::State) > 0);
    assert!(app.core().life.keeping().restoring());
    app.advance(RESTORING_PATIENCE_MS);
    a_second_copy_that_started(&app, &plan);
    assert!(
        state_but_the_launch_ok(app.disk()) == was,
        "nothing of the install settled, nothing of the record written"
    );
    assert!(app.core().saver.store.holds_the_record(), "the lock stays with the read while it's inside");
    app.let_through(Seam::State, 1);
    app.wait_until("let go once the read is out", || !app.core().saver.store.holds_the_record());
    app.settle();
    assert_eq!(app.core().life.keeping(), Keeping::NotKept);
    assert_eq!(app.reader().note, not_kept_words());
    let (_, disk) = app.quit();
    assert!(state_but_the_launch_ok(&disk) == was, "the record, updates.json and the plan as they were");
    disk.installer_saw_it_up();
    let next = disk.builder().update(&bed).start();
    assert_eq!(tab_names(&next), ["alpha"], "the next launch restores the session the hand-off wrote");
    crate::core::updates::first_frame_shown(next.core());
    next.wait_until("the install settled: the version proven", || {
        recto_update::saved::Saved::read(&next.disk().update_files().saved).proven.as_deref() == Some(NEW)
    });
    drop(next.quit());
}

/// Failure (third read, 2026-11-13, 4c): the window says it won't be remembered while a try is out and has the lock; the updater, told of the launch on its own
/// thread, asked the store whether this copy keeps the record, found the try's lock and settled the install as the first copy, in a window that said it won't
/// be remembered. The updater's look held while the try holds the lock: it hears a second copy, nothing of the install is settled, and the try's lock goes.
#[test]
fn the_updater_told_while_a_try_holds_the_lock_hears_a_second_copy() {
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let was = state_but_the_launch_ok(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    crate::core::updates::first_frame_shown(app.core());
    recto_platform::slow::gate::hold(&[Seam::RecordLock, Seam::Launch]);
    drop(old_on_its_way_out);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the try held with the lock", || app.waiting(Seam::RecordLock) > 0);
    let c = app.disk().pdf("c.pdf", 2);
    app.core().open_file(c);
    app.wait_until("the updater's look at the launch held", || app.waiting(Seam::Launch) > 0);
    app.let_through(Seam::Launch, 1);
    a_second_copy_that_started(&app, &plan);
    app.let_through(Seam::RecordLock, 1);
    app.wait_until("the try's lock gone", || !app.core().saver.store.holds_the_record());
    app.settle();
    let (_, disk) = app.quit();
    assert!(state_but_the_launch_ok(&disk) == was, "the record, updates.json and the plan as they were");
}

/// Failure (third read, 2026-11-13): after a late keep's read has gone past its patience, `session restore` loads the record while that read's thread is still
/// inside its own load, two loads at once on one folder. Refused, busy, for anyone, until the read's thread is out.
#[test]
fn a_restore_while_a_late_read_is_still_out_is_refused() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let on_disk = record_folder(&disk);
    let app = disk.builder().another_keeps().start();
    recto_platform::slow::gate::hold(&[Seam::State]);
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the read held", || app.waiting(Seam::State) > 0);
    app.advance(RESTORING_PATIENCE_MS);
    app.wait_until("the notice", || app.reader().note == not_kept_words());
    let refused = app.agent(Level::Control, r#"{"cmd":"session_restore"}"#).expect_err("refused");
    assert_eq!(refused.code, "busy", "{refused:?}");
    app.let_through(Seam::State, 1);
    app.wait_until("let go once the read is out", || !app.core().saver.store.holds_the_record());
    app.settle();
    let (_, disk) = app.quit();
    assert_eq!(record_folder(&disk), on_disk);
    comes_back_at_the_next_launch(disk);
}
