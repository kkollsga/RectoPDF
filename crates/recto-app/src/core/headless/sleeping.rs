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
