//! **A quit, as the app's life steps it** (step R1.4): its freeze, its write, its answers, and what the updater is told of each, on the app's own core.

use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{OLD, UpdateBed};
use recto_platform::slow::Seam;
use recto_update::saved::{Probation, Saved};
use std::sync::atomic::Ordering;

/// An installed copy of [`OLD`] whose run is being watched, as one is in its first minutes after an update.
fn watched_copy() -> (Disk, UpdateBed) {
    let disk = Disk::new();
    disk.install_app(OLD);
    Saved {
        proven: Some(OLD.to_owned()),
        probation: Some(Probation {
            version: OLD.to_owned(),
            since_ms: recto_update::wall_ms(),
            running_since_ms: None,
            early_ends: 0,
        }),
        ..Saved::default()
    }
    .write(&disk.update_files().saved);
    (disk, UpdateBed::new())
}

/// The state folder's own part that the record goes in, made so it can't be written, or writable again.
#[cfg(unix)]
fn record_folder_writable(app: &HeadlessApp, writable: bool) {
    use std::os::unix::fs::PermissionsExt as _;
    let folder = app.core().saver.store.main_folder();
    std::fs::create_dir_all(&folder).expect("the record's folder");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(if writable { 0o700 } else { 0o500 })).expect("the record's folder");
}

/// Failure (63cc07b, 2026-11-11): a quit whose state couldn't be written, with edits it would lose, answers go and loses them; or Quit Anyway counts as a run that
/// ended early, so a good version that once couldn't write its state is sent back; or what a person does stays held back while the app asks; or Stay leaves the
/// run unwatched, so a crash after it isn't counted. The record's folder made unwritable under an owner's edit: the quit asks, the run's no longer watched and
/// the app takes input; Stay, and it's watched again; asked again and Quit Anyway, and the next launch counts no early end.
#[cfg(unix)]
#[test]
fn quit_anyway_isnt_an_early_end_and_stay_is_watched_again() {
    let (disk, bed) = watched_copy();
    let files = disk.update_files();
    let probation = || Saved::read(&files.saved).probation.expect("watched");
    let doc = disk.pdf("edited.pdf", 3);
    let app = disk.builder().update(&bed).start();
    app.open(&doc);
    app.act(crate::core::controls::UiAction::Menu { id: "rotate.right".into() });
    app.settle();
    assert!(probation().running_since_ms.is_some(), "this run's watched");
    record_folder_writable(&app, false);
    let asked = app.ask_to_quit();
    assert!(!asked.go && !asked.quit_button.is_empty(), "the edits couldn't be kept, so it asks: {asked:?}");
    assert_eq!(probation().running_since_ms, None, "asked for, so not an early end, whatever's answered");
    assert!(!app.core().core.frozen.load(Ordering::SeqCst), "what a person does is taken while it asks");
    app.stay();
    assert!(probation().running_since_ms.is_some(), "stayed: watched again, so a crash from here counts");
    assert!(!app.ask_to_quit().go, "asked again");
    record_folder_writable(&app, true);
    let disk = app.quit_anyway();
    let app = disk.builder().update(&bed).start();
    assert_eq!(probation().early_ends, 0, "Quit Anyway isn't an early end");
    drop(app.quit());
}

/// How many tabs the record on disk names: the newest of its two slots, as the next launch reads it.
fn tabs_on_disk(app: &HeadlessApp) -> usize {
    let folder = app.core().saver.store.main_folder();
    ["record.a.json", "record.b.json"]
        .iter()
        .filter_map(|name| std::fs::read(folder.join(name)).ok())
        .filter_map(|bytes| recto_session::record::decode_record(&bytes, recto_session::record::CURRENT_VERSION).ok())
        .max_by_key(|decoded| decoded.seq)
        .map_or(0, |decoded| decoded.record.tabs.len())
}

/// Failure (slow mode, 2026-11-11, 80af1aa and d65da39: `a_thousand_tabs_come_back_cold` came back with 2 of its 1000): a quit waits for the state, and goes
/// without it when nothing's unsaved, and the state was written after the tab on screen's sleep file, which waited behind every sleep file the tabs just opened
/// had queued, one at a time; with sleep files slow to write, as on a busy disk, a quit after opening tabs lost them all. Two tabs and back from a quit with them,
/// then the rest opened one after another with every sleep file's write held at its seam: the quit's state is on disk with every tab before a single sleep file
/// is let through, and every tab comes back. (Replaces the app test `tabs_just_opened_come_back_when_sleep_files_are_slow`.)
#[test]
fn tabs_just_opened_come_back_when_their_sleep_files_are_late() {
    const TABS: usize = 6;
    let disk = Disk::new();
    let docs: Vec<_> = (0..TABS).map(|at| disk.pdf(&format!("t{at}.pdf"), 3)).collect();
    let app = disk.start();
    app.open(&docs[0]);
    app.open(&docs[1]);
    let (answer, disk) = app.quit();
    assert!(answer.go);
    // Each has its sleep file to wake from now, read as it wakes; the writes are held.
    let app = disk.builder().hold(&[Seam::SleepWrite]).start();
    assert_eq!(app.tabs().len(), 2, "both back");
    for doc in &docs[2..] {
        app.open(doc);
    }
    assert_eq!(app.tabs().len(), TABS);
    app.wait_until("sleep files waiting to be written", || app.waiting(Seam::SleepWrite) > 0);
    // The quit's asked for, and the host's call returns at once: the state goes to the disk while not one sleep file has been written.
    app.core().quit_requested();
    app.wait_until("the quit's state on disk with every tab, with the sleep files still held", || {
        tabs_on_disk(&app) == TABS
    });
    assert_eq!(
        recto_platform::slow::gate::arrived(Seam::SleepWrite),
        app.waiting(Seam::SleepWrite),
        "none let through"
    );
    recto_platform::slow::gate::open(Seam::SleepWrite);
    app.wait_until("the quit's answer", || {
        app.heard()
            .iter()
            .any(|heard| matches!(heard, crate::testkit::Heard::QuitAnswer(answer) if answer.go))
    });
    let disk = app.quit_anyway();
    let app = disk.start();
    assert_eq!(app.tabs().len(), TABS, "every tab came back");
    drop(app.quit());
}

/// Failure (final review N4): a quit during a read that's stuck wrote nothing at all, as what's right for a hand-off isn't for a quit: there the update waits,
/// but here the app's going anyway, and the reader's place came back as page 1. A file's read held back, the reader moved meanwhile, a quit: the next launch is
/// at the place. (Replaces the app test `a_quit_during_a_stuck_read_still_writes`: nothing of it was the window's.)
#[test]
fn a_quit_during_a_stuck_read_still_writes() {
    let app = HeadlessApp::start();
    let (file, other) = (app.disk().pdf("stuck.pdf", 20), app.disk().pdf("other.pdf", 2));
    app.open(&file);
    app.core().qa_session_verb("hold_reads", "on").expect("a QA verb").expect("reads held");
    app.act(crate::core::controls::UiAction::Open {
        path: other.display().to_string(),
    });
    app.wait_until("the other file's read to be under way", || app.core().life.reads_running() > 0);
    app.recto(recto_agent::Level::Full, &["goto", "12"]).expect("goto");
    let answer = app.ask_to_quit();
    assert!(answer.go, "{answer:?}");
    // The real app's process ends with the read still stuck; here the read is let go so the core can end, with the quit already written and the app frozen.
    app.core().qa_session_verb("hold_reads", "off").expect("a QA verb").expect("reads let go");
    let again = app.quit_anyway().start();
    assert_eq!(again.reader().place, "12 of 20", "the place the quit wrote, with a read stuck");
    drop(again.quit());
}

/// Failure (review of 2026-11-13): the quit's own thread couldn't be started, as on a system out of threads, and the quit never answered, so the app stayed
/// frozen for the hand-off with no way out. It answers as a write that didn't finish: with an unsaved edit, it asks.
#[test]
fn a_quit_whose_thread_wont_start_still_answers() {
    let app = crate::testkit::headless::HeadlessApp::start();
    let file = app.disk().pdf("a.pdf", 3);
    app.open(&file);
    app.act(crate::core::controls::UiAction::Menu { id: "rotate.right".into() });
    app.settle();
    crate::core::quitting::QUIT_THREAD_REFUSED.store(true, std::sync::atomic::Ordering::SeqCst);
    let answer = app.ask_to_quit();
    crate::core::quitting::QUIT_THREAD_REFUSED.store(false, std::sync::atomic::Ordering::SeqCst);
    assert!(!answer.go, "the unsaved edit isn't known to be kept, so it asks: {answer:?}");
}

/// Failure (coordinator, 2026-11-13, found beside R2.3's S1): what a quit that couldn't write the state and `recto stop` ask about, the tabs that would lose
/// something, misses a tab whose edit list is asleep: its unsaved edits are read from the list's summary, but its undone steps only from a list in memory, so
/// a tab asleep with only undone steps isn't named, and a quit goes ahead without asking. Both are named, asleep as awake.
#[test]
fn a_tab_asleep_is_named_among_those_that_would_lose_something() {
    let disk = Disk::new();
    let (a, b, c) = (disk.pdf("a.pdf", 3), disk.pdf("b.pdf", 3), disk.pdf("c.pdf", 3));
    let app = disk.start();
    for file in [&a, &b, &c] {
        app.open(file);
    }
    for line in [
        r##"{"cmd":"page_rotate","tab":"#1","pages":"1","degrees":90}"##,
        r##"{"cmd":"page_rotate","tab":"#2","pages":"1","degrees":90}"##,
        r##"{"cmd":"undo","tab":"#2"}"##,
        r##"{"cmd":"tap","tab":"#3"}"##,
    ] {
        app.person(line);
        app.settle();
    }
    for tab in [1, 2] {
        let slept = app
            .agent(recto_agent::Level::Full, &format!(r##"{{"cmd":"session_sleep","tab":"#{tab}"}}"##))
            .expect("put to sleep");
        assert_eq!(slept["list_asleep"], true, "#{tab}: {slept}");
    }
    let named: Vec<u32> = recto_control::host::Host::unsaved_tabs(&app.core().host.session)
        .into_iter()
        .map(|(id, _)| id.0)
        .collect();
    assert_eq!(named, vec![1, 2], "the edited tab and the one with only undone steps, both asleep");
}
