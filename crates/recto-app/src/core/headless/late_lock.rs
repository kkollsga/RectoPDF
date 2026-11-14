//! **A launch that doesn't get the record's lock at its start** (owner, 2026-11-13): it tries again quietly for ten seconds and keeps the record once it has it,
//! restoring the saved session as a launch would have; another copy that really keeps it is said as before, after the ten seconds; and a person who starts
//! working in the window before the lock comes is told at once that it won't be remembered. The copy before it is a store the test holds, let go of when the
//! test says, and the ten seconds are the fake clock's. The tries' spacing is the pure machine's, tested in `lifecycle::keeping`.

use crate::lifecycle::keeping::{Keeping, TRIES, TRY_EVERY_MS};
use crate::testkit::Heard;
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};
use recto_agent::Level;
use recto_control::record_store::{Flavor, RecordStore};
use recto_platform::slow::Seam;
use recto_session::record::{Rect, WindowRecord, notices};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The words of the notice a window that won't be remembered shows.
pub(super) fn not_kept_words() -> Option<String> {
    notices::words(notices::OTHER_INSTANCE).map(str::to_owned)
}

/// The copy before this one, still holding the record: a store of the test's on the disk.
pub(super) fn the_copy_before(disk: &Disk) -> RecordStore {
    let store = RecordStore::open(disk.paths(), Flavor::App);
    assert!(store.holds_the_record(), "the copy before keeps the record");
    store
}

/// How many tries it has made, while it's trying.
pub(super) fn tries(app: &HeadlessApp) -> Option<u32> {
    match app.core().life.keeping() {
        Keeping::Trying { tries, .. } => Some(tries),
        _ => None,
    }
}

/// The clock moved on to the next try, and that try made (and failed).
pub(super) fn next_try_fails(app: &HeadlessApp) {
    let before = tries(app).expect("trying");
    app.advance(TRY_EVERY_MS);
    app.wait_until("the next try", || tries(app) != Some(before));
    app.settle();
}

/// Every file of the record's folder, by its bytes: what a window that doesn't keep the record must leave as it is.
pub(super) fn record_folder(disk: &Disk) -> BTreeMap<PathBuf, Vec<u8>> {
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
    walk(&disk.paths().root.join("session"), &mut found);
    found
}

/// Where the copy before left its window.
const SAVED_X: f64 = 111.0;
/// Where this launch's window is, as its host says before anything comes back.
const REAL_X: f64 = 222.0;

/// The window's frame as the host says it, at `x`, as `AppCore::window_changed` sets it.
fn window_at(app: &HeadlessApp, x: f64) {
    crate::lock(&app.core().window.app).window = Some(WindowRecord {
        frame: Rect {
            x,
            y: 40.0,
            width: 900.0,
            height: 700.0,
        },
        screen: None,
        full_screen: false,
        minimised: false,
    });
    app.core().saver.view_changed();
}

/// Where the record would say the window is now.
fn window_x(app: &HeadlessApp) -> Option<f64> {
    app.core().window.app_record(|| None).window.map(|window| window.frame.x)
}

/// A session of two tabs, the first with a page turned and not saved, its window at [`SAVED_X`], quit as the copy before: its record and the first tab's journal
/// on disk.
pub(super) fn a_saved_session() -> Disk {
    let app = HeadlessApp::start();
    let (a, b) = (app.disk().pdf("a.pdf", 3), app.disk().pdf("b.pdf", 3));
    app.open(&a);
    app.open(&b);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##);
    window_at(&app, SAVED_X);
    app.settle();
    let (answer, disk) = app.quit();
    assert!(answer.go, "{answer:?}");
    disk
}

/// The page, counted from 0, of the tab on the file named.
pub(super) fn page_of(app: &HeadlessApp, file: &str) -> u64 {
    let session = app.session_json();
    let tab = session["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["path"].to_string().contains(file));
    tab.and_then(|tab| tab["page"].as_u64())
        .unwrap_or_else(|| panic!("no tab on {file}: {session}"))
}

pub(super) fn tab_names(app: &HeadlessApp) -> Vec<String> {
    app.tabs().into_iter().map(|row| row.text).collect()
}

/// Failure (owner, 2026-11-13; the case this is for: the copy before still quitting, or an update's hand-off letting go): a launch that doesn't get the lock at
/// its start never asks again, so the window isn't remembered and the saved session doesn't come back though the copy before let go a moment later; or it says
/// "won't be remembered" while it's still trying, and that turns out false. The copy before lets go after 2 s on the app's clock: the next try takes the
/// record, the saved session comes back as a launch's would, with the unsaved turn its journal kept, nothing on disk changed before that, and the notice never
/// shows, then or later.
#[test]
fn a_lock_let_go_after_two_seconds_is_kept_and_the_session_comes_back() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let on_disk = record_folder(&disk);
    assert!(
        on_disk.keys().any(|path| path.to_string_lossy().contains("journal-")),
        "the copy before left a journal: {:?}",
        on_disk.keys()
    );
    let app = disk.builder().another_keeps().start();
    assert!(!app.core().saver.store.holds_the_record());
    assert_eq!(tries(&app), Some(0), "trying, quietly");
    assert!(app.tabs().is_empty(), "nothing restored without the record");
    assert_eq!(app.reader().note, None, "nothing said while it tries");
    // What would be written at once while it tries (the app going to the background) isn't: the record is the copy before's.
    app.core().saver.write_soon();
    next_try_fails(&app);
    assert_eq!(app.reader().note, None, "nothing said after a try that failed");
    assert_eq!(record_folder(app.disk()), on_disk, "nothing on disk changed while it tries");
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert!(app.core().saver.store.holds_the_record());
    assert_eq!(tab_names(&app), ["a", "b"], "the saved session, as a launch would have brought it back");
    assert_eq!(
        app.session_json()["tabs"][0]["edits"]["edited"],
        true,
        "the turn the copy before left unsaved: {}",
        app.session_json()["tabs"][0]
    );
    assert_eq!(app.reader().note, None, "the notice never shows");
    app.advance(30_000);
    app.settle();
    assert_eq!(app.reader().note, None, "nor later");
    // And what's done from here is remembered: the next launch has it.
    app.person(r#"{"cmd":"goto","page":3}"#);
    app.settle();
    let (_, disk) = app.quit();
    let again = disk.start();
    assert_eq!(tab_names(&again), ["a", "b"]);
    assert_eq!(
        page_of(&again, "b.pdf"),
        2,
        "the page the person went to after the lock came: {}",
        again.session_json()
    );
    drop(again.quit());
}

/// Failure (owner, 2026-11-13: "another RectoPDF really keeping the state is said as it is today"): a launch whose lock is never let go says nothing, or says it
/// before the ten seconds are up, or goes on trying for ever; or a try holds the session thread or the window waiting for the lock. Nothing said through nine
/// tries, today's words at the tenth, and then no deadline and no more tries; the session thread answers and the window's calls return between every try.
#[test]
fn a_lock_never_let_go_is_said_after_ten_seconds_and_not_before() {
    let disk = Disk::new();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    for _ in 1..TRIES {
        next_try_fails(&app);
        assert_eq!(app.reader().note, None, "nothing said before the ten seconds");
        // The window's calls, with the lock still held elsewhere: none waits for it.
        let core = std::sync::Arc::clone(app.core());
        let (said, heard) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("test-window".into())
            .spawn(move || {
                let _ = core.life.ready_to_show(&core.host.session, &core.engine);
                let _ = core.life.state_read();
                let _ = crate::lock(&core.published).reader.home;
                let _ = said.send(core.core.flush(WAIT_LIMIT));
            })
            .expect("a thread");
        assert_eq!(
            heard.recv_timeout(WAIT_LIMIT),
            Ok(true),
            "the window's calls and the session thread answered while it tries"
        );
    }
    assert_eq!(tries(&app), Some(TRIES - 1));
    app.advance(TRY_EVERY_MS);
    app.wait_until("the notice", || app.reader().note == not_kept_words());
    assert_eq!(app.core().life.keeping(), Keeping::NotKept);
    assert_eq!(app.core().life.keeping_due(), None, "no more tries");
    drop(before);
    app.advance(60_000);
    app.settle();
    assert!(!app.core().saver.store.holds_the_record(), "it doesn't go on trying");
    assert_eq!(app.reader().note, not_kept_words(), "it stays said");
}

/// Failure (the coordinator's choice, 2026-11-13): the person opens a file before the lock comes, and the launch goes on trying, so the lock coming then
/// restores over their tab, or keeps the record with their window and writes it over the saved session; or it says nothing until the ten seconds are up. It
/// stops at once and says so, and the saved record and its journal are left exactly as they were, for the next launch.
#[test]
fn opening_a_file_before_the_lock_comes_stops_trying_and_says_so_at_once() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let on_disk = record_folder(&disk);
    let app = disk.builder().another_keeps().start();
    next_try_fails(&app);
    let c = app.disk().pdf("c.pdf", 2);
    app.open(&c);
    app.settle();
    assert_eq!(app.reader().note, not_kept_words(), "said at once, with the clock where it was");
    assert_eq!(app.core().life.keeping(), Keeping::NotKept);
    drop(before);
    app.advance(5 * TRY_EVERY_MS);
    app.settle();
    assert!(!app.core().saver.store.holds_the_record(), "no more tries once the person's working");
    assert_eq!(tab_names(&app), ["c"], "what they have, and nothing restored over it");
    app.core().saver.write_soon();
    app.settle();
    let (_, disk) = app.quit();
    assert_eq!(record_folder(&disk), on_disk, "the saved session and its journal as the copy before left them");
    let again = disk.start();
    assert_eq!(tab_names(&again), ["a", "b"], "the saved session at the next launch");
    assert_eq!(again.session_json()["tabs"][0]["edits"]["edited"], true);
    drop(again.quit());
}

/// Failure (found on the way, 2026-11-13): a window that doesn't keep the record tries to write it when something comes due (a minute after a change, or at once
/// when the app goes to the background), is refused, and says "RectoPDF couldn't save its state, so what's changed since the last save would be lost if it
/// closed now" over the notice that it won't be remembered: wrong, and alarming, as nothing it could save is being lost. It writes nothing and says only that.
#[test]
fn a_window_that_doesnt_keep_the_record_never_says_its_state_couldnt_be_saved() {
    let disk = Disk::new();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    let c = app.disk().pdf("c.pdf", 2);
    app.open(&c);
    app.core().saver.write_soon();
    app.settle();
    // Past the record's minute, and past any trying.
    app.advance(70_000);
    app.settle();
    assert!(!app.core().saver.has_failed(), "{:?}", app.core().saver.last_error());
    assert_eq!(app.reader().note, not_kept_words());
    drop(before);
}

/// An update handed off and installed, with the old copy still holding the record: the disk the new version launches on, the plan the installer went by, and
/// the old copy on its way out.
pub(super) fn handed_off_with_the_old_copy_holding() -> (Disk, crate::testkit::update_bed::UpdateBed, recto_update::plan::Plan, RecordStore) {
    use crate::testkit::update_bed::{Installed, NEW, OLD, UpdateBed};
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let file = disk.pdf("alpha.pdf", 20);
    let app = disk.builder().update(&bed).start();
    app.open(&file);
    app.agent(Level::Control, r#"{"cmd":"goto","page":9}"#).expect("goto");
    app.settle();
    bed.releases.offer(NEW);
    assert_eq!(app.agent(Level::Full, r#"{"cmd":"update_check"}"#).expect("check")["status"], "available");
    assert_eq!(app.agent(Level::Full, r#"{"cmd":"update_install"}"#).expect("install")["started"], true);
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::Swapped);
    let old_on_its_way_out = the_copy_before(&disk);
    (disk, bed, plan, old_on_its_way_out)
}

/// What the installer would make of the new version now, as `new_app_started` reads it: up (the version it was going to, with the tabs it was handed), or why
/// not yet.
pub(super) fn installer_sees(disk: &Disk, plan: &recto_update::plan::Plan) -> Result<(), String> {
    let text = std::fs::read_to_string(disk.update_files().launch_ok()).map_err(|_| "no launch-ok".to_owned())?;
    let (version, tabs) = recto_update::plan::read_launch_ok(&text).ok_or_else(|| format!("unreadable: {text}"))?;
    if version != plan.to {
        return Err(format!("another version: {text}"));
    }
    if recto_update::plan::misread(plan.handed_off_tabs, tabs) {
        return Err(format!("misread: {text}, handed {:?}", plan.handed_off_tabs));
    }
    Ok(())
}

/// Failure (owner, 2026-11-07: "It's crucial that app state is perfectly recovered before and after update"): the new version's launch asks for the record while
/// the old version is still letting go of it, in a hand-off's last moments, and starts empty and unremembered, as it did when it asked only once; or it keeps the
/// record late but the updater, told at the launch that this was a second copy, never says it came up (review, 2026-11-13): the installer waits out its two
/// minutes, stops the new app, puts the record from before the update back over what the person did meanwhile and swaps the old version back. Its window shows
/// while it tries; the lock comes at the ninth try: the session comes back as the hand-off wrote it, `update status` says it updated and isn't a second copy,
/// and the installer sees the new version up with the tab it was handed, well within its limit.
#[test]
fn an_update_whose_old_copy_still_holds_the_record_comes_back_as_it_was() {
    use crate::testkit::update_bed::NEW;
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let app = disk.builder().update(&bed).another_keeps().start();
    assert_eq!(app.disk().app_version().as_deref(), Some(NEW));
    assert!(app.tabs().is_empty() && app.reader().note.is_none(), "trying, quietly");
    // The window's first frame, as the host tells it: held for when it's known what this copy is.
    app.core().life.update_facts.first_frame_shown(&app.core().updates);
    for _ in 1..9 {
        next_try_fails(&app);
    }
    assert!(installer_sees(app.disk(), &plan).is_err(), "nothing said to the installer while it tries");
    drop(old_on_its_way_out);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["alpha"]);
    assert_eq!(page_of(&app, "alpha.pdf"), 8, "the page it was on: {}", app.session_json());
    assert_eq!(app.reader().note, None);
    app.wait_for("the installer to see it up", || installer_sees(app.disk(), &plan));
    let status = app.update_status();
    assert_eq!(status["status"], "updated", "{status}");
    assert_eq!(status["second_copy"], false, "{status}");
}

/// How many times the updater says a new version came up from the update it arrived by: once a launch, at most.
fn times_said_updated(disk: &Disk) -> usize {
    std::fs::read_to_string(disk.update_files().log)
        .unwrap_or_default()
        .matches("updated from")
        .count()
}

/// Failure (review, 2026-11-13, the blocker): the launch's wait for the record's lock runs out with the old copy still letting go, the updater is held back for
/// it, and then the lock is let go before the state's read, which waited for it a second time and got it: the copy kept the record with nothing settling the
/// machine through a try, so the updater was never told. For the rest of the run every check was "still starting", no timer, and after an update the installer
/// never saw it come up, waited out its two minutes, stopped the new version and put the record from before the update over what the person had done. The lock
/// let go at that moment (the read held at its seam, past the launch's wait): the read goes by the launch's one answer, a try keeps the record, the updater is
/// told once, `update status` says it updated, and the installer sees the new version up with the tab it was handed.
#[test]
fn a_lock_let_go_between_the_launchs_wait_and_its_read_still_tells_the_updater() {
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let app = disk.builder().update(&bed).another_keeps().hold(&[Seam::State]).start();
    // The window's first frame, as the host tells it with the launch held back.
    app.core().life.update_facts.first_frame_shown(&app.core().updates);
    drop(old_on_its_way_out);
    app.let_through(Seam::State, 1);
    app.wait_until("the state taken in", || app.core().life.state_read());
    app.settle();
    assert!(!app.core().saver.store.holds_the_record(), "the read didn't take the lock: it's the try's");
    app.advance(TRY_EVERY_MS);
    app.wait_until("the try's read of the saved session at its seam", || app.waiting(Seam::State) > 0);
    app.let_through(Seam::State, 1);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["alpha"], "the session the hand-off wrote");
    app.wait_for("the installer to see it up", || installer_sees(app.disk(), &plan));
    let status = app.update_status();
    assert_eq!(status["status"], "updated", "{status}");
    assert_eq!(status["second_copy"], false, "{status}");
    assert!(app.core().updates.next_wake().is_some(), "its timer: {status}");
    assert_eq!(
        times_said_updated(app.disk()),
        1,
        "told once: {}",
        std::fs::read_to_string(app.disk().update_files().log).unwrap_or_default()
    );
}

/// Failure (the coordinator's condition, 2026-11-13): while the launch is held back nothing of an update can begin, so a copy that turns out to be a second copy
/// has started a check, a download, a plan or a hand-off; or the person's check is passed over in silence. The person's check while it tries is answered that
/// it's still starting and starts nothing; once the tries run out the copy is a second copy, as before, and refuses an install as one.
#[test]
fn nothing_of_an_update_begins_while_it_tries() {
    use crate::testkit::update_bed::{NEW, OLD, UpdateBed};
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let before = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    app.core().life.update_facts.first_frame_shown(&app.core().updates);
    bed.releases.offer(NEW);
    let refused = crate::core::updates::command(
        &app.core().updates,
        &app.core().life.update_facts,
        &recto_agent::parse(r#"{"cmd":"update_check"}"#).expect("a command"),
    )
    .expect_err("not yet");
    assert_eq!(refused.code, "not_ready", "{refused:?}");
    app.person(r#"{"cmd":"update_check"}"#);
    app.person(r#"{"cmd":"update_install","now":true}"#);
    app.settle();
    let status = app.update_status();
    assert_eq!(status["status"], "idle", "nothing checked or started: {status}");
    assert!(app.core().updates.next_wake().is_none(), "and no timer for one");
    assert!(bed.installer.plans().is_empty());
    for _ in 1..TRIES {
        next_try_fails(&app);
    }
    app.advance(TRY_EVERY_MS);
    app.wait_until("the notice", || app.reader().note == not_kept_words());
    app.wait_until("told it's a second copy", || app.update_status()["second_copy"] == true);
    let refused = crate::core::updates::command(
        &app.core().updates,
        &app.core().life.update_facts,
        &recto_agent::parse(r#"{"cmd":"update_install","now":true}"#).expect("a command"),
    )
    .expect_err("a second copy");
    assert_eq!(refused.code, "another_copy", "{refused:?}");
    assert!(bed.installer.plans().is_empty(), "nothing ever started");
    assert!(
        !app.disk().update_files().launch_ok().exists(),
        "a second copy no update opened tells no installer it started"
    );
    drop(before);
}

/// What the installer was told, as it reads it: the version, and the tab count if one was said.
pub(super) fn launch_ok_says(disk: &Disk) -> Option<(String, Option<u32>)> {
    let text = std::fs::read_to_string(disk.update_files().launch_ok()).ok()?;
    recto_update::plan::read_launch_ok(&text).map(|(version, tabs)| (version.to_owned(), tabs))
}

/// The record's folder by its files' bytes, `updates.json` and the installer's plan.
pub(super) type StateFiles = (BTreeMap<PathBuf, Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);

/// The state folder's files that a second copy must leave as they were: the record's folder, `updates.json` and the installer's plan.
pub(super) fn state_but_the_launch_ok(disk: &Disk) -> StateFiles {
    let files = disk.update_files();
    (record_folder(disk), std::fs::read(&files.saved).ok(), std::fs::read(files.plan()).ok())
}

/// The new version as a second copy that came up after an update: the notice, `update status` says so, and the installer was told it started, with no tab
/// count, so it reads it as up and never as a misread.
pub(super) fn a_second_copy_that_started(app: &HeadlessApp, plan: &recto_update::plan::Plan) {
    use crate::testkit::update_bed::NEW;
    app.wait_until("the notice", || app.reader().note == not_kept_words());
    app.wait_until("told it's a second copy", || app.update_status()["second_copy"] == true);
    app.wait_for("the installer to see it up", || installer_sees(app.disk(), plan));
    assert_eq!(launch_ok_says(app.disk()), Some((NEW.to_owned(), None)), "started, with no tab count");
}

/// Failure (the coordinator's decision on S3, 2026-11-13: an update is never rolled back for a launch that came up as a second copy): a new version an update
/// opened, whose old copy still holds the record, is worked in by the person before the lock comes (a file opened at two seconds); it stops trying, as any
/// launch does, and says so, but tells the installer nothing, so the installer waits its two minutes, stops the window the person is working in and swaps the
/// old version back. It's a second copy, says so, runs the person's open at once, tells the installer it started (no tab count), and writes nothing else: the
/// record's folder, `updates.json` and the plan as they were. The next launch, with the lock, restores the session and settles the install: the version proven.
#[test]
fn an_update_launch_worked_in_before_the_lock_comes_is_a_second_copy_that_started() {
    use crate::testkit::update_bed::NEW;
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let was = state_but_the_launch_ok(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    app.core().life.update_facts.first_frame_shown(&app.core().updates);
    next_try_fails(&app);
    next_try_fails(&app);
    let c = app.disk().pdf("c.pdf", 2);
    app.open(&c);
    assert_eq!(tab_names(&app), ["c"], "the person's open, at once");
    a_second_copy_that_started(&app, &plan);
    assert_eq!(app.core().life.keeping(), Keeping::NotKept);
    app.core().saver.write_soon();
    app.settle();
    let (_, disk) = app.quit();
    assert!(state_but_the_launch_ok(&disk) == was, "the record, updates.json and the plan as they were");
    drop(old_on_its_way_out);
    disk.installer_saw_it_up();
    let next = disk.builder().update(&bed).start();
    assert_eq!(tab_names(&next), ["alpha"], "the next launch restores the session the hand-off wrote");
    assert_eq!(page_of(&next, "alpha.pdf"), 8);
    next.core().life.update_facts.first_frame_shown(&next.core().updates);
    next.wait_until("the install settled: the version proven", || {
        recto_update::saved::Saved::read(&next.disk().update_files().saved).proven.as_deref() == Some(NEW)
    });
    drop(next.quit());
}

/// Failure (the coordinator's decision on S3): a launch an update opened gives up at ten seconds like any other, so an old copy that lets go at 45 s leaves the
/// new version a second copy, and the session the hand-off wrote doesn't come back until the next launch. Untouched, it tries on, keeps the record at 45 s,
/// restores it, isn't a second copy, and tells the installer it came up with the tab it was handed.
#[test]
fn an_update_launch_left_alone_keeps_a_lock_that_comes_at_45_seconds() {
    use crate::testkit::update_bed::NEW;
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let app = disk.builder().update(&bed).another_keeps().start();
    app.core().life.update_facts.first_frame_shown(&app.core().updates);
    for _ in 1..45 {
        next_try_fails(&app);
    }
    assert_eq!(app.reader().note, None, "nothing said in 44 seconds");
    drop(old_on_its_way_out);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["alpha"]);
    app.wait_for("the installer to see it up", || installer_sees(app.disk(), &plan));
    assert_eq!(launch_ok_says(app.disk()), Some((NEW.to_owned(), Some(1))), "up, with the tab it read");
    assert_eq!(app.update_status()["second_copy"], false);
}

/// Failure (the coordinator's decision on S3): a launch an update opened and left alone, whose record another copy keeps for good, tries for ever, or gives up
/// telling the installer nothing, which then rolls back a version that came up fine. At a minute it's a second copy, says so, and tells the installer it
/// started.
#[test]
fn an_update_launch_left_alone_for_a_minute_is_a_second_copy_that_started() {
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let app = disk.builder().update(&bed).another_keeps().start();
    app.core().life.update_facts.first_frame_shown(&app.core().updates);
    for _ in 1..crate::lifecycle::keeping::TRIES_AFTER_UPDATE {
        next_try_fails(&app);
    }
    assert_eq!(app.reader().note, None, "nothing said before the minute");
    app.advance(TRY_EVERY_MS);
    a_second_copy_that_started(&app, &plan);
    assert_eq!(app.core().life.keeping_due(), None, "no more tries");
    drop(old_on_its_way_out);
}

/// Failure (the coordinator's condition, 2026-11-13): a quit while a new version is still trying for the record tells the updater something it then writes (a
/// launch-ok, a settled install, the outcome or the plan tidied away), so the installer, still waiting, sees a launch that never kept the record as one that came
/// up. Nothing of the updater's changes: what the installer reads is exactly as it was.
#[test]
fn a_quit_while_it_tries_tells_the_updater_nothing() {
    let (disk, bed, _plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    let update_folder = |disk: &Disk| {
        let mut found = BTreeMap::new();
        for entry in std::fs::read_dir(disk.update_files().root).into_iter().flatten().flatten() {
            if let Ok(bytes) = std::fs::read(entry.path()) {
                found.insert(entry.path(), bytes);
            }
        }
        found
    };
    let was = update_folder(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    app.core().life.update_facts.first_frame_shown(&app.core().updates);
    next_try_fails(&app);
    let (answer, disk) = app.quit();
    assert!(answer.go, "{answer:?}");
    assert!(!disk.update_files().launch_ok().exists(), "no launch-ok");
    assert_eq!(update_folder(&disk), was, "the installer's view as it was");
    drop(old_on_its_way_out);
}

/// Failure (review, 2026-11-13): the version before, back after a new one that kept ending, can't read the record that one left, and its launch didn't get the
/// lock at once: the record from before the update was used up by the launch refused the lock, or never looked for, so the late keep brings back nothing. Told
/// of the launch only once it keeps the record, the late keep falls back to the record from before the update, as a launch does, and says so.
#[test]
fn a_late_keep_after_a_version_that_kept_ending_falls_back_to_the_record_from_before() {
    let (disk, bed, plan, old_on_its_way_out) = handed_off_with_the_old_copy_holding();
    drop(old_on_its_way_out);
    for slot in ["record.a.json", "record.b.json"] {
        std::fs::write(disk.update_files().session().join(slot), "damaged by the version that kept ending").expect("damaged");
    }
    let back = recto_update::plan::Plan { keep_record: true, ..plan };
    disk.went_back(&back, "kept_ending");
    let before = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    next_try_fails(&app);
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["alpha"], "the session from before the update");
    let notice = app.core().updates.notice().expect("it says so");
    assert!(notice.text.contains("from before the update"), "{notice:?}");
}

/// The saved session as the copy before left it, at the next launch: both tabs, the first one's unsaved turn, and nothing else lost.
pub(super) fn comes_back_at_the_next_launch(disk: Disk) {
    let again = disk.start();
    assert_eq!(tab_names(&again), ["a", "b"], "the saved session at the next launch");
    assert_eq!(
        again.session_json()["tabs"][0]["edits"]["edited"],
        true,
        "and the unsaved turn its journal kept"
    );
    drop(again.quit());
}

/// The quit's answer, once it's heard.
fn quit_answered(app: &HeadlessApp) -> bool {
    app.heard().iter().any(|heard| matches!(heard, Heard::QuitAnswer(_)))
}

/// Failure (review, 2026-11-13, route 1): a quit while it tries starts the quit's write, which gathers the empty window; a try then gets the lock and restores, and
/// the quit's write, now let past the lock, writes the empty window as the newest record and deletes the places log, so the next launch has no tabs and the
/// journals of unsaved edits are orphaned. The quit begins, the copy before lets go and a try comes due: no try is made once the quit has begun, the quit's write
/// writes nothing (it doesn't keep the record), the record's folder is byte for byte as it was, and the next launch has the saved session.
#[test]
fn a_quit_while_it_tries_writes_nothing_over_the_saved_session() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let on_disk = record_folder(&disk);
    let app = disk.builder().another_keeps().start();
    app.core().quit_requested();
    app.wait_until("no longer trying", || !app.core().life.keeping().trying());
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the quit's answer", || quit_answered(&app));
    app.settle();
    assert!(!app.core().saver.store.holds_the_record(), "no try once the quit began");
    let disk = app.quit_anyway();
    assert_eq!(record_folder(&disk), on_disk, "the saved session and its journal as the copy before left them");
    comes_back_at_the_next_launch(disk);
}

/// Failure (review, 2026-11-13, route 2): something written at once (the app going to the background) lands after a try got the lock and before the saved session
/// is in, and writes the empty window as the record's first slot and removes the places log, or leaves an empty newest slot on disk. The saved session's read
/// is held at its seam after the try, and a write comes due then: it writes nothing, and once the read's in, the session is the saved one, on disk and in the
/// window.
#[test]
fn a_write_due_while_the_saved_session_is_read_back_writes_nothing_over_it() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let on_disk = record_folder(&disk);
    let app = disk.builder().another_keeps().start();
    recto_platform::slow::gate::hold(&[Seam::State]);
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the saved session's read held at its seam", || app.waiting(Seam::State) > 0);
    assert!(app.core().life.keeping().restoring());
    app.core().saver.write_soon();
    settled_but_the_held_thread(&app);
    assert_eq!(record_folder(app.disk()), on_disk, "nothing written while it's read back");
    app.let_through(Seam::State, 1);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["a", "b"]);
    let (_, disk) = app.quit();
    comes_back_at_the_next_launch(disk);
}

/// Every writer of the record, each once, as each is made: a flush (a `session save`, a quit's or a hand-off's), the saver's own pass when something is due at
/// once, and the journals written ahead of a tab going dormant. What each said, for the flush.
fn every_writer(app: &HeadlessApp) -> Result<(), String> {
    let core = Arc::clone(app.core());
    let flushed = std::thread::Builder::new()
        .name("test-writer".into())
        .spawn(move || {
            core.saver.write_journals_now(core.on_disk.keeper(), core.on_disk.sleeper());
            core.saver.flush(false, WAIT_LIMIT).map(|_| ()).map_err(|error| error.code.to_owned())
        })
        .expect("a thread")
        .join()
        .expect("the writer answered");
    app.core().saver.write_soon();
    settled_but_the_held_thread(app);
    flushed
}

/// **Settled, but for the try or the late keep's read the test holds at its seam** on purpose, which the app's settle waits for ([`crate::settled`]): the
/// session thread through what's queued and at rest, the record's writer through, and nothing else under way.
fn settled_but_the_held_thread(app: &HeadlessApp) {
    assert!(app.core().core.flush(WAIT_LIMIT), "the session thread took what was queued");
    app.wait_for("settled but for the held try or read", || {
        let busy = crate::settled::under_way(&crate::settled::watched!(app.core()), crate::settled::PageViews::AtRest);
        if busy.iter().all(|what| what.contains("record's lock")) {
            Ok(())
        } else {
            Err(format!("{busy:?}"))
        }
    });
}

/// Failure (review, 2026-11-13, route 3, and S1: the try no longer holds the saver's gate): a writer of the record writes the window as it is (empty) at the
/// moment a late try has the lock: back from its thread and not yet decided on, or while the saved session is read back. It would be the newest record, with
/// the places log removed, so the next launch has no tabs and the journals of unsaved edits are orphaned. At each moment every writer writes nothing, the
/// record's folder is byte for byte as it was, and once the saved session is in it's what's there, now and at the next launch.
#[test]
fn a_writer_at_the_moment_of_a_try_writes_nothing_over_the_saved_session() {
    for moment in [Seam::RecordLock, Seam::State] {
        let disk = a_saved_session();
        let before = the_copy_before(&disk);
        let on_disk = record_folder(&disk);
        let app = disk.builder().another_keeps().start();
        recto_platform::slow::gate::hold(&[moment]);
        drop(before);
        app.advance(TRY_EVERY_MS);
        app.wait_until("the try held at the moment", || app.waiting(moment) > 0);
        assert!(app.core().saver.store.holds_the_record(), "{moment:?}: the try has the lock");
        let flushed = every_writer(&app);
        assert!(flushed.is_err(), "{moment:?}: the flush is refused: {flushed:?}");
        assert_eq!(record_folder(app.disk()), on_disk, "{moment:?}: nothing written");
        app.let_through(moment, 1);
        app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
        app.settle();
        assert_eq!(tab_names(&app), ["a", "b"], "{moment:?}");
        let (_, disk) = app.quit();
        comes_back_at_the_next_launch(disk);
    }
}

/// Failure (review, 2026-11-13): an agent's `open` while it tries either counts as the person working in the window (stopping the retry and showing the person a
/// notice for what an agent did) or lands while the saved session's being read and is replaced by it, its tab number then naming one of the owner's tabs. It's
/// refused `busy`, with a next step, and changes nothing; an agent's question is answered; the trying goes on and keeps the record when the lock comes.
#[test]
fn an_agents_change_while_it_tries_is_refused_and_the_trying_goes_on() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    let c = app.disk().pdf("c.pdf", 2);
    let refused = app
        .agent(Level::Full, &format!(r#"{{"cmd":"open","path":"{}"}}"#, c.display()))
        .expect_err("refused while it tries");
    assert_eq!(refused.code, "busy", "{refused:?}");
    assert!(refused.next.contains("again"), "{refused:?}");
    let restore = app.agent(Level::Full, r#"{"cmd":"session_restore"}"#).expect_err("refused too");
    assert_eq!(restore.code, "busy", "an agent's restore can't take the lock outside the machine: {restore:?}");
    let asked = app.agent(Level::Full, r#"{"cmd":"session_show"}"#);
    assert!(asked.is_ok(), "a question's answered: {asked:?}");
    app.settle();
    assert!(app.tabs().is_empty(), "nothing opened");
    assert_eq!(tries(&app), Some(0), "still trying");
    assert_eq!(app.reader().note, None, "nothing said to the person");
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["a", "b"]);
    let page = app.agent(Level::Full, r#"{"cmd":"goto","page":2}"#);
    assert!(page.is_ok(), "and an agent's change is taken once it keeps the record: {page:?}");
    drop(app.quit());
}

/// What can come between a late try taking the record's lock and the app deciding to keep it.
#[derive(Clone, Copy, Debug)]
enum Between {
    /// The person opens a file.
    AnOpen,
    /// The person asks to quit.
    AQuit,
}

/// The window's own calls (the UI thread's), each answered within `limit` while a try is held on the disk, or which didn't answer.
fn the_window_answers_within(app: &HeadlessApp, limit: std::time::Duration) -> Result<(), &'static str> {
    let core = Arc::clone(app.core());
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("test-window".into())
        .spawn(move || {
            let _ = core.life.state_read();
            let _ = core.life.window_interactive();
            let _ = core.life.may_remake_window();
            let _ = said.send(());
        })
        .expect("a thread");
    heard.recv_timeout(limit).map_err(|_| "the window's calls waited on the try")
}

/// Failure (review, 2026-11-13, S1): a late try at the record's lock is made holding the app's life lock, which the UI thread takes to ask whether the state's
/// read, whether the window takes input and whether to make it again, and for a quit: a try's open of the lock file on a sleeping disk then stalls the window for
/// as long as the disk takes. And once the try is made off that lock, a quit or the person's change that comes between the lock being taken and the app deciding
/// must not leave the lock kept, or anything written over the saved session. The try held on the disk: the window answers, the quit and the open are taken;
/// let go, the lock is let go again at once, the record's folder is byte for byte as it was, and the next launch has the saved session.
#[test]
fn a_try_held_on_the_disk_holds_up_nothing_and_what_comes_meanwhile_wins() {
    for between in [Between::AnOpen, Between::AQuit] {
        let disk = a_saved_session();
        let before = the_copy_before(&disk);
        let on_disk = record_folder(&disk);
        let app = disk.builder().another_keeps().hold(&[Seam::RecordLock]).start();
        drop(before);
        app.advance(TRY_EVERY_MS);
        app.wait_until("the try held on the disk", || app.waiting(Seam::RecordLock) > 0);
        assert_eq!(the_window_answers_within(&app, std::time::Duration::from_secs(5)), Ok(()), "{between:?}");
        match between {
            Between::AnOpen => {
                let c = app.disk().pdf("c.pdf", 2);
                let _ = app.act(crate::core::controls::UiAction::Open { path: c.display().to_string() });
                app.wait_until("the person's open taken", || tab_names(&app) == ["c"]);
            }
            Between::AQuit => app.core().quit_requested(),
        }
        app.let_through(Seam::RecordLock, 1);
        app.wait_until("the try decided", || !app.core().life.keeping().trying());
        app.wait_until("the lock let go again", || !app.core().saver.store.holds_the_record());
        let disk = match between {
            Between::AnOpen => {
                assert_eq!(app.reader().note, not_kept_words(), "said, as for any change before the lock");
                assert_eq!(tab_names(&app), ["c"], "what the person has, and nothing restored over it");
                app.core().saver.write_soon();
                app.settle();
                app.quit().1
            }
            Between::AQuit => {
                app.wait_until("the quit's answer", || quit_answered(&app));
                app.quit_anyway()
            }
        };
        assert_eq!(
            record_folder(&disk),
            on_disk,
            "{between:?}: the saved session and its journal as the copy before left them"
        );
        comes_back_at_the_next_launch(disk);
    }
}

/// Failure (review, 2026-11-13, S2): agents can keep a late try from ever getting the lock, so the person's saved session isn't restored: every refused change
/// of an agent's was counted as under way before it was refused, and a try that found one under way was a try that didn't get the lock; and an agent's
/// `session save`, which only asks, wasn't refused, took the saver's gate and gathered, and a try that found the gate held didn't get the lock either. Eight
/// agents loop changes and saves for the whole ten seconds of the fake clock: every try is made, each refused change says `busy`, and the lock let go before the
/// ninth try is kept there and the saved session comes back.
#[test]
fn eight_agents_looping_changes_and_saves_dont_stop_the_tries() {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    let c = app.disk().pdf("c.pdf", 2);
    let stop = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicU32::new(0));
    let agents: Vec<_> = (0..8)
        .map(|agent| {
            let (core, stop, busy) = (Arc::clone(app.core()), Arc::clone(&stop), Arc::clone(&busy));
            let lines = [
                format!(r#"{{"cmd":"open","path":"{}"}}"#, c.display()),
                r#"{"cmd":"session_save"}"#.to_owned(),
                r#"{"cmd":"view","paper":"dark"}"#.to_owned(),
                r#"{"cmd":"goto","page":2}"#.to_owned(),
            ];
            std::thread::Builder::new()
                .name(format!("test-agent-{agent}"))
                .spawn(move || {
                    let mut sent = agent;
                    while !stop.load(Ordering::SeqCst) {
                        let command = recto_agent::parse(&lines[sent % lines.len()]).expect("a command");
                        let answer = recto_control::host::Host::run(&*core.host, recto_agent::Actor::Agent(Level::Full), &command, &mut Vec::new());
                        if answer.is_err_and(|refused| refused.code == "busy") {
                            busy.fetch_add(1, Ordering::SeqCst);
                        }
                        sent += 1;
                    }
                })
                .expect("a thread")
        })
        .collect();
    for _ in 1..9 {
        next_try_fails(&app);
        assert_eq!(
            app.core().host.running.load(Ordering::SeqCst),
            0,
            "no agent's change is ever counted as under way while it tries"
        );
    }
    assert_eq!(tries(&app), Some(8), "every try made with the agents at it");
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept at the ninth try", || app.core().life.keeping() == Keeping::Keeps);
    stop.store(true, Ordering::SeqCst);
    for agent in agents {
        agent.join().expect("an agent's thread");
    }
    app.settle();
    assert!(busy.load(Ordering::SeqCst) > 0, "the agents' changes were refused busy while it tried");
    assert_eq!(&tab_names(&app)[..2], ["a", "b"], "the saved session came back");
    assert_eq!(app.reader().note, None, "and nothing was said to the person");
    drop(app.quit());
}

/// Failure (S2, with the agents' count gone from the try): a person's command running on its own thread (an export, a check for updates), whose change isn't in
/// to be looked at yet, is under way as a try comes back with the lock; the restore then replaces what it does, or what it does lands on the restored session
/// as an agent's would. The lock is given back, for now, and not let go for good: the trying goes on, nothing is said, and the next try, with nothing under way,
/// keeps the record and the saved session comes back.
#[test]
fn a_try_back_with_a_persons_command_under_way_gives_the_lock_back_for_the_next() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().hold(&[Seam::RecordLock]).start();
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the try held with the lock", || app.waiting(Seam::RecordLock) > 0);
    let person = app.core().life.person_running();
    app.let_through(Seam::RecordLock, 1);
    app.wait_until("given back", || tries(&app) == Some(1) && !app.core().saver.store.holds_the_record());
    assert_eq!(app.reader().note, None, "nothing said");
    drop(person);
    recto_platform::slow::gate::open(Seam::RecordLock);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept at the next try", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["a", "b"]);
    drop(app.quit());
}

/// How the late keep's read of the saved session goes wrong.
#[derive(Clone, Copy, Debug)]
enum ReadGoes {
    /// It never comes back (a dead disk): held at its seam past the restore's patience, then let through.
    NeverBack,
    /// Its thread panics.
    Panics,
}

/// Failure (review, 2026-11-13, S4): nothing bounded a late keep's restore, so a read on a dead disk or one whose thread panicked held what the person did and
/// refused every agent for good, with the record's lock kept and nothing written. The restore ends: past its patience on the fake clock (a read that never
/// comes back) or at once (a panic), the window goes on as it is, the person's action that waited for it runs, the notice says the window won't be remembered,
/// the lock is let go and the record's folder is byte for byte as it was. A read that comes in after that is dropped, not put over what the person has, and the
/// next launch has the saved session.
#[test]
fn a_late_keeps_read_that_never_comes_back_or_panics_ends_the_restore() {
    for goes in [ReadGoes::NeverBack, ReadGoes::Panics] {
        let disk = a_saved_session();
        let before = the_copy_before(&disk);
        let on_disk = record_folder(&disk);
        let app = disk.builder().another_keeps().start();
        match goes {
            ReadGoes::NeverBack => recto_platform::slow::gate::hold(&[Seam::State]),
            ReadGoes::Panics => crate::core::keeper::panic_in_the_late_read(),
        }
        drop(before);
        app.advance(TRY_EVERY_MS);
        if let ReadGoes::NeverBack = goes {
            app.wait_until("the read held", || app.waiting(Seam::State) > 0);
            assert!(app.core().life.keeping().restoring());
            let c = app.disk().pdf("c.pdf", 2);
            let _ = app.act(crate::core::controls::UiAction::Open { path: c.display().to_string() });
            settled_but_the_held_thread(&app);
            assert!(tab_names(&app).is_empty(), "the person's open waits for the restore");
            app.advance(crate::lifecycle::keeping::RESTORING_PATIENCE_MS);
        }
        app.wait_until("the restore over", || app.core().life.keeping().not_kept());
        app.settle();
        assert_eq!(app.reader().note, not_kept_words(), "{goes:?}");
        if let ReadGoes::NeverBack = goes {
            assert_eq!(tab_names(&app), ["c"], "the person's open ran on the window as it is");
            assert!(app.core().saver.store.holds_the_record(), "the lock stays with the read while it's inside");
            app.let_through(Seam::State, 1);
            app.wait_until("the read's thread out", || app.core().life.keeping() == Keeping::NotKept);
            app.settle();
            assert_eq!(tab_names(&app), ["c"], "the read that came late wasn't put over it");
        }
        app.wait_until("the lock let go", || !app.core().saver.store.holds_the_record());
        app.core().saver.write_soon();
        app.settle();
        let (_, disk) = app.quit();
        assert_eq!(
            record_folder(&disk),
            on_disk,
            "{goes:?}: the saved session and its journal as the copy before left them"
        );
        comes_back_at_the_next_launch(disk);
    }
}

/// A late keep held with its saved session being read: the copy before let go, a try took the lock, and the read waits at its seam.
fn restoring_held(disk: Disk) -> HeadlessApp {
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    recto_platform::slow::gate::hold(&[Seam::State]);
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the saved session's read held", || app.waiting(Seam::State) > 0);
    assert!(app.core().life.keeping().restoring());
    app
}

/// The restore let in, and kept.
fn let_the_restore_in(app: &HeadlessApp) {
    app.let_through(Seam::State, 1);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
}

/// Failure (review, 2026-11-13, S5: "the last thing a person asked for wins"): while a late keep's saved session is read, what the person does is held and runs
/// on it once it's in, but a command that runs on a thread of its own (⌘K's palette running a row, a check for updates, anything that may wait) wasn't held:
/// it ran on the window before the restore, and the restore replaced what it did. Dark paper chosen from the palette while it's read is the paper once it's
/// in.
#[test]
fn a_palette_row_run_while_the_saved_session_is_read_lands_on_it() {
    let app = restoring_held(a_saved_session());
    app.person(r#"{"cmd":"palette","query":"dark paper","run":true}"#);
    settled_but_the_held_thread(&app);
    let_the_restore_in(&app);
    assert_eq!(tab_names(&app), ["a", "b"]);
    let paper = app.core().host.session.with_session(|session| session.viewing().view().paper);
    assert_eq!(paper, Some(recto_session::Paper::Dark), "the person's last choice, on the restored session");
    drop(app.quit());
}

/// Failure (review, 2026-11-13, S5): the person sets the appearance (which the window sets directly, not by a command) while a late keep's saved session is read,
/// and the restore puts the saved appearance over it, and the window then flips back to the saved one. What the person chose last stays.
#[test]
fn an_appearance_chosen_while_the_saved_session_is_read_stays() {
    use recto_session::record::Appearance;
    let app = restoring_held(a_saved_session());
    assert_eq!(
        app.core().window.app_record(|| None).appearance,
        Appearance::System,
        "as the copy before left it"
    );
    crate::lock(&app.core().window.app).appearance = Appearance::Dark;
    app.core().saver.view_changed();
    let_the_restore_in(&app);
    assert_eq!(tab_names(&app), ["a", "b"]);
    assert_eq!(app.core().window.app_record(|| None).appearance, Appearance::Dark, "what the person chose last");
    drop(app.quit());
}

/// Failure (review, 2026-11-13, also 4: the host now applies Rust's appearance when it hears a late state it doesn't make the window again from, which this
/// branch added for a late keep): an ordinary state that comes in after the window went up without it and has taken input (past its ten seconds) isn't made
/// into the window, but the saved appearance was put in the app's state over the one the window shows, so the host now flips the window to it, over what the
/// person chose meanwhile. The window that's taking input keeps its appearance, and the record says it; the rest of the session comes back into it.
#[test]
fn an_ordinary_late_state_keeps_the_appearance_the_window_shows_once_it_takes_input() {
    use recto_session::record::Appearance;
    let app = a_saved_session().builder().hold(&[Seam::State]).start();
    app.advance(u64::try_from(crate::core::updates::INPUT_PATIENCE.as_millis()).expect("ms"));
    assert!(app.core().life.window_interactive(), "past its patience, the window takes input");
    crate::lock(&app.core().window.app).appearance = Appearance::Dark;
    app.core().saver.view_changed();
    app.let_state_in();
    app.settle();
    assert_eq!(tab_names(&app), ["a", "b"], "the session comes back into the window");
    assert_eq!(crate::lock(&app.host).remade, 0, "not made again: it's taking input");
    assert_eq!(
        app.core().window.app_record(|| None).appearance,
        Appearance::Dark,
        "what the window shows, which the host applies"
    );
    drop(app.quit());
}

/// Failure (review, 2026-11-13: only opening a file was tested): the person changes the paper, or the appearance, before the lock comes, and the restore then
/// replaces their choice, or keeps the record with it and writes it over the saved session. Each stops the trying and says so (the paper at once, as a call on
/// the session thread; the appearance, set by the window directly, by the look before the next try), and the saved session waits for the next launch.
#[test]
fn changing_the_paper_or_the_appearance_while_it_tries_stops_it() {
    for change in ["paper", "appearance"] {
        let disk = a_saved_session();
        let before = the_copy_before(&disk);
        let on_disk = record_folder(&disk);
        let app = disk.builder().another_keeps().start();
        if change == "paper" {
            app.person(r#"{"cmd":"view","paper":"dark"}"#);
            app.settle();
            assert_eq!(app.reader().note, not_kept_words(), "said at once");
        } else {
            crate::lock(&app.core().window.app).appearance = recto_session::record::Appearance::Dark;
            app.core().saver.view_changed();
        }
        drop(before);
        app.advance(TRY_EVERY_MS);
        app.wait_until("the notice", || app.reader().note == not_kept_words());
        app.advance(5 * TRY_EVERY_MS);
        app.settle();
        assert_eq!(app.core().life.keeping(), Keeping::NotKept, "{change}");
        assert!(!app.core().saver.store.holds_the_record(), "{change}: no more tries");
        assert!(app.tabs().is_empty(), "{change}: nothing restored over it");
        let (_, disk) = app.quit();
        assert_eq!(record_folder(&disk), on_disk, "{change}");
        comes_back_at_the_next_launch(disk);
    }
}

/// Failure: hiding the sidebar, which is the window's arrangement like its frame (and is replaced by the restore as a launch would set it), stops the trying,
/// so a person who only tidied the window loses the saved session to a notice. It doesn't, and the lock coming restores as a launch would.
#[test]
fn hiding_the_sidebar_while_it_tries_doesnt_stop_it() {
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let app = disk.builder().another_keeps().start();
    app.person(r#"{"cmd":"view","sidebar":"off"}"#);
    app.settle();
    assert_eq!(tries(&app), Some(0), "still trying");
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["a", "b"]);
    assert_eq!(app.reader().note, None);
    drop(app.quit());
}

/// Failure (owner, 2026-11-13: "restored as a launch would have"): the saved session comes back and the window keeps the frame it went up with, while the record
/// says the saved one, so the two disagree; or the window's made again twice, or made again when the person had already touched it, closing what they're
/// using. Untouched, it's made again once, from the saved frame. Touched before the lock comes, or with a sheet up (an agent's permission request, which would
/// go with the window), it isn't made again: what's in it comes back, and the record says where the real window is.
#[test]
fn a_late_keep_makes_the_window_again_once_unless_the_person_touched_it() {
    for (touched, sheet) in [(false, false), (true, false), (false, true)] {
        let disk = a_saved_session();
        let before = the_copy_before(&disk);
        let app = disk.builder().another_keeps().hold(&[Seam::Remake]).start();
        window_at(&app, REAL_X);
        if touched {
            app.touch();
        }
        // An agent's permission request up on the window, as the host says a sheet is.
        app.core().life.sheet_changed(sheet);
        app.settle();
        assert_eq!(tries(&app), Some(0), "touching the window isn't working in it");
        drop(before);
        app.advance(TRY_EVERY_MS);
        app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
        app.settle();
        app.wait_until("the host told", || app.waiting(Seam::Remake) > 0);
        app.let_through(Seam::Remake, 1);
        app.settle();
        assert_eq!(
            tab_names(&app),
            ["a", "b"],
            "touched {touched}, a sheet up {sheet}: the session comes back either way"
        );
        let remade = crate::lock(&app.host).remade;
        if touched || sheet {
            assert_eq!(remade, 0, "not made again under the person, or under a sheet");
            assert_eq!(window_x(&app), Some(REAL_X), "the record says where the real window is");
        } else {
            assert_eq!(remade, 1, "made again once");
            assert_eq!(window_x(&app), Some(SAVED_X), "from the saved frame");
        }
        if sheet {
            app.core().life.sheet_changed(false);
        }
        assert!(!app.core().life.may_remake_window(), "and never again this launch");
        drop(app.quit());
    }
}
