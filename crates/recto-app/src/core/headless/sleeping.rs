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
/// own, holding its pictures while it waited on the one-at-a-time lock, so with a slow disk the app's threads and memory grew with the tabs leaving. And
/// (9988d7c, review 1) memory pressure wrote the tabs going dormant on the session thread, behind every file queued, while nothing else was served. With every
/// sleep file's write held at its seam, as a disk that doesn't answer, tabs opened one after another past the warm ones and the queue's cap on tabs: never more
/// than one writer thread, the pictures queued and being written under their cap, those past the cap on tabs dropped and counted; memory pressure then, with
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
        leaving["peak_bytes"].as_u64().unwrap() <= leaving["cap_bytes"].as_u64().unwrap(),
        "the pictures queued passed their cap: {leaving}"
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
    assert_eq!(rest["queued_bytes"], 0, "nothing's held for it at rest: {rest}");
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

/// The view the host would have, at the top of the document: a Retina window, the page at its fit.
fn window_view() -> recto_reader::Viewport {
    recto_reader::Viewport {
        x: 0.0,
        y: 0.0,
        width: 1168.0,
        height: 900.0,
        frame_width: 1168.0,
        magnification: 1.0,
        backing: 2.0,
        live: false,
        scrolled_to: u64::MAX,
    }
}

/// **Stands in for the host's UI thread,** which the kit has none of: takes what the page view says, scrolls where it's sent and says where it is now, until the session thread has nothing queued and nothing comes, and (with `settled`) everything's drawn, a tab off screen's place included. Bounded.
fn host_turns(app: &crate::testkit::headless::HeadlessApp, view: &mut recto_reader::Viewport, settled: bool) {
    let engine = &app.core().engine;
    let began = std::time::Instant::now();
    loop {
        assert!(
            app.core().core.flush(crate::testkit::headless::WAIT_LIMIT),
            "the session thread took what was queued"
        );
        let mut ops = engine.drain();
        engine.applied();
        let mut quiet = ops.is_empty();
        while let Some(op) = ops.pop() {
            if let recto_reader::Op::ScrollTo { y, seq } = op {
                view.y = y;
                view.scrolled_to = seq;
                ops = engine.set_viewport(*view);
                engine.applied();
                quiet = false;
            }
        }
        // Drawing for a tab off screen's place isn't what settling waits for, so it's waited for here too.
        if quiet && (!settled || (engine.wait_settled(std::time::Duration::from_millis(20)) && engine.report().0["sleep_draws"]["under_way"] == false)) {
            return;
        }
        assert!(
            began.elapsed() < crate::testkit::headless::WAIT_LIMIT,
            "the page view didn't come to rest: {}",
            engine.report().0
        );
    }
}

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

/// Opens a document, at the top of a window, drawn; then sends it to page 30 with nothing drawn there, as a `goto` and a change of tab before the renderer answers do.
fn opened_and_moved(app: &crate::testkit::headless::HeadlessApp, doc: &std::path::Path, view: &mut recto_reader::Viewport) {
    app.open(doc);
    *view = window_view();
    drop(app.core().engine.set_viewport(*view));
    host_turns(app, view, true);
    app.core().engine.hold_drawing(true);
    full(app, &["goto", "30"]);
    host_turns(app, view, false);
}

/// Failure (owner, 2026-11-13, asked whether the place should be drawn first: "I think it makes sense, and extra draw is not costly right?"): a tab moved and left before its place was drawn went to sleep with no picture of where it was, and woke to bare paper until the renderer caught up. On the app's own core with real workers: tab a, moved and left for b, has its place drawn once b is, and put to sleep (after the system has taken back what's discardable) and woken it starts from a picture of that place from its sleep file. Tab c, moved and left the same way with its drawing held till it sleeps, as it was before this, wakes with nothing to show. And leaving b where it was drawn asks the renderer for nothing.
#[test]
fn a_tab_moved_and_left_before_its_place_is_drawn_wakes_with_a_picture_of_it() {
    let disk = Disk::new();
    let (a, b, c) = (disk.pdf("a.pdf", 40), disk.pdf("b.pdf", 3), disk.pdf("c.pdf", 40));
    let app = disk.start();
    let engine = app.core().engine.clone();
    let mut view = window_view();
    opened_and_moved(&app, &a, &mut view);
    app.open(&b);
    engine.hold_drawing(false);
    view = window_view();
    drop(engine.set_viewport(view));
    host_turns(&app, &mut view, true);
    let draws = engine.report().0["sleep_draws"].clone();
    assert_eq!(draws["leaves_owing"], 1, "{draws}");
    assert_eq!(draws["ended"]["done"], 1, "a's place is drawn: {draws}");
    assert!(draws["pictures"].as_u64().unwrap() >= 1, "{draws}");

    // b left where it was drawn: nothing more's asked of the renderer for it.
    let jobs = draws["jobs"].clone();
    opened_and_moved(&app, &c, &mut view);
    let draws = engine.report().0["sleep_draws"].clone();
    assert_eq!(draws["leaves_pictured"], 1, "{draws}");
    assert_eq!(draws["jobs"], jobs, "{draws}");
    // c left for b with its drawing held, so it sleeps as a tab left that way did before.
    full(&app, &["tap", &format!("#{}", tab_named(&app, "b.pdf"))]);
    host_turns(&app, &mut view, false);
    // What the tabs off screen have drawn is discardable, and the system takes it back whenever it likes: what a sleeps with is what it was left with.
    assert!(engine.empty_discardable() > 0);
    for name in ["a.pdf", "c.pdf"] {
        full(&app, &["session", "sleep", "--tab", &format!("#{}", tab_named(&app, name))]);
    }
    engine.hold_drawing(false);
    host_turns(&app, &mut view, true);

    let mut woke = |name: &str| {
        full(&app, &["tap", &format!("#{}", tab_named(&app, name))]);
        host_turns(&app, &mut view, true);
        engine.report().0["last_wake"].clone()
    };
    let a_woke = woke("a.pdf");
    assert_eq!(a_woke["tab"], tab_named(&app, "a.pdf"), "{a_woke}");
    assert_eq!(a_woke["had"], "sleep_file", "a picture of its place, from its sleep file: {a_woke}");
    assert!(a_woke["sleep_in_view"].as_u64().unwrap() >= 1, "{a_woke}");
    let c_woke = woke("c.pdf");
    assert_eq!(c_woke["had"], "nothing", "as it was before: {c_woke}");
    drop(app.quit());
}

/// Failure (review 1 of drawing a tab's place before it sleeps, F9): a warm tab off screen whose pages are deleted keeps the pictures it was left with, filed by where they were, so its sleep file would wake it with a picture of another page where one was deleted, which is worse than bare paper. Tab a, left drawn at its top, has its first page deleted while it's off screen and is put to sleep; woken, nothing from its sleep file is in view. Tab c, the same with no edit, wakes from its sleep file, so the look isn't empty for want of one.
#[test]
fn a_tab_whose_pages_change_off_screen_wakes_with_no_picture_of_another_page() {
    let disk = Disk::new();
    let (a, b, c) = (disk.pdf("a.pdf", 6), disk.pdf("b.pdf", 3), disk.pdf("c.pdf", 6));
    let app = disk.start();
    let engine = app.core().engine.clone();
    let mut view = window_view();
    for doc in [&c, &a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    let (a, c) = (tab_named(&app, "a.pdf"), tab_named(&app, "c.pdf"));
    full(&app, &["page", "delete", "1", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    for tab in [a, c] {
        full(&app, &["session", "sleep", "--tab", &format!("#{tab}")]);
    }
    host_turns(&app, &mut view, true);
    let mut woke = |tab: u64| {
        full(&app, &["tap", &format!("#{tab}")]);
        host_turns(&app, &mut view, true);
        engine.report().0["last_wake"].clone()
    };
    let c_woke = woke(c);
    assert_eq!(c_woke["had"], "sleep_file", "unedited, it wakes from its sleep file: {c_woke}");
    let a_woke = woke(a);
    assert_eq!(a_woke["tab"], a, "{a_woke}");
    assert_eq!(a_woke["sleep_in_view"], 0, "a picture from before its pages changed, of another page: {a_woke}");
    drop(app.quit());
}

/// Failure, the same rule from the other side: a tab already asleep, with a sleep file on disk of its pages as they were, has its first page deleted while it's off screen (an agent's edit), and is shown: its first frame comes from the old file, a picture of the deleted page where page 2 now is. The wake takes no picture from a sleep file of another list of pages than the session's now.
#[test]
fn a_tab_asleep_whose_pages_change_wakes_with_no_picture_from_before() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 6), disk.pdf("b.pdf", 3));
    let app = disk.start();
    let engine = app.core().engine.clone();
    let mut view = window_view();
    for doc in [&a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
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
    assert_eq!(woke["sleep_in_view"], 0, "a picture from before its pages changed: {woke}");
    drop(app.quit());
}

/// Whether a wake's first frame came from pictures it was given: a sleep file read from disk, or what the tab left with, handed back inside its cool-down.
fn from_a_picture(wake: &serde_json::Value) -> bool {
    wake["had"] == "sleep_file" || wake["had"] == "handed_back"
}

/// Shows `tab` from asleep with drawing held, as the first frame has it, and says how that wake went; then lets it draw and settle.
fn woken_at_once(app: &crate::testkit::headless::HeadlessApp, tab: u64, view: &mut recto_reader::Viewport) -> serde_json::Value {
    let engine = &app.core().engine;
    engine.hold_drawing(true);
    full(app, &["tap", &format!("#{tab}")]);
    host_turns(app, view, false);
    let wake = engine.report().0["last_wake"].clone();
    engine.hold_drawing(false);
    host_turns(app, view, true);
    wake
}

/// Failure (the first worker's quiet run of 2026-11-13, `02-wake-at-once-dark.png`): a tab put to sleep and woken a second time in a run, with the look changed
/// between, showed bare paper at once, while `last_wake` said it had pictures from its sleep file (`bare_with_picture=2`, `first_frame_us=null`). The second sleep
/// came inside its sleep file's write gap, so the file wasn't written again and still held pictures for the other paper, which the page view keeps but never puts
/// on a page of this paper. On the app's own core, the window up so a bare frame counts: three cycles of sleep and wake for one tab, white, white again, then
/// dark paper, each wake's first frame (drawing held) covered from its sleep file, the dark one written once inside the gap for its paper; and then the look
/// changed back while it's asleep, which no write can put right: it wakes bare and says it had nothing, never that it had a picture it didn't show.
#[test]
fn a_tab_woken_again_after_the_look_changed_shows_a_picture_at_once() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 3));
    let app = disk.start();
    let engine = app.core().engine.clone();
    engine.waking().window_up();
    let mut view = window_view();
    for doc in [&a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    let (a, b) = (tab_named(&app, "a.pdf"), tab_named(&app, "b.pdf"));
    let sleep_record = || app.record_json()["sleep"].clone();
    let mut bytes_before_dark = 0;
    for (cycle, paper) in ["white", "white", "dark"].into_iter().enumerate() {
        full(&app, &["view", "--paper", paper]);
        full(&app, &["tap", &format!("#{a}")]);
        host_turns(&app, &mut view, true);
        full(&app, &["tap", &format!("#{b}")]);
        host_turns(&app, &mut view, true);
        bytes_before_dark = sleep_record()["sleep_bytes_written"].as_u64().expect("bytes");
        full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
        host_turns(&app, &mut view, true);
        let wake = woken_at_once(&app, a, &mut view);
        assert_eq!(wake["tab"], a, "cycle {cycle} ({paper}): {wake}");
        assert_eq!(
            wake["bare_with_picture"], 0,
            "cycle {cycle} ({paper}): it had a picture and showed bare paper: {wake}"
        );
        assert_eq!(wake["had"], "sleep_file", "cycle {cycle} ({paper}): {wake}");
        assert!(
            wake["first_frame_us"].is_u64(),
            "cycle {cycle} ({paper}): the first frame, drawing held, isn't covered: {wake}"
        );
    }
    let written = sleep_record();
    assert_eq!(written["writes_for_paper"], 1, "the change of look, once: {written}");
    assert!(
        written["writes_put_off"].as_u64().unwrap() >= 1,
        "the second white sleep is put off as before: {written}"
    );
    println!(
        "a sleep file written inside the gap for the change of look: {} bytes",
        written["sleep_bytes_written"].as_u64().unwrap() - bytes_before_dark
    );
    // Asleep with dark pictures, the look goes back to white: woken, nothing from its file covers a page of this paper, and it says so. (Its pictures are
    // still given: at a launch the paper can turn after the first frame is asked for, and the pictures are what it then shows.)
    full(&app, &["tap", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["view", "--paper", "white"]);
    host_turns(&app, &mut view, true);
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert_eq!(wake["had"], "nothing", "{wake}");
    assert!(wake["blank_frames"].as_u64().unwrap() > 0, "it was bare, and says so: {wake}");
    assert_eq!(wake["bare_with_picture"], 0, "{wake}");
    drop(app.quit());
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
    let engine = app.core().engine.clone();
    let mut view = window_view();
    for doc in [&a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
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
    let mut view = window_view();
    for doc in [&a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    let (a, b) = (tab_named(&app, "a.pdf"), tab_named(&app, "b.pdf"));
    let decided = || {
        let sleep = app.record_json()["sleep"].clone();
        sleep["writes_put_off"].as_u64().expect("put off") + sleep["sleep_files_written"].as_u64().expect("written")
    };
    let held = app.hold_session_thread();
    let id = recto_session::TabId(u32::try_from(a).expect("a tab id"));
    assert_eq!(app.core().go_dormant(&[id], super::super::sleeper::Why::LeftAlone), 1, "a goes dormant");
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
    let engine = app.core().engine.clone();
    let mut view = window_view();
    for doc in [&a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
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

/// Failure (a quiet run of 2026-11-13, `dark-paper-sheet-woken-dark.png`: of pages 2, 4, 7, 8 and 9 each left, slept and woken in turn, the first woke from its
/// sleep picture and the later ones to an empty pane, `sleep_pictures=1 sleep_in_view=0`): a tab whose sleep file was written under a minute before (the write
/// gap, for the drive's sake) had its next write put off, so the file kept its picture of the page the tab was left on before, which a wake at the page it's
/// at now can't show. Switching through more tabs than are kept warm does it with no forced sleep: each tab pushed out has its file written, so a tab read on
/// and left again within the minute woke bare. On the app's own core, the window up so a bare frame counts: four tabs (its first file written as the others
/// opened), a moved to page 6 and then 10, each time the other three shown in turn so a is pushed out of the warm ones, and then shown, drawing held: each
/// wake's first frame is covered from its sleep file, with a picture of the page it was left on. Left again at page 10, its file already shows that page, so
/// its write is put off as before (the store's own byte compare would hide a write of the same bytes, so the put-off count is what's asked).
#[test]
fn a_tab_left_again_at_another_page_within_a_minute_wakes_with_a_picture_of_that_page() {
    let disk = Disk::new();
    let docs = [disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 3), disk.pdf("c.pdf", 3), disk.pdf("d.pdf", 3)];
    let app = disk.start();
    let engine = app.core().engine.clone();
    engine.waking().window_up();
    let mut view = window_view();
    for doc in &docs {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    let a = tab_named(&app, "a.pdf");
    let others: Vec<u64> = ["b.pdf", "c.pdf", "d.pdf"].iter().map(|name| tab_named(&app, name)).collect();
    let put_off = || app.record_json()["sleep"]["writes_put_off"].as_u64().expect("writes put off");
    let mut put_off_before = 0;
    for page in ["6", "10", "10"] {
        full(&app, &["tap", &format!("#{a}")]);
        host_turns(&app, &mut view, true);
        full(&app, &["goto", page]);
        host_turns(&app, &mut view, true);
        put_off_before = put_off();
        for other in &others {
            full(&app, &["tap", &format!("#{other}")]);
            host_turns(&app, &mut view, true);
        }
        let wake = woken_at_once(&app, a, &mut view);
        assert_eq!(wake["tab"], a, "page {page}: {wake}");
        assert!(from_a_picture(&wake), "page {page}: woken bare, its sleep file of another page: {wake}");
        assert!(wake["sleep_in_view"].as_u64().unwrap() >= 1, "page {page}: {wake}");
        assert!(
            wake["first_frame_us"].is_u64(),
            "page {page}: the first frame, drawing held, isn't covered: {wake}"
        );
    }
    let sleep = app.record_json()["sleep"].clone();
    assert!(
        sleep["writes_put_off"].as_u64().unwrap() > put_off_before,
        "left again at the same page inside the gap, its write is put off as before: {sleep}"
    );
    drop(app.quit());
}

/// Failure (the cool-down's review, 2026-11-13, S2): a tab's file written at page 5, then page 5 rotated and the tab left inside the minute's gap: the file's
/// picture of page 5 counted as showing the place, so the write was put off, and the wake refused that file for its other list of pages and was bare, the bug
/// the place rule fixes, for an edited tab. On the app's own core, the window up: a slept at page 5, woken, page 5 rotated, slept again inside the gap: written,
/// for the place, and woken from a picture.
#[test]
fn a_tab_left_inside_the_gap_after_a_page_was_rotated_wakes_with_a_picture() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 3));
    let app = disk.start();
    let engine = app.core().engine.clone();
    engine.waking().window_up();
    let mut view = window_view();
    for doc in [&a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
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
        after["writes_for_place"].as_u64().unwrap(),
        before["writes_for_place"].as_u64().unwrap() + 1,
        "put off on a file of the list before the rotation: {after}"
    );
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert_eq!(wake["had"], "sleep_file", "woken bare, its file of the list before: {wake}");
    drop(app.quit());
}

/// The person's own command, as the window sends it for them: through the one executor as theirs, never an agent's.
fn person(app: &crate::testkit::headless::HeadlessApp, words: &[&str]) {
    let words: Vec<String> = words.iter().map(|word| (*word).to_owned()).collect();
    app.person(&recto_agent::table::parse_cli(&words).expect("a command").wire());
    app.settle();
}

/// Failure (the cool-down's review, 2026-11-13, S3): an agent looping a move and `session sleep` used the whole app's hour of sleep files, so every wake of the
/// person's after was bare for the hour. With agents' share set at one and a half of the agent's first file: that file is written, the next (at another place,
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
    let engine = app.core().engine.clone();
    let mut view = window_view();
    for file in &docs[..2] {
        app.open(file);
        view = window_view();
        drop(engine.set_viewport(view));
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
    app.core().sleeper.set_agent_hour_cap(file + file / 2);
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
        view = window_view();
        drop(engine.set_viewport(view));
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

/// Failure (the sleep branch's second review, 2026-11-13): an agent's share counted a write of a tab's facts alone, with no file there, as one put off, so
/// the agent was told its sleep file wasn't written when its facts were; and an agent's `session save` that its share kept from writing the tab on screen's
/// file said nothing, though a crash after it leaves the next launch's first frame without a picture. With agents' share set small: an agent's sleep of a tab
/// with no file writes its facts and says so; then, the share used, an agent's save writes the record, not the tab on screen's file, and says that.
#[test]
fn an_agents_answer_says_what_its_share_did_to_its_own_sleep_file() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 6), disk.pdf("b.pdf", 6));
    let app = disk.start();
    let engine = app.core().engine.clone();
    let mut view = window_view();
    for doc in [&a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    // Room for a tab's facts, not its pictures (a picture of a synthetic page packs to a few KB).
    app.core().sleeper.set_agent_hour_cap(1_000);
    let a = tab_named(&app, "a.pdf");
    let slept = full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    let note = slept["note"]
        .as_str()
        .unwrap_or_else(|| panic!("nothing said of a's facts written alone: {slept}"));
    assert!(note.contains("without pictures"), "{note}");
    let sleep = app.record_json()["sleep"].clone();
    assert_eq!(sleep["sleep_files_written"], 1, "its facts: {sleep}");
    let used = sleep["agent_bytes_this_hour"].as_u64().expect("agents' bytes");
    assert!(used > 0 && used <= 1_000, "{sleep}");
    app.core().sleeper.set_agent_hour_cap(used);
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
/// every wake of the person's after was bare. Six tabs, the gap passed before each step, agents' share set at two and a half files: the agent's tap-and-goto
/// cycle through them (six push-outs) writes up to its share and then is put off, never past it; and the person's own cycle after is still written, none of
/// it counted as the agent's.
#[test]
fn an_agent_cycling_through_tabs_stops_at_its_share_and_the_persons_cycling_is_still_written() {
    let disk = Disk::new();
    let docs: Vec<_> = (0..6).map(|at| disk.pdf(&format!("t{at}.pdf"), 6)).collect();
    let app = disk.start();
    // Nothing goes dormant by itself here: only tabs pushed out of the warm ones are written.
    app.core().sleeper.set_after(24 * 60 * 60);
    let engine = app.core().engine.clone();
    let mut view = window_view();
    for doc in &docs {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
    let sleep = app.record_json()["sleep"].clone();
    let (files, bytes) = (sleep["sleep_files_written"].as_u64().unwrap(), sleep["sleep_bytes_written"].as_u64().unwrap());
    assert!(files >= 3, "the person's opens pushed three out: {sleep}");
    assert_eq!(sleep["agent_bytes_this_hour"], 0, "{sleep}");
    let share = bytes / files * 5 / 2;
    app.core().sleeper.set_agent_hour_cap(share);
    let tabs: Vec<u64> = (0..6).map(|at| tab_named(&app, &format!("t{at}.pdf"))).collect();
    let gap = u64::from(crate::sleeper::SLEEP_WRITE_EVERY_SECONDS) * 1000 + 1;
    let mut cycle = |by_agent: bool| {
        for (step, tab) in tabs.iter().enumerate() {
            app.advance(gap);
            // Each cycle at other pages than the last, so no file comes out the same bytes (which the store doesn't write again).
            let page = format!("{}", 2 + (step + usize::from(!by_agent)) % 5);
            if by_agent {
                full(&app, &["tap", &format!("#{tab}")]);
                host_turns(&app, &mut view, true);
                full(&app, &["goto", &page]);
            } else {
                person(&app, &["tap", &format!("#{tab}")]);
                host_turns(&app, &mut view, true);
                person(&app, &["goto", &page]);
            }
            host_turns(&app, &mut view, true);
            app.wait_until("the queue written", || app.record_json()["sleep"]["leaving"]["writer"] == false);
        }
    };
    cycle(true);
    let agents = app.record_json()["sleep"].clone();
    assert!(
        agents["agent_share_used"].as_u64().unwrap() >= 1,
        "the agent's cycle never met its share: {agents}"
    );
    let agent_bytes = agents["agent_bytes_this_hour"].as_u64().unwrap();
    assert!(agent_bytes > 0, "nothing of the agent's was written up to its share: {agents}");
    assert!(agent_bytes <= share, "past the share, {agent_bytes} of {share}: {agents}");
    let written = agents["sleep_files_written"].as_u64().unwrap();
    cycle(false);
    let persons = app.record_json()["sleep"].clone();
    assert!(
        persons["sleep_files_written"].as_u64().unwrap() >= written + 3,
        "the person's own cycle, after the agent's share was used, wasn't written: {persons}"
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
    app.core().sleeper.panic_next_leaving_writer();
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
/// written at the wait, after the close, or its pictures held for nothing till then. Closed inside the wait: nothing of it is held, and past the wait no file of
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
    assert_eq!(leaving["queued_bytes"], 0, "held for a closed tab: {leaving}");
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
    app.core().frontmost_changed(true, true);
    app.core().frontmost_changed(false, false);
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
    app.core().sleeper.panic_next_leaving_writer();
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
    app.core().frontmost_changed(true, true);
    app.core().frontmost_changed(false, false);
    app.wait_until("written on going to the background, with no writer left to take a word", || {
        sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a))
    });
    drop(app.quit());
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

/// Failure (the sleeper's review, 2026-11-13): a tab pushed out of the warm ones has its facts and pictures copied into the app's queue for the one writer, and
/// close retired its sleep file in the session only: the queue still held it, so its file was written after the close's retirement had been carried out, and
/// a picture and the contents of a closed document stayed in the app's folder. With every sleep file's write held at its seam: five tabs opened, so x is pushed
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
/// another page before it's left; with the cool-down at `wait` and the hour's cap at `cap`. Each showing is classed as from warm (no wake), from a picture, or bare. Synthetic pages: their files are a few KB, a real page's about
/// 72 KB, so the files written are what to read for the cap, at the real size.
fn switching(tabs: usize, cycle: usize, dwell: (u64, u64), total: u64, wait: u64, cap: u64, reading: bool) -> String {
    let disk = Disk::new();
    let docs: Vec<_> = (0..tabs).map(|at| disk.pdf(&format!("t{at}.pdf"), 6)).collect();
    let app = disk.builder().leaving_wait(wait).start();
    app.core().sleeper.set_hour_cap(cap);
    let engine = app.core().engine.clone();
    engine.waking().window_up();
    let mut view = window_view();
    for doc in &docs {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
        app.advance(dwell.0);
    }
    let ids: Vec<u64> = (0..cycle).map(|at| tab_named(&app, &format!("t{at}.pdf"))).collect();
    let (mut warm, mut picture, mut bare, mut elapsed, mut seed) = (0, 0, 0, 0, 7_u64);
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
        } else if from_a_picture(&wake) {
            picture += 1;
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
        "{tabs} tabs, {cycle} switched among, {}-{} s each for {} s; cool-down {} s, hour's cap {} B: wakes from warm {warm}, from a picture {picture}, bare \
         {bare}; sleep files written {}, bytes {}, put off {}, for the place {}; queue peak {} tabs / {} B, handed back {}, taken while being written {}, pictures dropped {}",
        dwell.0 / 1000,
        dwell.1 / 1000,
        total / 1000,
        wait / 1000,
        cap,
        sleep["sleep_files_written"],
        sleep["sleep_bytes_written"],
        sleep["writes_put_off"],
        sleep["writes_for_place"],
        leaving["peak_tabs"],
        leaving["peak_bytes"],
        leaving["handed_back"],
        leaving["stale_skipped"],
        leaving["pictures_dropped"],
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
    let mut view = window_view();
    for doc in &docs {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    let a = tab_named(&app, "a.pdf");
    assert_eq!(app.record_json()["sleep"]["leaving"]["queued"], 1, "a waits: {}", app.record_json()["sleep"]);
    (app, a, view)
}

/// Failure (the cool-down on a pushed-out tab's write, owner 2026-11-13): written at once, a tab flicked past and back wrote a sleep file each time (the drive's
/// wear), and with the wait but nothing handed back it would wake bare, as the page view let go of its pictures when they were copied. Shown again inside the
/// wait, a wakes at once from what it was copied with, drawing held, and nothing's written for it; left and pushed out again, its file is written at the wait
/// and not a millisecond before.
#[test]
fn a_tab_shown_again_inside_its_cool_down_wakes_from_memory_and_nothing_is_written() {
    let wait = crate::sleeper::LEAVING_WAIT_MS;
    let (app, a, mut view) = a_pushed_out_and_waiting(Disk::new(), wait);
    app.advance(wait / 2);
    app.settle();
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert_eq!(wake["had"], "handed_back", "from what it was copied with, said as a wake from memory: {wake}");
    assert!(wake["first_frame_us"].is_u64(), "the first frame, drawing held, isn't covered: {wake}");
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
    app.advance(wait - 1);
    app.settle();
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

/// Failure: what's handed back to a tab shown inside its wait is of the version it was copied from, so a file changed meanwhile showed the old version's
/// picture. a's file is written over inside the wait, then a's shown: its first frame is never from what was waiting.
#[test]
fn a_tab_whose_file_changed_inside_its_cool_down_is_never_shown_the_old_picture() {
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
    assert!(!from_a_picture(&wake), "the old version's picture: {wake}");
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
/// tap, move, `session sleep` wrote a new sleep file every time it asked, most of a megabyte in ten turns. It keeps to the limits anything else does: five
/// sleeps at one page inside the gap write one file; five at a new page each write one each (the file there showing nothing of the place, counted as
/// `writes_for_place`), which the hour's cap bounds: with the cap set at what's been written, the next move-and-sleep is put off and counted.
#[test]
fn session_sleep_keeps_to_the_limits_on_writing() {
    let disk = Disk::new();
    let (doc, other) = (disk.pdf("doc.pdf", 12), disk.pdf("other.pdf", 2));
    let app = disk.start();
    let engine = app.core().engine.clone();
    let mut view = window_view();
    for file in [&doc, &other] {
        app.open(file);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    let (doc, other) = (tab_named(&app, "doc.pdf"), tab_named(&app, "other.pdf"));
    let counts = || app.record_json()["sleep"].clone();
    let mut turn = |page: &str| {
        full(&app, &["tap", &format!("#{doc}")]);
        host_turns(&app, &mut view, true);
        full(&app, &["goto", page]);
        host_turns(&app, &mut view, true);
        full(&app, &["tap", &format!("#{other}")]);
        host_turns(&app, &mut view, true);
        full(&app, &["session", "sleep", "--tab", &format!("#{doc}")]);
        host_turns(&app, &mut view, true);
    };
    // Once its file's there, every sleep at the same page is put off. (The first turn's sleep can come before the page view has the tab's version, when it has
    // nothing to write: then the next turn's writes it.)
    let mut before = counts();
    for _ in 0..5 {
        turn("3");
        let now = counts();
        if before["sleep_files_written"] == 1 {
            assert_eq!(
                now["writes_put_off"].as_u64().unwrap(),
                before["writes_put_off"].as_u64().unwrap() + 1,
                "a sleep at the same page inside the gap wasn't put off: {now}"
            );
        }
        before = now;
    }
    let one_page = counts();
    assert_eq!(one_page["sleep_files_written"], 1, "one page, one file: {one_page}");
    assert!(one_page["writes_put_off"].as_u64().unwrap() >= 3, "{one_page}");
    for page in ["5", "7", "9", "11"] {
        turn(page);
    }
    let moved = counts();
    assert_eq!(moved["sleep_files_written"], 5, "one for each new place: {moved}");
    assert_eq!(moved["writes_for_place"], 4, "{moved}");
    // At the hour's cap: the next is put off, and counted.
    app.core().sleeper.set_hour_cap(moved["sleep_bytes_written"].as_u64().expect("bytes"));
    let put_off = moved["writes_put_off"].as_u64().unwrap();
    turn("2");
    let capped = counts();
    assert_eq!(capped["sleep_files_written"], 5, "past the hour's cap: {capped}");
    assert_eq!(capped["writes_put_off"].as_u64().unwrap(), put_off + 1, "{capped}");
    drop(app.quit());
}

/// Failure (the cool-down's own measurement, 2026-11-13: 77 of 363 showings bare in an hour of switching with reading on, against none before): a tab shown
/// again inside its wait, with a sleep file on disk from the time before, was woken from that file, read on the way, and not from what was handed back, so its
/// first frame was a picture of where it had been. a's file is written at the top; a's moved to page 4 and pushed out again, and shown inside the wait: its
/// first frame is of page 4.
#[test]
fn a_tab_shown_inside_its_cool_down_wakes_from_what_it_left_with_not_an_older_file() {
    let wait = crate::sleeper::LEAVING_WAIT_MS;
    let (app, a, mut view) = a_pushed_out_and_waiting(Disk::new(), wait);
    app.advance(wait);
    app.wait_until("a's first file written", || sleep_files_on_disk(&app).contains(&sleep_file_of(&app, a)));
    full(&app, &["tap", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["goto", "4"]);
    host_turns(&app, &mut view, true);
    for name in ["b.pdf", "c.pdf", "d.pdf"] {
        full(&app, &["tap", &format!("#{}", tab_named(&app, name))]);
        host_turns(&app, &mut view, true);
    }
    app.advance(wait / 2);
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert_eq!(wake["had"], "handed_back", "{wake}");
    assert!(
        wake["sleep_in_view"].as_u64().unwrap() >= 1,
        "a picture of where it was the time before: {wake}"
    );
    drop(app.quit());
}
