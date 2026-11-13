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
        .with_session(|session| session.tabs().active().and_then(|id| session.sleep_facts(id)))
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
/// changed back while it's asleep, which no write can put right: it wakes with nothing from its file and says so, never that it had a picture it didn't show.
#[test]
fn a_tab_woken_again_after_the_look_changed_shows_a_picture_at_once() {
    let disk = Disk::new();
    let (a, b) = (disk.pdf("a.pdf", 12), disk.pdf("b.pdf", 3));
    let app = disk.start();
    let engine = app.core().engine.clone();
    engine.window_up();
    let mut view = window_view();
    for doc in [&a, &b] {
        app.open(doc);
        view = window_view();
        drop(engine.set_viewport(view));
        host_turns(&app, &mut view, true);
    }
    let (a, b) = (tab_named(&app, "a.pdf"), tab_named(&app, "b.pdf"));
    let sleep = || app.record_json()["sleep"].clone();
    let mut bytes_before_dark = 0;
    for (cycle, paper) in ["white", "white", "dark"].into_iter().enumerate() {
        full(&app, &["view", "--paper", paper]);
        full(&app, &["tap", &format!("#{a}")]);
        host_turns(&app, &mut view, true);
        full(&app, &["tap", &format!("#{b}")]);
        host_turns(&app, &mut view, true);
        bytes_before_dark = sleep()["sleep_bytes_written"].as_u64().expect("bytes");
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
    let written = sleep();
    assert_eq!(written["writes_for_paper"], 1, "the change of look, once: {written}");
    assert!(
        written["writes_put_off"].as_u64().unwrap() >= 1,
        "the second white sleep is put off as before: {written}"
    );
    println!(
        "a sleep file written inside the gap for the change of look: {} bytes",
        written["sleep_bytes_written"].as_u64().unwrap() - bytes_before_dark
    );
    // Asleep with dark pictures, the look goes back to white: woken, nothing from its file is shown or held, and it says so.
    full(&app, &["tap", &format!("#{b}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["session", "sleep", "--tab", &format!("#{a}")]);
    host_turns(&app, &mut view, true);
    full(&app, &["view", "--paper", "white"]);
    host_turns(&app, &mut view, true);
    let wake = woken_at_once(&app, a, &mut view);
    assert_eq!(wake["tab"], a, "{wake}");
    assert_eq!((wake["had"].as_str(), wake["sleep_pictures"].as_u64()), (Some("nothing"), Some(0)), "{wake}");
    assert_eq!(wake["bare_with_picture"], 0, "{wake}");
    drop(app.quit());
}
