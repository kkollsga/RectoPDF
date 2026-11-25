//! **Tabs going to sleep, on the app's own core:** the sleep files' one writer and its queue when the disk is slow, memory pressure meanwhile, and what's left at
//! rest. The decisions are [`crate::sleep_writer`]'s and [`crate::sleep_queue`]'s; these hold the threads and the session thread that carry them out.

use crate::testkit::headless::Disk;
use recto_platform::slow::Seam;

/// This test process's threads named `recto-sleep`: the sleep files' writers. A test is a process of its own under nextest, so they're all this test's app's.
fn sleep_writers() -> usize {
    recto_platform::local::thread_names(std::process::id())
        .expect("this process's threads")
        .iter()
        .filter(|name| *name == "recto-sleep")
        .count()
}

/// Failure (3a0f8b2, slow mode 2026-11-11: 39 at once in `a_thousand_tabs_come_back_cold`): each tab pushed out of the warm ones got a sleep-writer thread of its
/// own, waiting on the one-at-a-time lock, so with a slow disk the app's threads and memory grew with the tabs leaving. And
/// (9988d7c, review 1) memory pressure wrote the tabs going dormant on the session thread, behind every file queued, while nothing else was served. With every
/// sleep file's write held at its seam, as a disk that doesn't answer, tabs opened one after another past the warm ones and the queue's cap on tabs: never more
/// than one writer thread, those past the cap on tabs dropped and counted; memory pressure then, with
/// the writer stuck mid-write, and a tab shown, are both taken by the session thread at once; let through, the queue's written and there's no writer and no
/// thread for one at rest, nothing held. (Replaces the app test `long_session::tabs_leaving_in_bulk_with_slow_sleep_files_have_one_writer`, 107 s on CI, whose
/// footprint bound is the queue's cap held here and by `sleep_queue`'s own tests.)
#[test]
fn tabs_leaving_with_their_sleep_files_held_have_one_writer_and_a_capped_queue() {
    let tabs = recto_session::MAX_WARM + crate::sleep_queue::MAX_LEAVING_TABS + 6;
    let disk = Disk::new();
    let docs: Vec<_> = (0..tabs).map(|at| disk.pdf(&format!("t{at}.pdf"), 3)).collect();
    let app = disk.builder().hold(&[Seam::SleepWrite]).start();
    for (at, doc) in docs.iter().enumerate() {
        app.open(doc);
        app.settle();
        assert!(sleep_writers() <= 1, "tab {at} of {tabs}: {} sleep writers", sleep_writers());
    }
    app.wait_until("a sleep file held mid-write", || app.waiting(Seam::SleepWrite) > 0);
    let leaving = app.record_json()["sleep"]["leaving"].clone();
    assert!(leaving["dropped"].as_u64().unwrap() > 0, "enough left to pass the cap on tabs: {leaving}");
    assert!(
        leaving["peak_tabs"].as_u64().unwrap() <= leaving["cap_queued_tabs"].as_u64().unwrap(),
        "the tabs queued passed their cap: {leaving}"
    );
    assert_eq!(leaving["peak_writers"], 1, "{leaving}");
    assert!(leaving["queued"].as_u64().unwrap() >= 1, "still queued behind the one held: {leaving}");
    // The system asks for memory back, and a tab's shown, with the writer stuck: the session thread takes both, as the warm ones off screen are queued.
    let first = app.session_json()["tabs"][0]["id"].as_u64().expect("a tab");
    app.core().qa_session_verb("memory_pressure", "warning").expect("a QA verb").expect("pressure");
    app.recto(recto_agent::Level::Full, &["tap", &format!("#{first}")]).expect("tap");
    app.settle();
    let sleep = app.record_json()["sleep"].clone();
    assert!(sleep["gone_dormant"]["pressure"].as_u64().unwrap() >= 1, "{sleep}");
    assert_eq!(app.session_json()["active"], first);
    recto_platform::slow::gate::open(Seam::SleepWrite);
    app.wait_until("the queue written, and no writer left", || {
        app.record_json()["sleep"]["leaving"]["writer"] == false && sleep_writers() == 0
    });
    let rest = app.record_json()["sleep"]["leaving"].clone();
    assert_eq!(rest["writers_running"], 0, "{rest}");
    assert_eq!(rest["queued"], 0, "{rest}");
    assert_eq!(rest["in_flight"], false, "nothing's held for it at rest: {rest}");
    assert_eq!(rest["peak_writers"], 1, "{rest}");
    drop(app.quit());
}

/// Whether the tab on screen has a sleep file in the state folder.
fn active_has_a_sleep_file(app: &crate::testkit::headless::HeadlessApp) -> bool {
    let core = app.core();
    core.host
        .session
        .with_session(|session| session.tabs().active().and_then(|id| session.dormant().sleep_facts(id)))
        .is_some_and(|facts| core.saver.store.read_sleep(facts.sleep.file, &facts.path).is_some())
}

/// Failure (d65da39, review 1 of the slow mode): a flush right after opening tabs, as a quit or an update has it, skipped the tab on screen's sleep file whenever
/// another was being written, with no trace, and the next launch's first frame was bare paper. With every sleep file's write held at its seam, tabs opened,
/// and a flush given a moment: the state's written, the tab on screen's file isn't, and the skip is counted; let through, the next flush writes it. (Replaces the
/// app test `restore::a_flush_writes_the_tab_on_screens_sleep_file_or_says_it_skipped_it`, 7 s on CI, which could pass either way of its `or`.)
#[test]
fn a_flush_counts_the_tab_on_screens_sleep_file_it_skipped_and_writes_it_given_the_time() {
    let disk = Disk::new();
    let docs: Vec<_> = (0..8).map(|at| disk.pdf(&format!("t{at}.pdf"), 3)).collect();
    let app = disk.builder().hold(&[Seam::SleepWrite]).start();
    for doc in &docs {
        app.open(doc);
    }
    app.wait_until("a sleep file held mid-write", || app.waiting(Seam::SleepWrite) > 0);
    let report = app.core().saver.flush(false, std::time::Duration::from_millis(50)).expect("the state written");
    assert_eq!(report.tabs, 8, "the state first, whatever the sleep files are doing");
    assert!(!active_has_a_sleep_file(&app));
    let sleep = app.record_json()["sleep"].clone();
    assert_eq!(sleep["writes_skipped_busy"], 1, "the skip is counted: {sleep}");
    recto_platform::slow::gate::open(Seam::SleepWrite);
    app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    app.core().saver.flush(false, crate::testkit::headless::WAIT_LIMIT).expect("the state written");
    assert!(active_has_a_sleep_file(&app), "given the time, it's written: {}", app.record_json()["sleep"]);
    drop(app.quit());
}

/// The view the host would have, at the top of the document, the page at its fit: a small window at one pixel a point, so each picture is a tenth of a
/// Retina window's and a test's every show and draw is quick under CI's limits (the file's tests took about 60 s of test time under `--like-ci` in a
/// 1168 by 900 Retina window, 2026-11-14). What's decided doesn't depend on a picture's size, except where a test says so ([`retina_view`]).
pub(super) fn window_view() -> recto_reader::Viewport {
    recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 292.0,
        height: 225.0,
        frame_width: 292.0,
        magnification: 1.0,
        backing: 1.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

/// A Retina window, 1168 by 900 points at two pixels a point, as the host has: for a test that needs a picture of a synthetic page to pack to a few KB.
fn retina_view() -> recto_reader::Viewport {
    recto_reader::Viewport {
        width: 1168.0,
        frame_width: 1168.0,
        height: 900.0,
        backing: 2.0,
        ..window_view()
    }
}

// The host's UI thread, as the kit stands in for it.
use crate::testkit::headless::host_turns;

fn tab_named(app: &crate::testkit::headless::HeadlessApp, name: &str) -> u64 {
    app.session_json()["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["path"].as_str().is_some_and(|path| path.ends_with(name)))
        .and_then(|tab| tab["id"].as_u64())
        .expect("the tab")
}

fn full(app: &crate::testkit::headless::HeadlessApp, words: &[&str]) -> serde_json::Value {
    app.recto(recto_agent::Level::Full, words)
        .unwrap_or_else(|error| panic!("{words:?}: {error:?}"))
}

/// Failure (review 1 of drawing a tab's place before it sleeps, F9, kept for the sizes): a warm tab off screen whose pages are deleted keeps the sizes it
/// was laid out with, filed by where they were, so its sleep file would lay another page out at a deleted one's size. Tab a, left drawn, has its first page
/// deleted while it's off screen and is put to sleep; its sleep file, read back, has no size of its pages from before. Tab c, the same with no edit, has
/// its sizes, and wakes from its facts, so the file isn't empty for want of one.
#[test]
fn a_tab_whose_pages_change_off_screen_sleeps_with_no_size_of_another_page() {
    let disk = Disk::new();
    let (a, b, c) = (disk.pdf("a.pdf", 6), disk.pdf("b.pdf", 3), disk.pdf("c.pdf", 6));
    let app = disk.start();
    let engine = app.core().engine.clone();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for doc in [&c, &a, &b] {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    let (a, c) = (tab_named(&app, "a.pdf"), tab_named(&app, "c.pdf"));
    full(&app, &["page", "delete", "1", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    for tab in [a, c] {
        full(&app, &["session", "sleep", "--tab", &format!("#{tab}")]);
    }
    host_turns(&app, &mut view, true);
    let read_back = |tab: u64| {
        let path = sleep_files_on_disk_paths(&app)
            .into_iter()
            .find(|path| path.file_name().is_some_and(|name| name.to_string_lossy() == sleep_file_of(&app, tab)))
            .unwrap_or_else(|| panic!("tab {tab}'s sleep file: {}", app.record_json()["sleep"]));
        recto_session::sleep::decode_sleep(&std::fs::read(path).expect("read"), recto_session::record::CURRENT_VERSION).expect("a sleep file that reads")
    };
    let (a_file, c_file) = (read_back(a), read_back(c));
    assert!(c_file.usual.is_some(), "unedited, its sizes are kept: {c_file:?}");
    assert!(
        a_file.usual.is_none() && a_file.sizes.is_empty(),
        "sizes from before its pages changed: {a_file:?}"
    );
    full(&app, &["tap", &format!("#{c}")]);
    host_turns(&app, &mut view, true);
    let c_woke = engine.report().0["last_wake"].clone();
    assert_eq!(c_woke["had"], "facts", "unedited, it wakes from its sleep file: {c_woke}");
    drop(app.quit());
}

/// Failure, the same rule from the other side: a tab already asleep, with a sleep file on disk of its pages as they were, has its first page deleted while it's off screen (an agent's edit), and is shown: its first frame comes from the old file, the deleted page's size where page 2 now is. The wake takes nothing from a sleep file of another list of pages than the session's now.
#[test]
fn a_tab_asleep_whose_pages_change_wakes_with_nothing_from_before() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 6), disk.pdf("b.pdf", 3));
    let app = disk.start();
    let engine = app.core().engine.clone();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for doc in [&a, &b] {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    let a = tab_named(&app, "a.pdf");
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["page", "delete", "1", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    let woke = engine.report().0["last_wake"].clone();
    assert_eq!(woke["tab"], a, "{woke}");
    assert_ne!(woke["had"], "facts", "the facts of its pages from before they changed: {woke}");
    drop(app.quit());
}

/// Whether a wake's first frame came with its pages' sizes from its facts: a sleep file read from disk, or what the tab left with, handed back inside its
/// cool-down.
fn from_facts(wake: &serde_json::Value) -> bool {
    wake["had"] == "facts"
}

/// Shows `tab` from asleep with drawing held, as the first frame has it, and says how that wake went, with the pages on their paper then (`paper_up`); then
/// lets it draw and settle.
fn woken_at_once(app: &crate::testkit::headless::HeadlessApp, tab: u64, view: &mut recto_reader::Viewport) -> serde_json::Value {
    let engine = &app.core().engine;
    engine.hold_drawing(true);
    full(app, &["tap", &format!("#{tab}")]);
    host_turns(app, view, false);
    let report = engine.report().0;
    let mut wake = report["last_wake"].clone();
    wake["paper_up"] = report["tiles"]["page_layers"].clone();
    engine.hold_drawing(false);
    host_turns(app, view, true);
    wake
}

/// Failure (a whole-suite run of 2026-11-13, `a_tab_woken_again_after_the_look_changed_shows_a_picture_at_once` once in one run under load: its second sleep
/// neither written nor put off): a tab woken from its sleep file stays "from a sleep file, unchecked" until its worker says the file's length, and such a tab
/// has no sleep file written. Its worker had outlived the tab's dormancy, as the page view's thread hadn't come round between the tab going and coming back,
/// and a worker says the length once in its life, so it never said it again: the tab kept its old file for good, whose pictures are of where it was.
/// Forced with the page view's thread held in a job for b (`Seam::Worker`) while a is put to sleep and woken: once drawn, a's next sleep is decided.
#[test]
fn a_tab_woken_while_its_worker_outlived_its_sleep_has_its_next_sleep_file_decided() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 12));
    let app = disk.start();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for doc in [&a, &b] {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    let (a, b) = (tab_named(&app, "a.pdf"), tab_named(&app, "b.pdf"));
    let decided = || {
        let sleep = app.record_json()["sleep"].clone();
        sleep["writes_put_off"].as_u64().expect("put off") + sleep["sleep_files_written"].as_u64().expect("written")
    };
    // a's file written, a woken and drawn: its worker's open and has said the length.
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    // The page view's thread held in a job for b, so it doesn't come round while a goes and comes back.
    recto_platform::slow::gate::hold(&[Seam::Worker]);
    full(&app, &["goto", "7"]);
    host_turns(&app, &mut view, false);
    app.wait_until("a job for b held", || app.waiting(Seam::Worker) > 0);
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, false);
    full(&app, &["tap", &format!("#{a}")]);
    host_turns(&app, &mut view, false);
    recto_platform::slow::gate::open(Seam::Worker);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    let before = decided();
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    assert_eq!(
        decided(),
        before + 1,
        "a's sleep reached no decision: its file's length was never checked: {}",
        app.record_json()["sleep"]
    );
    drop(app.quit());
}

/// Failure (the sleep branch's second review, 2026-11-13): the length was asked for again only when a page view let go of a tab's view, and a tab going dormant
/// and being woken can fall into one look of the session thread's, so the view was never let go of: the session thread puts a tab left alone to sleep and
/// queues its look behind whatever's queued already (a tap of the person's), and an agent's tap on the socket runs while the session thread is busy. Then
/// no ask, the worker that had said the length kept quiet, and the woken tab stayed unchecked for good. Forced on the app's own core with the session thread
/// held: a goes dormant, its sleep file written, and is tapped awake, all before one look; let go, a is shown with its view kept; its next sleep is decided.
#[test]
fn a_tab_put_to_sleep_and_woken_in_one_look_has_its_next_sleep_file_decided() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 3));
    let app = disk.start();
    let engine = app.core().engine.clone();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for doc in [&a, &b] {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    let (a, b) = (tab_named(&app, "a.pdf"), tab_named(&app, "b.pdf"));
    let decided = || {
        let sleep = app.record_json()["sleep"].clone();
        sleep["writes_put_off"].as_u64().expect("put off") + sleep["sleep_files_written"].as_u64().expect("written")
    };
    let held = app.hold_session_thread();
    let id = recto_session::TabId(u32::try_from(a).expect("a tab id"));
    let core = app.core();
    let going_dormant = crate::core::sleeper::GoingDormant::new(
        crate::core::sleeper::OnHand::new(&core.host.session, &core.panes, &core.engine),
        &core.on_disk,
        &core.saver,
        &core.core.frozen,
    );
    assert_eq!(
        core.on_disk.sleeper().go_dormant(&going_dormant, &[id], super::super::sleeper::Why::LeftAlone),
        1,
        "a goes dormant"
    );
    let name = sleep_file_of(&app, a);
    app.wait_until("a's sleep file written", || sleep_files_on_disk(&app).contains(&name));
    full(&app, &["tap", &format!("#{a}")]);
    assert!(engine.waking().holds(id.0), "a's view is still held: the look that would let it go hasn't come");
    drop(held);
    host_turns(&app, &mut view, true);
    assert_eq!(app.record_json()["sleep"]["woken_from_file"], 1, "{}", app.record_json()["sleep"]);
    full(&app, &["tap", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    let before = decided();
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    assert_eq!(
        decided(),
        before + 1,
        "a's sleep reached no decision: its file's length was never asked for again: {}",
        app.record_json()["sleep"]
    );
    drop(app.quit());
}

/// Failure (the sleep branch's second review, 2026-11-13): a worker says its file's length once, and it marked it said on a job for any tab, telling only the
/// tab whose own file it is. So a job on `a.pdf` for b, a document with a's pages added, used up the saying (on a first open as much as after a re-ask), and
/// a, woken from its sleep file, stayed unchecked for good: never given a new sleep file. On the app's own core: a put to sleep, b sent to one of a's pages
/// (b's job on `a.pdf` comes first), a woken and drawn; its next sleep is decided.
#[test]
fn another_tabs_job_on_a_file_leaves_its_length_to_be_said_to_the_tab_that_owns_it() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 2));
    let app = disk.start();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for doc in [&a, &b] {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    full(&app, &["page", "insert", a.to_str().expect("a path"), "--at", "3"]);
    host_turns(&app, &mut view, true);
    let (a, b) = (tab_named(&app, "a.pdf"), tab_named(&app, "b.pdf"));
    let decided = || {
        let sleep = app.record_json()["sleep"].clone();
        sleep["writes_put_off"].as_u64().expect("put off") + sleep["sleep_files_written"].as_u64().expect("written")
    };
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    // b at a page of a's: its job on a.pdf is the first since a went.
    full(&app, &["goto", "10", "--tab", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    let before = decided();
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    assert_eq!(
        decided(),
        before + 1,
        "a's sleep reached no decision: its file's length went to b's job and was never said to a: {}",
        app.record_json()["sleep"]
    );
    drop(app.quit());
}

/// Failure (the cool-down's review, 2026-11-13, S2): a tab's file written at page 5, then page 5 rotated and the tab left inside the minute's gap: the write
/// was put off, and the wake refused that file for its other list of pages and read the PDF. On the app's own core, the window up: a slept at page 5, woken,
/// page 5 rotated, slept again inside the gap: written, for the list, and woken from its facts, its pages on their paper with drawing held.
#[test]
fn a_tab_left_inside_the_gap_after_a_page_was_rotated_wakes_from_its_facts() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 3));
    let app = disk.start();
    let engine = app.core().engine.clone();
    engine.waking().window_up();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for doc in [&a, &b] {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    let (a, b) = (tab_named(&app, "a.pdf"), tab_named(&app, "b.pdf"));
    full(&app, &["tap", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["goto", "5"]);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["page", "rotate", "5", "--by", "90", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["tap", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    let before = app.record_json()["sleep"].clone();
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    let after = app.record_json()["sleep"].clone();
    assert_eq!(
        after["writes_for_list"].as_u64().unwrap(),
        before["writes_for_list"].as_u64().unwrap() + 1,
        "put off on a file of the list before the rotation: {after}"
    );
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert_eq!(wake["had"], "facts", "its file of the list before: {wake}");
    assert!(wake["paper_up"].as_u64().unwrap() >= 1, "no page on its paper with nothing drawn: {wake}");
    drop(app.quit());
}

/// The person's own command, as the window sends it for them: through the one executor as theirs, never an agent's.
fn person(app: &crate::testkit::headless::HeadlessApp, words: &[&str]) {
    let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
    app.person(&recto_agent::table::parse_cli(&words).expect("a command").wire());
    app.settle();
}

/// Failure (the cool-down's review, 2026-11-13, S3): an agent looping a move and `session sleep` used the whole app's hour of sleep files, so every wake of the
/// person's after was bare for the hour. With agents' share set at one and a half of the agent's first file: that file is written, the next (past the gap,
/// which would otherwise be written) doesn't fit and is put off, and the agent is told why and what to do from that write's own outcome; then the person's own
/// route, the tab pushed out of the warm ones by the person's own opens, is still written. (The second review: the person's part was driven by an agent at
/// Full, which now counts as the agent's; it goes through the person's own commands.)
#[test]
fn an_agents_sleeps_stop_at_its_share_and_the_persons_push_out_is_still_written() {
    let disk = Disk::new();
    let docs: Vec<_> = ["doc", "other", "c", "d", "e"]
        .iter()
        .map(|name| disk.pdf(&format!("{name}.pdf"), if *name == "doc" { 12 } else { 2 }))
        .collect();
    let app = disk.start();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for file in &docs[..2] {
        app.open(file);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    let (doc, other) = (tab_named(&app, "doc.pdf"), tab_named(&app, "other.pdf"));
    let mut turn = |page: &str| {
        full(&app, &["tap", &format!("#{doc}")]);
        host_turns(&app, &mut view, true);
        full(&app, &["goto", page]);
        host_turns(&app, &mut view, true);
        full(&app, &["tap", &format!("#{other}")]);
        host_turns(&app, &mut view, true);
        let answer = full(&app, &["session", "sleep", "--tab", &format!("#{doc}")]);
        host_turns(&app, &mut view, true);
        answer
    };
    let first = turn("3");
    assert!(first.get("note").is_none(), "{first}");
    let sleep = app.record_json()["sleep"].clone();
    assert_eq!(sleep["sleep_files_written"], 1, "{sleep}");
    let file = sleep["agent_bytes_this_hour"].as_u64().expect("agents' bytes");
    assert_eq!(sleep["sleep_bytes_written"], file, "the agent's file is counted as the agent's: {sleep}");
    app.core().on_disk.sleeper().set_agent_hour_cap(file + file / 2);
    app.advance(u64::from(crate::sleeper::SLEEP_WRITE_EVERY_SECONDS) * 1000 + 1);
    let second = turn("5");
    let note = second["note"]
        .as_str()
        .unwrap_or_else(|| panic!("the agent isn't told its share is used: {second}"));
    assert!(note.contains("agents' share") && note.contains("wasn't written"), "{note}");
    let sleep = app.record_json()["sleep"].clone();
    assert_eq!(sleep["sleep_files_written"], 1, "{sleep}");
    assert_eq!(sleep["agent_share_used"], 1, "{sleep}");
    assert_eq!(sleep["agent_bytes_this_hour"], file, "nothing past the share: {sleep}");
    // The person reads on in doc and leaves it, and opens three more: doc is pushed out of the warm ones by the person, and its file is written.
    person(&app, &["tap", &format!("#{doc}")]);
    host_turns(&app, &mut view, true);
    person(&app, &["goto", "7"]);
    host_turns(&app, &mut view, true);
    for file in &docs[2..] {
        app.open(file);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    let after = app.record_json()["sleep"].clone();
    assert!(
        after["sleep_files_written"].as_u64().unwrap() > 1,
        "the person's push-out wasn't written past the agents' share: {after}"
    );
    assert_eq!(after["agent_bytes_this_hour"], file, "the person's push-out counted as the agent's: {after}");
    drop(app.quit());
}

/// Failure (the sleep branch's second review, 2026-11-13): an agent's share counted a write of a tab's facts, with no file there, as one put off, so the agent
/// was told its sleep file wasn't written when it was; and an agent's `session save` that its share kept from writing the tab on screen's file said nothing,
/// though a crash after it leaves the next launch's first frame without it. With agents' share set small: an agent's sleep of a tab with no file writes its
/// facts and says nothing; then, the share used, an agent's save writes the record, not the tab on screen's file, and says that.
#[test]
fn an_agents_answer_says_what_its_share_did_to_its_own_sleep_file() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 6), disk.pdf("b.pdf", 6));
    let app = disk.start();
    let mut view = crate::testkit::host_standin::first_shown(retina_view());
    for doc in [&a, &b] {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, retina_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    // Room for a tab's facts.
    app.core().on_disk.sleeper().set_agent_hour_cap(1_000);
    let a = tab_named(&app, "a.pdf");
    let slept = full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    assert!(slept.get("note").is_none(), "a's facts were written, and nothing cut them short: {slept}");
    let sleep = app.record_json()["sleep"].clone();
    assert_eq!(sleep["sleep_files_written"], 1, "its facts: {sleep}");
    let used = sleep["agent_bytes_this_hour"].as_u64().expect("agents' bytes");
    assert!(used > 0 && used <= 1_000, "{sleep}");
    app.core().on_disk.sleeper().set_agent_hour_cap(used);
    let saved = full(&app, &["session", "save"]);
    assert_eq!(saved["saved"], true, "{saved}");
    let note = saved["note"]
        .as_str()
        .unwrap_or_else(|| panic!("the save says nothing of the tab on screen's file: {saved}"));
    assert!(note.contains("tab on screen") && note.contains("wasn't written"), "{note}");
    assert!(!active_has_a_sleep_file(&app), "{}", app.record_json()["sleep"]);
    drop(app.quit());
}

/// Failure (the sleep branch's second review, 2026-11-13): tabs an agent's `tap` or `goto` pushed out of the warm ones were written as the person's, so an
/// agent cycling through tabs, with a move before each push-out to pass the place rule, could use the whole hour of sleep files in a minute or two, and
/// every wake of the person's after was bare. One tab more than the warm ones, so every tap pushes one out, each tab drawn as it's opened, the gap passed on
/// the fake clock before each step, and agents' share set at one and a half files: the agent's tap and goto to the next two tabs (two push-outs, of tabs
/// with no file yet) writes the first and is cut short at the second, never past its share; and the person's own taps to the other two after (two push-outs)
/// are still written, none of it counted as the agent's. Kept cheap for CI, where each step is a show and a draw (10.7 s under `--like-ci` with six tabs
/// of six pages, a Retina window and two whole cycles): three pages a tab, a small window, and only the steps the rule needs.
#[test]
fn an_agent_cycling_through_tabs_stops_at_its_share_and_the_persons_cycling_is_still_written() {
    const TABS: usize = recto_session::MAX_WARM + 1;
    let disk = Disk::new();
    let docs: Vec<_> = (0..TABS).map(|at| disk.pdf(&format!("t{at}.pdf"), 3)).collect();
    let app = disk.start();
    // Nothing goes dormant by itself here: only tabs pushed out of the warm ones are written.
    app.core().on_disk.sleeper().set_after(24 * 60 * 60);
    let small = || recto_reader::Viewport {
        width: 160.0,
        height: 120.0,
        frame_width: 160.0,
        backing: 1.0,
        ..window_view()
    };
    let mut view = crate::testkit::host_standin::first_shown(small());
    for doc in &docs {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, small());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    let sleep = app.record_json()["sleep"].clone();
    let (files, bytes) = (sleep["sleep_files_written"].as_u64().unwrap(), sleep["sleep_bytes_written"].as_u64().unwrap());
    assert_eq!(files, 1, "the person's last open pushed the first out: {sleep}");
    assert_eq!(sleep["agent_bytes_this_hour"], 0, "{sleep}");
    let share = bytes * 3 / 2;
    app.core().on_disk.sleeper().set_agent_hour_cap(share);
    let tabs: Vec<u64> = (0..TABS).map(|at| tab_named(&app, &format!("t{at}.pdf"))).collect();
    let gap = u64::from(crate::sleeper::SLEEP_WRITE_EVERY_SECONDS) * 1000 + 1;
    // A tap to each tab in turn, from the first, pushes out the one after it; the first, pushed out last, is left at page 2, so its file isn't the
    // same bytes as the one its open wrote (which the store doesn't write again).
    let mut step = |tab: u64, by_agent: bool| {
        app.advance(gap);
        if by_agent {
            full(&app, &["tap", &format!("#{tab}")]);
            host_turns(&app, &mut view, true);
            full(&app, &["goto", "2"]);
        } else {
            person(&app, &["tap", &format!("#{tab}")]);
            host_turns(&app, &mut view, true);
            person(&app, &["goto", "2"]);
        }
        host_turns(&app, &mut view, true);
        app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    };
    step(tabs[0], true);
    step(tabs[1], true);
    let agents = app.record_json()["sleep"].clone();
    assert!(
        agents["agent_share_used"].as_u64().unwrap() >= 1,
        "the agent's taps never met its share: {agents}"
    );
    let agent_bytes = agents["agent_bytes_this_hour"].as_u64().unwrap();
    assert!(agent_bytes > 0, "nothing of the agent's was written up to its share: {agents}");
    assert!(agent_bytes <= share, "past the share, {agent_bytes} of {share}: {agents}");
    let written = agents["sleep_files_written"].as_u64().unwrap();
    step(tabs[2], false);
    step(tabs[3], false);
    let persons = app.record_json()["sleep"].clone();
    // The first of the person's pushes out a tab with no file, which is written; the second pushes out the first, whose facts are as its file has them,
    // so nothing new is written for it (the store compares the bytes): what's held is that neither was cut short as the agent's.
    assert!(
        persons["sleep_files_written"].as_u64().unwrap() > written,
        "the person's own taps, after the agent's share was used, weren't written: {persons}"
    );
    assert_eq!(
        persons["agent_share_used"], agents["agent_share_used"],
        "the person's cut short as the agent's: {persons}"
    );
    assert_eq!(persons["agent_bytes_this_hour"], agent_bytes, "the person's counted as the agent's: {persons}");
    drop(app.quit());
}

/// Failure (the cool-down's review, 2026-11-13, S3): an agent's `session save` flushed as a quit does, draining the cool-down, so an agent could cut every
/// pushed-out tab's wait short on demand. With a tab waiting its cool-down: an agent's save writes the record and leaves it waiting; a quit's flush still
/// writes it.
#[test]
fn an_agents_save_leaves_the_cool_down_alone_and_a_quit_still_writes_it() {
    let (app, _a, _view) = a_pushed_out_and_waiting(Disk::new(), crate::sleeper::LEAVING_WAIT_MS);
    let saved = full(&app, &["session", "save"]);
    assert_eq!(saved["saved"], true, "{saved}");
    assert_eq!(app.record_json()["sleep"]["leaving"]["queued"], 1, "{}", app.record_json()["sleep"]);
    app.core().saver.flush(false, crate::testkit::headless::WAIT_LIMIT).expect("the state written");
    assert_eq!(app.record_json()["sleep"]["leaving"]["queued"], 0, "{}", app.record_json()["sleep"]);
    drop(app.quit());
}

/// Failure (the cool-down's review, 2026-11-13, S4): after a sleep writer panicked, what it left queued had no thread; a quit's drain waited for it to its
/// limit and nothing started one, so the queued files were never written and the tab on screen's waited behind them. A writer that panics as it starts, a tab
/// left waiting in its cool-down, then a quit's flush: the drain starts a writer, everything queued is written, the tab on screen's file is there, and the flush
/// takes nothing like its limit.
#[test]
fn a_quit_after_a_sleep_writer_panicked_writes_what_waits_and_the_tab_on_screen() {
    let disk = Disk::new();
    let docs: Vec<_> = ["a", "b", "c", "d"].iter().map(|name| disk.pdf(&format!("{name}.pdf"), 3)).collect();
    let app = disk.builder().leaving_wait(crate::sleeper::LEAVING_WAIT_MS).start();
    app.core().on_disk.sleeper().panic_next_leaving_writer();
    for doc in &docs {
        app.open(doc);
        app.settle();
    }
    app.wait_until("the writer gone, panicked", || {
        let leaving = app.record_json()["sleep"]["leaving"].clone();
        leaving["writer_panics"] == 1 && leaving["writer"] == false
    });
    assert_eq!(app.record_json()["sleep"]["leaving"]["queued"], 1, "{}", app.record_json()["sleep"]);
    let limit = std::time::Duration::from_secs(20);
    let began = std::time::Instant::now();
    app.core().saver.flush(false, limit).expect("the state written");
    let took = began.elapsed();
    let leaving = app.record_json()["sleep"]["leaving"].clone();
    assert_eq!(leaving["queued"], 0, "what waited was never written: {leaving}");
    assert!(active_has_a_sleep_file(&app), "{}", app.record_json()["sleep"]);
    assert!(took < limit / 2, "the quit waited out its limit for a writer there wasn't: {took:?}");
    drop(app.quit());
}

/// Failure (the cool-down's review, 2026-11-13, S7): a quit drained the cool-down before it wrote the tab on screen's file, so on a slow disk the queued files
/// used its limit and the one file the next launch's first frame needs was skipped. With every sleep file's write held at its seam (a disk that answers when
/// the test says), three tabs waiting their cool-down, and a quit's flush with a short limit: the first write to arrive is let through, and the limit then
/// cuts the drain; the tab on screen's file is there, as it was written first.
#[test]
fn a_quit_writes_the_tab_on_screens_sleep_file_before_what_waits() {
    let disk = Disk::new();
    let docs: Vec<_> = (0..6).map(|at| disk.pdf(&format!("t{at}.pdf"), 3)).collect();
    let app = disk.builder().leaving_wait(crate::sleeper::LEAVING_WAIT_MS).hold(&[Seam::SleepWrite]).start();
    for doc in &docs {
        app.open(doc);
        app.settle();
    }
    assert_eq!(app.record_json()["sleep"]["leaving"]["queued"], 3, "{}", app.record_json()["sleep"]);
    assert_eq!(app.waiting(Seam::SleepWrite), 0, "nothing written while they wait");
    std::thread::scope(|scope| {
        let flushed = std::thread::Builder::new()
            .name("test-quit-flush".into())
            .spawn_scoped(scope, || app.core().saver.flush(false, std::time::Duration::from_millis(1_500)))
            .expect("a thread for the quit's flush");
        app.wait_until("the first sleep file of the quit held", || app.waiting(Seam::SleepWrite) > 0);
        recto_platform::slow::gate::let_through(Seam::SleepWrite, 1);
        flushed.join().expect("the flush returns").expect("the state written");
    });
    assert!(
        active_has_a_sleep_file(&app),
        "the tab on screen's file was skipped behind what waited: {}",
        app.record_json()["sleep"]
    );
    recto_platform::slow::gate::open(Seam::SleepWrite);
    drop(app.quit());
}

/// Failure (the cool-down's review, 2026-11-13: the commonest close now, untested): a tab closed while its sleep file waits in its cool-down had its file
/// written at the wait, after the close, or its facts held for nothing till then. Closed inside the wait: nothing of it is held, and past the wait no file of
/// it is on disk and nothing was handed back.
#[test]
fn a_tab_closed_while_it_waits_in_its_cool_down_leaves_nothing() {
    let wait = crate::sleeper::LEAVING_WAIT_MS;
    let (app, a, mut view) = a_pushed_out_and_waiting(Disk::new(), wait);
    let name = sleep_file_of(&app, a);
    full(&app, &["close", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    // The close's retirement, carried out by the saver's next write.
    app.core().saver.flush(false, crate::testkit::headless::WAIT_LIMIT).expect("the state written");
    let leaving = app.record_json()["sleep"]["leaving"].clone();
    assert_eq!(leaving["queued"], 0, "{leaving}");
    assert_eq!(leaving["in_flight"], false, "held for a closed tab: {leaving}");
    assert_eq!(leaving["retired"], 1, "{leaving}");
    app.advance(wait + 1);
    app.wait_until("the writer gone", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    assert!(!sleep_files_on_disk(&app).contains(&name), "a closed tab's sleep file was written at its wait");
    assert_eq!(app.record_json()["sleep"]["leaving"]["handed_back"], 0);
    drop(app.quit());
}

/// Failure (the cool-down's review, 2026-11-13): going to the background and memory pressure are meant to hurry what waits, as no one may come back for it,
/// and nothing tested either: deleting the hurry from either left the files waiting their whole cool-down. With a tab waiting and the clock standing still:
/// going to the background has its file written; and, for a second app, memory pressure does.
#[test]
fn going_to_the_background_and_memory_pressure_write_what_waits_now() {
    let wait = crate::sleeper::LEAVING_WAIT_MS;
    let (app, a, _view) = a_pushed_out_and_waiting(Disk::new(), wait);
    app.core().window.frontmost_changed(true, true, &app.core().saver, &app.core().core);
    app.core().window.frontmost_changed(false, false, &app.core().saver, &app.core().core);
    app.wait_until("written on going to the background, the clock standing still", || {
        sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a))
    });
    drop(app.quit());
    let (app, a, _view) = a_pushed_out_and_waiting(Disk::new(), wait);
    app.core().qa_session_verb("memory_pressure", "warning").expect("a QA verb").expect("pressure");
    app.wait_until("written on memory pressure, the clock standing still", || {
        sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a))
    });
    drop(app.quit());
}

/// Failure (the sleep branch's second review, 2026-11-13): the UI thread's hurry on going to the background started no writer, so after a sleep writer
/// panicked what waited in its cool-down had no thread and stayed unwritten till something else started one. A writer that panics as it starts, a tab
/// waiting its cool-down, then the app goes to the background with the clock standing still: what waits is written.
#[test]
fn going_to_the_background_after_a_sleep_writer_panicked_writes_what_waits() {
    let disk = Disk::new();
    let docs: Vec<_> = ["a", "b", "c", "d"].iter().map(|name| disk.pdf(&format!("{name}.pdf"), 3)).collect();
    let app = disk.builder().leaving_wait(crate::sleeper::LEAVING_WAIT_MS).start();
    app.core().on_disk.sleeper().panic_next_leaving_writer();
    for doc in &docs {
        app.open(doc);
        app.settle();
    }
    app.wait_until("the writer gone, panicked", || {
        let leaving = app.record_json()["sleep"]["leaving"].clone();
        leaving["writer_panics"] == 1 && leaving["writer"] == false
    });
    let a = tab_named(&app, "a.pdf");
    assert!(!sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a)), "written before its wait");
    app.core().window.frontmost_changed(true, true, &app.core().saver, &app.core().core);
    app.core().window.frontmost_changed(false, false, &app.core().saver, &app.core().core);
    app.wait_until("written on going to the background, with no writer left to take a word", || {
        sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a))
    });
    drop(app.quit());
}

/// Failure (the sleep branch's second review, 2026-11-13, with the record's-lock retry merged): the queue's writer and asked sleeps aren't behind the saver's
/// gate, and the store's own refusal goes by its lock alone, which a copy can hold while it doesn't keep the record (a late keep's read past its patience
/// keeps the lock until its thread is out of the read). So a tab pushed out then, and a sleep asked for, wrote sleep files into a record this copy was letting
/// go of. A late keep's read held past its patience, the person's opens that waited run and push a tab out of the warm ones, and a sleep is asked for the
/// tab on screen: the record's folder is byte for byte as it was.
#[test]
fn a_copy_letting_the_record_go_writes_no_sleep_file() {
    use super::late_lock::{a_saved_session, record_folder, the_copy_before};
    use crate::lifecycle::keeping::{RESTORING_PATIENCE_MS, TRY_EVERY_MS};
    let disk = a_saved_session();
    let before = the_copy_before(&disk);
    let on_disk = record_folder(&disk);
    let app = disk.builder().another_keeps().start();
    recto_platform::slow::gate::hold(&[Seam::State]);
    drop(before);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the saved session's read held", || app.waiting(Seam::State) > 0);
    assert!(app.core().life.keeping().restoring());
    for name in ["c", "d", "e", "f"] {
        let doc = app.disk().pdf(&format!("{name}.pdf"), 3);
        let _ = app.act(crate::core::controls::UiAction::Open {
            path: doc.display().to_string(),
        });
    }
    app.advance(RESTORING_PATIENCE_MS);
    app.wait_until("the restore over", || app.core().life.keeping().not_kept());
    app.settle();
    assert!(
        app.core().saver.store.holds_the_record(),
        "the lock stays with the read while it's inside: the moment tested"
    );
    let pushed = app.record_json()["sleep"]["leaving"]["writers_started"].as_u64().unwrap();
    assert!(
        pushed >= 1,
        "a tab pushed out of the warm ones went to the queue: {}",
        app.record_json()["sleep"]
    );
    app.wait_until("the queue's writer done", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    assert_eq!(
        record_folder(app.disk()),
        on_disk,
        "a pushed-out tab's sleep file written into a record this copy doesn't keep"
    );
    let on_screen = app
        .core()
        .host
        .session
        .with_session(|session| session.tabs().active())
        .expect("a tab on screen");
    assert_eq!(
        app.core().on_disk.sleeper().write_sleep_for(
            &crate::core::sleeper::OnHand::new(&app.core().host.session, &app.core().panes, &app.core().engine),
            &app.core().saver.store,
            &app.core().on_disk.keeper().asks(),
            on_screen,
            crate::sleep_writer::By::Asked(recto_agent::Actor::Person),
            None
        ),
        None
    );
    assert_eq!(record_folder(app.disk()), on_disk, "a sleep file written into a record this copy doesn't keep");
    app.let_through(Seam::State, 1);
    app.wait_until("the lock let go", || !app.core().saver.store.holds_the_record());
    drop(app.quit());
}

/// The sleep files in the state folder, wherever under it they are.
fn sleep_files_on_disk_paths(app: &crate::testkit::headless::HeadlessApp) -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("sleep-")) {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(&app.disk().paths().root, &mut found);
    found
}

/// The sleep files in the state folder, by name, wherever under it they are.
fn sleep_files_on_disk(app: &crate::testkit::headless::HeadlessApp) -> Vec<String> {
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if let Some(name) = path.file_name().and_then(|name| name.to_str()).filter(|name| name.starts_with("sleep-")) {
                found.push(name.to_string());
            }
        }
    }
    let mut found = Vec::new();
    walk(&app.disk().paths().root, &mut found);
    found
}

/// Failure (the sleeper's review, 2026-11-13): a tab pushed out of the warm ones has its facts copied into the app's queue for the one writer, and close
/// retired its sleep file in the session only: the queue still held it, so its file was written after the close's retirement had been carried out, and the
/// contents of a closed document stayed in the app's folder. With every sleep file's write held at its seam: five tabs opened, so x is pushed
/// out and held mid-write and a is queued behind it; both closed, the close's retirement written; let through, no sleep file is left of either, and none of
/// the three still open (never pushed out) has one either.
#[test]
fn a_tab_closed_while_its_sleep_file_is_queued_or_being_written_leaves_none() {
    let disk = Disk::new();
    let docs: Vec<_> = ["x", "a", "b", "c", "d"].iter().map(|name| disk.pdf(&format!("{name}.pdf"), 3)).collect();
    let app = disk.builder().hold(&[Seam::SleepWrite]).start();
    for doc in &docs {
        app.open(doc);
        app.settle();
    }
    app.wait_until("x's sleep file held mid-write", || app.waiting(Seam::SleepWrite) > 0);
    let leaving = app.record_json()["sleep"]["leaving"].clone();
    assert_eq!(leaving["queued"], 1, "a queued behind x: {leaving}");
    for name in ["x.pdf", "a.pdf"] {
        full(&app, &["close", "--tab", &format!("#{}", tab_named(&app, name))]);
    }
    app.settle();
    // The close's retirement carried out, while x's write is still held: a short limit, as the tab on screen's file would wait behind it.
    app.core().saver.flush(false, std::time::Duration::from_millis(50)).expect("the state written");
    recto_platform::slow::gate::open(Seam::SleepWrite);
    app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    app.core().saver.flush(false, crate::testkit::headless::WAIT_LIMIT).expect("the state written");
    // The tab on screen's own, written by the flush, is the one that's there.
    let core = app.core();
    let on_screen = core
        .host
        .session
        .with_session(|session| session.tabs().active().and_then(|id| session.dormant().sleep_facts(id)))
        .map(|facts| recto_session::record::sleep_file_name(facts.sleep.file, &facts.path))
        .expect("the tab on screen's sleep file name");
    let left: Vec<String> = sleep_files_on_disk(&app).into_iter().filter(|name| *name != on_screen).collect();
    assert!(left.is_empty(), "a closed tab's sleep file outlived it: {left:?}");
    drop(app.quit());
}

/// Failure (the sleeper's review, 2026-11-13): a tab pushed out of the warm ones and shown again before the one writer got to it still had its sleep file
/// written from what it was copied with as it left: a write for nothing, against the owner's rule on the drive's wear, as the page view holds it again and it's
/// queued afresh when it next leaves. With every sleep file's write held at its seam: five tabs opened, so x is held mid-write and a queued behind it; a shown
/// again; let through, x's file is written and a's isn't.
#[test]
fn a_tab_shown_again_before_its_queued_sleep_file_is_written_has_none_written() {
    let disk = Disk::new();
    let docs: Vec<_> = ["x", "a", "b", "c", "d"].iter().map(|name| disk.pdf(&format!("{name}.pdf"), 3)).collect();
    let app = disk.builder().hold(&[Seam::SleepWrite]).start();
    for doc in &docs {
        app.open(doc);
        app.settle();
    }
    app.wait_until("x's sleep file held mid-write", || app.waiting(Seam::SleepWrite) > 0);
    let a = tab_named(&app, "a.pdf");
    full(&app, &["tap", &format!("#{a}")]);
    app.settle();
    recto_platform::slow::gate::open(Seam::SleepWrite);
    app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    let name_of = |id: u64| {
        app.core()
            .host
            .session
            .with_session(|session| session.dormant().sleep_facts(recto_session::TabId(u32::try_from(id).expect("a tab id"))))
            .map(|facts| recto_session::record::sleep_file_name(facts.sleep.file, &facts.path))
            .expect("a sleep file name")
    };
    let on_disk = sleep_files_on_disk(&app);
    assert!(!on_disk.contains(&name_of(a)), "written for a tab shown again: {on_disk:?}");
    assert!(!on_disk.is_empty(), "x's is written: {on_disk:?}");
    drop(app.quit());
}

/// Whatever the writer's taking now is written: nothing in flight at two looks 5 ms apart, so one it was about to take has been taken and written. Bounded.
fn writes_landed(app: &crate::testkit::headless::HeadlessApp) {
    let began = std::time::Instant::now();
    let idle = || app.record_json()["sleep"]["leaving"]["in_flight"] == false;
    while began.elapsed() < crate::testkit::headless::WAIT_LIMIT {
        if idle() {
            std::thread::sleep(std::time::Duration::from_millis(5)); // ratchet: poll
            if idle() {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1)); // ratchet: poll
    }
    panic!("a sleep file stayed in flight: {}", app.record_json()["sleep"]["leaving"]);
}

/// **Measures what switching tabs writes and how tabs wake,** asserting nothing: `tabs` documents opened, then the first `cycle` of them shown in a seeded
/// random order, each for `dwell` milliseconds of the app's clock (low to high), until `total` has gone by, the window up, and with `reading` moved to
/// another page before it's left; with the cool-down at `wait` and the hour's cap at `cap`. Each showing is classed as from warm (no wake), from its
/// facts (its pages on their paper at once), or with nothing.
fn switching(tabs: usize, cycle: usize, dwell: (u64, u64), total: u64, wait: u64, cap: u64, reading: bool) -> String {
    let disk = Disk::new();
    let docs: Vec<_> = (0..tabs).map(|at| disk.pdf(&format!("t{at}.pdf"), 6)).collect();
    let app = disk.builder().leaving_wait(wait).start();
    app.core().on_disk.sleeper().set_hour_cap(cap);
    let engine = app.core().engine.clone();
    engine.waking().window_up();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for doc in &docs {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
        app.advance(dwell.0);
    }
    let ids: Vec<u64> = (0..cycle).map(|at| tab_named(&app, &format!("t{at}.pdf"))).collect();
    let (mut warm, mut facts, mut bare, mut elapsed, mut seed) = (0, 0, 0, 0, 7_u64);
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        seed >> 33
    };
    let mut shown = ids[0];
    while elapsed < total {
        let mut id = ids[usize::try_from(next()).unwrap_or(0) % ids.len()];
        if id == shown {
            id = ids[(ids.iter().position(|at| *at == id).unwrap_or(0) + 1) % ids.len()];
        }
        let before = engine.report().0["last_wake"].clone();
        let wake = woken_at_once(&app, id, &mut view);
        if wake == before || wake["tab"] != id {
            warm += 1;
        } else if from_facts(&wake) {
            facts += 1;
        } else {
            bare += 1;
        }
        shown = id;
        // Reading on: another page of its six before it's left.
        if reading {
            full(&app, &["goto", &format!("{}", 1 + next() % 6)]);
            host_turns(&app, &mut view, true);
        }
        let dwell = dwell.0 + next() % (dwell.1 - dwell.0 + 1);
        app.advance(dwell);
        elapsed += dwell;
        // The clock jumps the whole dwell at once, so whatever came due in it is written now, just as the next tab's shown: as a person's dwell is spent, not
        // jumped, those writes are let land first (a tab shown in the milliseconds its file is being written wakes from the file before).
        writes_landed(&app);
    }
    app.advance(wait + 1);
    app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    let sleep = app.record_json()["sleep"].clone();
    let leaving = &sleep["leaving"];
    let line = format!(
        "{tabs} tabs, {cycle} switched among, {}-{} s each for {} s; cool-down {} s, hour's cap {} B: wakes from warm {warm}, from facts {facts}, with nothing \
         {bare}; sleep files written {}, bytes {}, put off {}, for the list {}; queue peak {} tabs, handed back {}, taken while being written {}",
        dwell.0 / 1000,
        dwell.1 / 1000,
        total / 1000,
        wait / 1000,
        cap,
        sleep["sleep_files_written"],
        sleep["sleep_bytes_written"],
        sleep["writes_put_off"],
        sleep["writes_for_list"],
        leaving["peak_tabs"],
        leaving["handed_back"],
        leaving["stale_skipped"],
    );
    drop(app.quit());
    line
}

/// **Measures switching, before and after the cool-down and the hour's cap ([`crate::sleeper::SLEEP_BYTES_PER_HOUR`]):** ten tabs flicked through twice, three seconds each; and twelve tabs with six
/// switched among every 5 to 15 seconds for an hour of the app's clock. Run when asked:
/// `cargo nextest run -p recto-app -p recto-render-service --run-ignored only --no-capture -E 'test(=core::headless::sleeping::switching_tabs_writes)'`.
#[test]
#[ignore = "a measurement, run when asked"]
fn switching_tabs_writes() {
    let (old_cap, cap) = (1024 * 1024, crate::sleeper::SLEEP_BYTES_PER_HOUR);
    let wait = crate::sleeper::LEAVING_WAIT_MS;
    let runs = [
        ("flicking", 10, 10, (3_000, 3_000), 60_000, false),
        ("an hour, reading on", 12, 6, (5_000, 15_000), 3_600_000, true),
    ];
    for (what, tabs, cycle, dwell, total, reading) in runs {
        for (when, wait, cap) in [("before", 0, old_cap), ("the hour's cap, no wait", 0, cap), ("after", wait, cap)] {
            println!("{what}, {when}: {}", switching(tabs, cycle, dwell, total, wait, cap, reading));
        }
    }
}

/// The name of `tab`'s sleep file, as its facts say now.
fn sleep_file_of(app: &crate::testkit::headless::HeadlessApp, tab: u64) -> String {
    app.core()
        .host
        .session
        .with_session(|session| {
            let id = recto_session::TabId(u32::try_from(tab).expect("a tab id"));
            Some(recto_session::record::sleep_file_name(
                session.tabs().own_stamp(id)?,
                &session.tabs().path_of(id)?,
            ))
        })
        .expect("a sleep file name")
}

/// Four tabs opened with the cool-down on, so the first, a, is pushed out of the warm ones and its sleep file waits. Says the app and a's tab.
fn a_pushed_out_and_waiting(disk: Disk, wait_ms: u64) -> (crate::testkit::headless::HeadlessApp, u64, recto_reader::Viewport) {
    let docs: Vec<_> = ["a", "b", "c", "d"].iter().map(|name| disk.pdf(&format!("{name}.pdf"), 6)).collect();
    let app = disk.builder().leaving_wait(wait_ms).start();
    let engine = app.core().engine.clone();
    engine.waking().window_up();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for doc in &docs {
        app.open(doc);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    let a = tab_named(&app, "a.pdf");
    assert_eq!(app.record_json()["sleep"]["leaving"]["queued"], 1, "a waits: {}", app.record_json()["sleep"]);
    (app, a, view)
}

/// Failure (the cool-down on a pushed-out tab's write, owner 2026-11-13): written at once, a tab flicked past and back wrote a sleep file each time (the drive's
/// wear), and with the wait but nothing handed back it would wake with nothing, as the page view let go of its view when it was pushed out and no file was
/// there yet. Shown again inside the wait, a wakes at once from the facts it left with, its pages on their paper with drawing held, and nothing's written
/// for it; left and pushed out again, its file is written at the wait and not a millisecond before.
#[test]
fn a_tab_shown_again_inside_its_cool_down_wakes_from_memory_and_nothing_is_written() {
    let wait = crate::sleeper::LEAVING_WAIT_MS;
    let (app, a, mut view) = a_pushed_out_and_waiting(Disk::new(), wait);
    app.advance(wait / 2);
    app.settle();
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert_eq!(wake["had"], "facts", "from the facts it left with: {wake}");
    assert!(wake["paper_up"].as_u64().unwrap() >= 1, "no page on its paper with nothing drawn: {wake}");
    let sleep = app.record_json()["sleep"].clone();
    assert_eq!(sleep["leaving"]["handed_back"], 1, "{sleep}");
    assert!(
        !sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a)),
        "written for a tab shown again inside its wait"
    );
    // Pushed out again, by showing the other three: written at the wait, not before.
    for name in ["b.pdf", "c.pdf", "d.pdf"] {
        full(&app, &["tap", &format!("#{}", tab_named(&app, name))]);
        host_turns(&app, &mut view, true);
    }
    // The writer has looked at the queue since the clock moved, and gone back to waiting: only then does "not written" say it chose not to. The kit's
    // settle doesn't wait for the sleep files' writer (`crate::settled`), so without this the look could come before the writer had woken.
    let waits = app.record_json()["sleep"]["leaving"]["waits"].as_u64().expect("the writer's waits");
    app.advance(wait - 1);
    app.settle();
    app.wait_until("the sleep files' writer to look at the queue at the new time", || {
        let leaving = app.record_json()["sleep"]["leaving"].clone();
        // Back to waiting, or (written early) through the queue and gone.
        leaving["in_flight"] == false && (leaving["waits"].as_u64().is_some_and(|now| now > waits) || leaving["writer"] == false)
    });
    assert!(!sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a)), "written before its wait was up");
    app.advance(1);
    app.wait_until("a's sleep file written at its wait", || {
        sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a))
    });
    app.wait_until("and the writer gone", || {
        app.record_json()["sleep"]["leaving"]["writer"] == false && sleep_writers() == 0
    });
    drop(app.quit());
}

/// Failure: what's handed back to a tab shown inside its wait is of the version it was copied from, so a file changed meanwhile was laid out at the old
/// version's sizes. a's file is written over inside the wait, then a's shown: its first frame is never from what was waiting.
#[test]
fn a_tab_whose_file_changed_inside_its_cool_down_is_never_laid_out_as_the_old_one() {
    let disk = Disk::new();
    let (app, a, mut view) = a_pushed_out_and_waiting(disk, crate::sleeper::LEAVING_WAIT_MS);
    let path = app.session_json()["tabs"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|tab| tab["id"] == a)
        .and_then(|tab| tab["path"].as_str().map(std::path::PathBuf::from))
        .expect("a's path");
    let other = app.disk().pdf("other.pdf", 9);
    std::fs::copy(&other, &path).expect("written over");
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert!(!from_facts(&wake), "the old version's facts: {wake}");
    drop(app.quit());
}

/// Failure: a quit with sleep files waiting in the cool-down went without them, so tabs the person had just left came back bare at the next launch. With
/// eight tabs waiting, a quit writes them, within its own time.
#[test]
fn a_quit_writes_the_sleep_files_waiting_in_the_cool_down() {
    let disk = Disk::new();
    let tabs = recto_session::MAX_WARM + crate::sleep_queue::MAX_LEAVING_TABS;
    let docs: Vec<_> = (0..tabs).map(|at| disk.pdf(&format!("t{at}.pdf"), 3)).collect();
    let app = disk.builder().leaving_wait(crate::sleeper::LEAVING_WAIT_MS).start();
    for doc in &docs {
        app.open(doc);
        app.settle();
    }
    assert_eq!(
        app.record_json()["sleep"]["leaving"]["queued"],
        crate::sleep_queue::MAX_LEAVING_TABS,
        "{}",
        app.record_json()["sleep"]
    );
    let began = std::time::Instant::now();
    let report = app.core().saver.flush(false, std::time::Duration::from_secs(5)).expect("the state written");
    println!("a flush with {} tabs waiting took {:?}", crate::sleep_queue::MAX_LEAVING_TABS, began.elapsed());
    assert_eq!(report.tabs, tabs);
    let leaving = app.record_json()["sleep"]["leaving"].clone();
    assert_eq!(leaving["queued"], 0, "{leaving}");
    // Those waiting, and the tab on screen's.
    assert!(
        sleep_files_on_disk(&app).len() > crate::sleep_queue::MAX_LEAVING_TABS,
        "{:?}",
        sleep_files_on_disk(&app)
    );
    drop(app.quit());
}

/// Failure (review follow-up 2, 2026-11-07, moved here from the app test `restore::session_sleep_keeps_to_the_limits_on_writing` on 2026-11-13): an agent going
/// tap, move, `session sleep` wrote a new sleep file every time it asked. It keeps to the limits anything else does: sleeps inside the gap, at one page or
/// another, write one file (a tab's facts don't change with its place); a change of its pages is written once inside the gap (counted as
/// `writes_for_list`), which the hour's cap bounds: with the cap set at what's been written, the next change-and-sleep is put off and counted.
#[test]
fn session_sleep_keeps_to_the_limits_on_writing() {
    let disk = Disk::new();
    let (doc, other) = (disk.pdf("doc.pdf", 12), disk.pdf("other.pdf", 2));
    let app = disk.start();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    for file in [&doc, &other] {
        app.open(file);
        view = crate::testkit::host_standin::shown_again(&view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
        host_turns(&app, &mut view, true);
    }
    let (doc, other) = (tab_named(&app, "doc.pdf"), tab_named(&app, "other.pdf"));
    let counts = || app.record_json()["sleep"].clone();
    let mut turn = |page: &str, rotate: bool| {
        full(&app, &["tap", &format!("#{doc}")]);
        host_turns(&app, &mut view, true);
        full(&app, &["goto", page]);
        if rotate {
            full(&app, &["page", "rotate", page, "--by", "90"]);
        }
        host_turns(&app, &mut view, true);
        full(&app, &["tap", &format!("#{other}")]);
        host_turns(&app, &mut view, true);
        full(&app, &["session", "sleep", "--tab", &format!("#{doc}")]);
        host_turns(&app, &mut view, true);
    };
    for page in ["3", "3", "5", "7"] {
        turn(page, false);
    }
    let unchanged = counts();
    assert_eq!(unchanged["sleep_files_written"], 1, "one file, wherever it's left: {unchanged}");
    assert!(unchanged["writes_put_off"].as_u64().unwrap() >= 2, "{unchanged}");
    turn("4", true);
    let rotated = counts();
    assert_eq!(rotated["sleep_files_written"], 2, "its pages changed, so its file's written once: {rotated}");
    assert_eq!(rotated["writes_for_list"], 1, "{rotated}");
    // At the hour's cap: the next is put off, and counted.
    app.core()
        .on_disk
        .sleeper()
        .set_hour_cap(rotated["sleep_bytes_written"].as_u64().expect("bytes"));
    let put_off = rotated["writes_put_off"].as_u64().unwrap();
    turn("6", true);
    let capped = counts();
    assert_eq!(capped["sleep_files_written"], 2, "past the hour's cap: {capped}");
    assert_eq!(capped["writes_put_off"].as_u64().unwrap(), put_off + 1, "{capped}");
    drop(app.quit());
}

/// Failure (the cool-down's own measurement, 2026-11-13: a tab shown again inside its wait woke from the older file on disk, read on the way, and not from
/// what was handed back). a's file is written; a's pages change (a page turned) and it's pushed out again, and shown inside the wait: the older file, of its
/// pages before, would be refused, and what it left with is what it wakes from.
#[test]
fn a_tab_shown_inside_its_cool_down_wakes_from_what_it_left_with_not_an_older_file() {
    let wait = crate::sleeper::LEAVING_WAIT_MS;
    let (app, a, mut view) = a_pushed_out_and_waiting(Disk::new(), wait);
    app.advance(wait);
    app.wait_until("a's first file written", || sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a)));
    full(&app, &["tap", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["page", "rotate", "1", "--by", "90"]);
    host_turns(&app, &mut view, true);
    for name in ["b.pdf", "c.pdf", "d.pdf"] {
        full(&app, &["tap", &format!("#{}", tab_named(&app, name))]);
        host_turns(&app, &mut view, true);
    }
    app.advance(wait / 2);
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert_eq!(wake["had"], "facts", "woken from the older file of its pages before, refused: {wake}");
    drop(app.quit());
}

/// A sleep file's bytes as a build of `format` wrote them (none said, as 0.1.0's), with `pictures` bytes after its facts as format 3's held a picture.
fn sleep_file_bytes_of_format(format: Option<u32>, pictures: usize) -> Vec<u8> {
    let sleep = recto_session::sleep::SleepFile {
        file: recto_platform::FileStamp {
            size: 1,
            mtime_ns: 1,
            device: 1,
            inode: 1,
        },
        file_pages: 3,
        pages: 3,
        list_seq: 0,
        renderer_only: false,
        version: None,
        toc: recto_core::Toc::default(),
        usual: Some((612.0, 792.0)),
        sizes: Vec::new(),
    };
    let good = recto_session::sleep::encode_sleep(&sleep);
    let body = std::str::from_utf8(&good[good.iter().position(|byte| *byte == b'\n').expect("a header") + 1..]).expect("text");
    let now = format!("{{\"sleep_format\":{},", recto_session::sleep::SLEEP_FORMAT);
    let facts = body.trim_end().replacen(
        &now,
        &format.map_or_else(|| "{".to_string(), |format| format!("{{\"sleep_format\":{format},")),
        1,
    );
    let mut bytes = facts.into_bytes();
    bytes.push(b'\n');
    bytes.extend(std::iter::repeat_n(0x7f, pictures));
    recto_session::record::seal(recto_session::sleep::SLEEP_KIND, 0, &bytes)
}

/// Failure (owner, 2026-11-15: no picture of a page is kept): sleep files an earlier version wrote, 0.1.0's and format 3's, with pictures of pages in them,
/// stay in the state folder after the update, for a tab that's never woken again for up to 30 days, readable by any program of the person's. On the app's own
/// core: a state folder with one of each, a file of this format and one that isn't a sleep file beside them, and a launch: once it's settled, both older
/// ones are gone, the others are as they were, and the pass is said in `qa state`. No thread of it is left.
#[test]
fn the_launch_removes_every_sleep_file_of_an_older_format() {
    let disk = Disk::new();
    let folder = disk.paths().root.join("session").join("journals");
    std::fs::create_dir_all(&folder).expect("the folder");
    let put = |name: &str, bytes: &[u8]| {
        let path = folder.join(name);
        recto_platform::local::write_private_file(&path, bytes).expect("written");
        path
    };
    let of_0_1_0 = put("sleep-00000000000000c1.bin", &sleep_file_bytes_of_format(None, 70_000));
    let format_3 = put("sleep-00000000000000c2.bin", &sleep_file_bytes_of_format(Some(3), 70_000));
    let now = put(
        "sleep-00000000000000c3.bin",
        &sleep_file_bytes_of_format(Some(recto_session::sleep::SLEEP_FORMAT), 0),
    );
    // Not a sleep file's name, though it starts as one does (the store's own unit test holds journals and the rest).
    let not_one = put("sleep-notes.txt", b"not looked at");
    let app = disk.start();
    app.wait_until("the pass done", || !app.record_json()["sleep"]["older_files"].is_null());
    app.settle();
    assert!(!of_0_1_0.exists() && !format_3.exists(), "an older sleep file is still there");
    assert!(now.exists(), "this format's was removed");
    assert_eq!(std::fs::read(&not_one).expect("a file that isn't a sleep file"), b"not looked at");
    let older = app.record_json()["sleep"]["older_files"].clone();
    assert_eq!((older["looked_at"].as_u64(), older["removed"].as_u64()), (Some(3), Some(2)), "{older}");
    app.wait_until("its thread gone", || {
        !recto_platform::local::thread_names(std::process::id())
            .expect("this process's threads")
            .iter()
            .any(|name| name == "recto-sleep-sweep")
    });
    drop(app.quit());
}

/// The facts a sleep file may hold, by their keys: what this build writes, and nothing of a page's picture.
const FACT_KEYS: [&str; 14] = [
    "sleep_format",
    "file",
    "file_pages",
    "pages",
    "list_seq",
    "renderer_only",
    "version",
    "truncated",
    "toc",
    "tops",
    "labels",
    "labels_truncated",
    "usual",
    "sizes",
];

/// Every sleep file in `root`'s state folder, read back from its bytes as anything else would: each must be one line of facts after its header, with no
/// key but [`FACT_KEYS`] and not a byte after them, so no picture of a page is in it whatever wrote it. Says how many there are.
fn every_sleep_file_holds_facts_only(root: &std::path::Path, route: &str) -> usize {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(recto_session::record::is_sleep_name)
            {
                found.push(path);
            }
        }
    }
    for path in &found {
        let bytes = std::fs::read(path).expect("a sleep file");
        let (_, _, body) = recto_session::record::open_sealed(recto_session::sleep::SLEEP_KIND, &bytes, recto_session::record::CURRENT_VERSION)
            .unwrap_or_else(|error| panic!("{route}: {} isn't a sealed sleep file: {error:?}", path.display()));
        let end = body.iter().position(|byte| *byte == b'\n').expect("a line of facts");
        assert_eq!(
            end + 1,
            body.len(),
            "{route}: {} bytes after the facts in {}",
            body.len() - end - 1,
            path.display()
        );
        let facts: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&body[..end]).expect("facts");
        let other: Vec<&String> = facts.keys().filter(|key| !FACT_KEYS.contains(&key.as_str())).collect();
        assert!(other.is_empty(), "{route}: {} holds {other:?}", path.display());
    }
    found.len()
}

/// Failure (owner, 2026-11-15: "Remove the image loading feature all together"): some route that writes a sleep file still puts a picture of a page in it,
/// in the state folder, where any program of the person's can read it. Every route there is, each seen to write: a tab pushed out of the warm ones, an
/// agent's `session sleep`, a tab left alone going dormant, memory pressure, an update's hand-off and a quit's flush; after each, every file on disk is read
/// back from its bytes and holds its facts and nothing else.
#[test]
fn every_route_that_writes_a_sleep_file_writes_no_picture() {
    let disk = Disk::new();
    let docs: Vec<_> = ["a", "b", "c", "d", "e", "f"].iter().map(|name| disk.pdf(&format!("{name}.pdf"), 4)).collect();
    let app = disk.start();
    let engine = app.core().engine.clone();
    engine.waking().window_up();
    let root = app.disk().paths().root.clone();
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    let open = |doc: &std::path::Path, view: &mut recto_reader::Viewport| {
        app.open(doc);
        *view = crate::testkit::host_standin::shown_again(&*view, window_view());
        crate::testkit::host_standin::report_where_the_host_is(&app, view);
        host_turns(&app, view, true);
    };
    for doc in &docs[..4] {
        open(doc, &mut view);
    }
    let queue_written = || app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    queue_written();
    let mut seen = every_sleep_file_holds_facts_only(&root, "pushed out");
    assert!(seen >= 1, "a's pushed out: {}", app.record_json()["sleep"]);
    full(&app, &["session", "sleep", "--tab", &format!("#{}", tab_named(&app, "b.pdf"))]);
    let now = every_sleep_file_holds_facts_only(&root, "session sleep");
    assert!(now > seen, "b's slept: {}", app.record_json()["sleep"]);
    seen = now;
    app.core().qa_session_verb("dormant_after", "0").expect("a QA verb").expect("set");
    app.wait_until("left alone, gone dormant", || {
        app.record_json()["sleep"]["gone_dormant"]["left_alone"].as_u64().unwrap_or(0) >= 1
    });
    app.core().qa_session_verb("dormant_after", "86400").expect("a QA verb").expect("set");
    queue_written();
    let now = every_sleep_file_holds_facts_only(&root, "left alone");
    assert!(now > seen, "c's gone dormant: {}", app.record_json()["sleep"]);
    seen = now;
    open(&docs[4], &mut view);
    open(&docs[5], &mut view);
    app.core().qa_session_verb("memory_pressure", "warning").expect("a QA verb").expect("pressure");
    app.settle();
    queue_written();
    let now = every_sleep_file_holds_facts_only(&root, "memory pressure");
    assert!(now > seen, "what pressure put to sleep: {}", app.record_json()["sleep"]);
    seen = now;
    // The tab on screen's file goes at a hand-off, and again at a quit: removed between, so each is seen to write it.
    let on_screen = |app: &crate::testkit::headless::HeadlessApp| {
        let tab = app.session_json()["active"].as_u64().expect("a tab on screen");
        root.join("session").join("journals").join(sleep_file_of(app, tab))
    };
    let _ = std::fs::remove_file(on_screen(&app));
    app.core().hand_off(crate::testkit::headless::WAIT_LIMIT).expect("handed off");
    assert!(on_screen(&app).exists(), "the hand-off wrote the tab on screen's");
    every_sleep_file_holds_facts_only(&root, "a hand-off");
    let app = disk_again(app);
    // Drawn, so its worker has said its length and what came from its sleep file is checked (`Session::file_length_is`): only then is it written again.
    let mut view = crate::testkit::host_standin::first_shown(window_view());
    crate::testkit::host_standin::report_where_the_host_is(&app, &mut view);
    host_turns(&app, &mut view, true);
    let file = on_screen(&app);
    let _ = std::fs::remove_file(&file);
    // The disk kept past the quit, as the folder goes with it.
    let (_, disk) = app.quit();
    assert!(file.exists(), "the quit wrote the tab on screen's");
    assert!(every_sleep_file_holds_facts_only(&root, "a quit") >= seen);
    drop(disk);
}

/// The app after a hand-off, gone, and started again on the same disk, as the copy that takes over is.
fn disk_again(app: crate::testkit::headless::HeadlessApp) -> crate::testkit::headless::HeadlessApp {
    let disk = app.kill();
    disk.start()
}
