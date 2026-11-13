//! **A launch that doesn't get the record's lock at its start** (owner, 2026-11-13): it tries again quietly for ten seconds and keeps the record once it has it,
//! restoring the saved session as a launch would have; another copy that really keeps it is said as before, after the ten seconds; and a person who starts
//! working in the window before the lock comes is told at once that it won't be remembered. The copy before it is a store the test holds, let go of when the
//! test says, and the ten seconds are the fake clock's. The tries' spacing is the pure machine's, tested in `lifecycle::keeping`.

use crate::lifecycle::keeping::{Keeping, TRIES, TRY_EVERY_MS};
use crate::testkit::headless::{Disk, HeadlessApp, WAIT_LIMIT};
use recto_control::record_store::{Flavor, RecordStore};
use recto_session::record::notices;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The words of the notice a window that won't be remembered shows.
fn not_kept_words() -> Option<String> {
    notices::words(notices::OTHER_INSTANCE).map(str::to_owned)
}

/// The copy before this one, still holding the record: a store of the test's on the disk.
fn the_copy_before(disk: &Disk) -> RecordStore {
    let store = RecordStore::open(disk.paths(), Flavor::App);
    assert!(store.holds_the_record(), "the copy before keeps the record");
    store
}

/// How many tries it has made, while it's trying.
fn tries(app: &HeadlessApp) -> Option<u32> {
    match app.core().life.keeping() {
        Keeping::Trying { tries, .. } => Some(tries),
        _ => None,
    }
}

/// The clock moved on to the next try, and that try made (and failed).
fn next_try_fails(app: &HeadlessApp) {
    let before = tries(app).expect("trying");
    app.advance(TRY_EVERY_MS);
    app.wait_until("the next try", || tries(app) != Some(before));
    app.settle();
}

/// Every file of the record's folder, by its bytes: what a window that doesn't keep the record must leave as it is.
fn record_folder(disk: &Disk) -> BTreeMap<PathBuf, Vec<u8>> {
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

/// A session of two tabs, the first with a page turned and not saved, quit as the copy before: its record and the first tab's journal on disk.
fn a_saved_session() -> Disk {
    let app = HeadlessApp::start();
    let (a, b) = (app.disk().pdf("a.pdf", 3), app.disk().pdf("b.pdf", 3));
    app.open(&a);
    app.open(&b);
    app.person(r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##);
    app.settle();
    let (answer, disk) = app.quit();
    assert!(answer.go, "{answer:?}");
    disk
}

/// The page, counted from 0, of the tab on the file named.
fn page_of(app: &HeadlessApp, file: &str) -> u64 {
    let session = app.session_json();
    let tab = session["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["path"].to_string().contains(file));
    tab.and_then(|tab| tab["page"].as_u64())
        .unwrap_or_else(|| panic!("no tab on {file}: {session}"))
}

fn tab_names(app: &HeadlessApp) -> Vec<String> {
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
                let _ = core.ready_to_show();
                let _ = core.state_read();
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

/// Failure (owner, 2026-11-07: "It's crucial that app state is perfectly recovered before and after update"): the new version's launch asks for the record while
/// the old version is still letting go of it, in a hand-off's last moments, and starts empty and unremembered, as it did when it asked only once. It keeps the
/// record once the old one lets go, with the session as the hand-off wrote it, on the new version.
#[test]
fn an_update_whose_old_copy_still_holds_the_record_comes_back_as_it_was() {
    use crate::testkit::update_bed::{Installed, NEW, OLD, UpdateBed};
    use recto_agent::Level;
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
    let app = disk.builder().update(&bed).another_keeps().start();
    assert_eq!(app.disk().app_version().as_deref(), Some(NEW));
    assert!(app.tabs().is_empty() && app.reader().note.is_none(), "trying, quietly");
    next_try_fails(&app);
    drop(old_on_its_way_out);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    assert_eq!(tab_names(&app), ["alpha"]);
    assert_eq!(page_of(&app, "alpha.pdf"), 8, "the page it was on: {}", app.session_json());
    assert_eq!(app.reader().note, None);
}
