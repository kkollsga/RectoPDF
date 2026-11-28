use super::*;

/// A new version `0.2.0` that has ended too often, with a kept `0.1.0` (that reads the record or not) and a record there or not; says what the last launch
/// did.
fn after_too_many_early_ends(marked: bool, record_there: bool) -> (Bed, Files, Arc<Updater>, AtLaunch) {
    let bed = Bed::new("back-removal");
    let root = bed.dir.join("state");
    let files = Files::in_state(&root);
    let beside = beside_app(&bed.app).unwrap();
    let kept = fake_app_at(&previous_app_in(&beside), "0.1.0");
    if marked {
        let added = std::process::Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", &format!("Add :{} string 1", crate::removal_record::GATE_KEY)])
            .arg(kept.join("Contents/Info.plist"))
            .status()
            .unwrap();
        assert!(added.success());
    }
    if record_there {
        std::fs::create_dir_all(root.join("removal")).unwrap();
        std::fs::write(root.join("removal").join("pending.v1"), b"anything at the name").unwrap();
    }
    local::ensure_private_dir(&files.work).unwrap();
    Plan {
        id: "1-1".into(),
        cdhash: "a".repeat(40),
        team_required: false,
        any_version: false,
        development_build: false,
        team_signed: false,
        from: "0.1.0".into(),
        to: "0.2.0".into(),
        pid: 1,
        app: bed.app.clone(),
        beside: beside.clone(),
        root: root.clone(),
        quit_at_ms: 5,
        background: false,
        quit_limit_ms: 1,
        start_limit_ms: 1,
        handed_off_tabs: None,
        open: Vec::new(),
        keep_record: false,
        test: None,
    }
    .write(&files.plan())
    .unwrap();
    let launch = || {
        let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        let at = updater.launched(Facts {
            version: "0.2.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        updater.first_frame(updater.now_ms());
        (updater, at)
    };
    for _ in 0..EARLY_ENDS_LIMIT {
        launch();
    }
    let (updater, at) = launch();
    (bed, files, updater, at)
}

/// Failure (D3b): a new version that keeps ending is gone back from while an unfinished removal's record is there, to a kept version that doesn't read it. That
/// version would restore everything at its start, with the data the removal is purging, and an installer started for it would only refuse, quitting and
/// reopening this one again and again. With a kept version that reads the record, it goes back as before.
#[test]
fn going_back_with_a_removal_pending_starts_only_for_a_build_that_reads_it() {
    for (marked, record_there) in [(false, true), (true, true), (false, false)] {
        let (bed, files, _updater, at) = after_too_many_early_ends(marked, record_there);
        let started = lock(&bed.fake.installers).clone();
        let goes_back = !record_there || marked;
        assert_eq!(
            started.iter().filter(|(mode, _)| *mode == InstallerMode::Back).count(),
            usize::from(goes_back),
            "marked {marked}, record there {record_there}: {started:?}"
        );
        assert_eq!(at == AtLaunch::Exit, goes_back, "marked {marked}, record there {record_there}");
        assert_eq!(
            files.plan().exists(),
            goes_back,
            "no plan left for an installer that was never started (marked {marked}, record {record_there})"
        );
    }
}

/// Failure (D3b's gates): a going back that is refused for a record only logs, so the failing version stays and nobody is told why it isn't put back. The
/// person is told, in words that name both versions and the unfinished removal, and that it stays; with a record that the kept version reads (it goes back)
/// or no record, nothing is said.
#[test]
fn a_going_back_refused_for_a_record_says_why() {
    let (_bed, _files, updater, _) = after_too_many_early_ends(false, true);
    let said = updater.notice().expect("the person is told").text;
    for part in ["0.2.0", "0.1.0", "unfinished removal", "stays"] {
        assert!(said.contains(part), "{part:?} in {said:?}");
    }
    for (marked, record_there) in [(true, true), (false, false)] {
        let (_bed, _files, updater, _) = after_too_many_early_ends(marked, record_there);
        assert!(updater.notice().is_none(), "marked {marked}, record {record_there}: {:?}", updater.notice());
    }
}

/// Failure (D3b's gates): the update's week-old copy of the record (`update/before`) is dropped while an unfinished removal's record is there, which takes
/// away what the removal's plan pinned. Past its week it stays with a record there, and goes with none.
#[test]
fn the_updates_copy_stays_past_its_week_while_a_record_is_there() {
    for record_there in [true, false] {
        let bed = Bed::new("week-removal");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();
        if record_there {
            std::fs::create_dir_all(root.join("removal")).unwrap();
            std::fs::write(root.join("removal").join("pending.v1"), b"anything at the name").unwrap();
        }
        std::fs::create_dir_all(files.before()).unwrap();
        std::fs::write(files.before().join("record.a.json"), "the state before").unwrap();
        let aged = std::time::SystemTime::now() - Duration::from_millis(KEEP_RECORD_COPY_MS + 60_000);
        std::fs::File::create(files.before().join(".whole")).unwrap().set_modified(aged).unwrap();
        let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        updater.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert_eq!(files.before().exists(), record_there, "record there {record_there}");
    }
}

/// Failure (D3b's gates): an install chosen by hand quits the app for an installer that will refuse for the record (the person's window closes and reopens
/// for nothing). With a record there it is never handed off, never quit and no installer is started, and the person is told why; with none it goes.
#[test]
fn an_install_with_a_record_there_never_hands_off() {
    let bed = Bed::new("hand-removal");
    bed.publish("0.2.0");
    bed.found("0.2.0");
    std::fs::create_dir_all(bed.dir.join("state").join("removal")).unwrap();
    std::fs::write(bed.dir.join("state").join("removal").join("pending.v1"), b"anything at the name").unwrap();
    assert_eq!(bed.updater.install(Asked::ByHand), Ok("0.2.0".to_owned()));
    bed.wait_for("the failure", |bed| matches!(bed.status(), Status::Failed { .. }));
    assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 0, "no hand-off");
    assert_eq!(bed.fake.quits.load(Ordering::SeqCst), 0, "no quit");
    assert!(lock(&bed.fake.installers).is_empty(), "no installer");
    let said = bed.updater.notice().expect("the person is told").text;
    assert!(said.contains("left unfinished") && !said.contains("is finished first"), "{said}");
}

/// Failure (D3b): the launch that settles an install the installer refused (for an unfinished removal, or for any other reason) clears `update/before`, the
/// older copy of the session that a going back may still need, while a removal's record is there; or, with none there, keeps what it always cleared. And
/// the person is told in the words, exactly.
#[test]
fn settling_a_refusal_keeps_the_copy_while_a_removal_is_pending_and_says_why() {
    let notice_of = |kind: OutcomeKind, why: &str, record: bool| {
        let bed = Bed::new("settle-removal");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();
        if record {
            std::fs::create_dir_all(root.join("removal")).unwrap();
            std::fs::write(root.join("removal").join("pending.v1"), b"anything").unwrap();
        }
        std::fs::create_dir_all(files.before()).unwrap();
        std::fs::write(files.before().join(".whole"), "").unwrap();
        Outcome::new(kind, "0.1.0", "0.2.0", why).write(&files.outcome()).unwrap();
        let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        updater.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        (files.before().join(".whole").exists(), updater.notice().expect("said").text)
    };
    let (kept, said) = notice_of(OutcomeKind::NotInstalled, "removal_pending", true);
    assert!(kept);
    assert!(said.contains("left unfinished") && !said.contains("is finished first"), "{said}");
    let (kept, said) = notice_of(OutcomeKind::CouldntGoBack, "removal_pending", true);
    assert!(kept);
    assert_eq!(
        said,
        "RectoPDF 0.2.0 is what's installed, and a removal of data from a document was left unfinished, so the earlier version was not put back."
    );
    let (kept, _) = notice_of(OutcomeKind::NotInstalled, "swap_failed", true);
    assert!(kept, "another reason, a record there: the copy stays");
    let (kept, _) = notice_of(OutcomeKind::NotInstalled, "swap_failed", false);
    assert!(!kept, "another reason, no record: cleared as it always was");
}

/// Failure (the gates' review): a record that comes in between the look before the hand-off and the one after it (nothing looked again past the hand-off)
/// has the app quit for an installer that will refuse. Handed off with the record arriving as it freezes: no plan, no installer, no quit, the app given back
/// and the update failed with the installer's own words.
#[test]
fn a_record_arriving_at_the_hand_off_stops_the_quit() {
    let bed = Bed::new("hand-record-late");
    bed.publish("0.2.0");
    bed.found("0.2.0");
    lock(&bed.fake.hand_off_is).record_comes_in = Some(bed.dir.join("state").join("removal").join("pending.v1"));
    assert_eq!(bed.updater.install(Asked::ByHand), Ok("0.2.0".to_owned()));
    bed.wait_for("the failure", |bed| matches!(bed.status(), Status::Failed { .. }));
    assert_eq!(bed.fake.hand_offs.load(Ordering::SeqCst), 1);
    assert_eq!(bed.fake.cancels.load(Ordering::SeqCst), 1, "given back");
    assert_eq!(bed.fake.quits.load(Ordering::SeqCst), 0, "no quit");
    assert!(lock(&bed.fake.installers).is_empty(), "no installer");
    assert!(!Files::in_state(&bed.dir.join("state")).plan().exists(), "no plan left for an installer");
}

/// Failure (the gates' review): the notice that a going back was refused is wiped by the next thing an install attempt does when it doesn't go ahead (a
/// failure, or going back on offer), so the person is told for a moment and never again. It stays through both, and goes only when they've read it.
#[test]
fn the_refused_going_back_notice_stays_through_a_later_attempt() {
    let (_bed, _files, updater, _) = after_too_many_early_ends(false, true);
    let said = updater.notice().expect("told").text;
    updater.back_on_offer("0.3.0", None);
    assert_eq!(updater.notice().map(|notice| notice.text), Some(said.clone()), "after going back on offer");
    updater.failed(Asked::ByItself, "0.3.0", "it couldn't be put in place", false);
    assert_eq!(updater.notice().map(|notice| notice.text), Some(said), "after a failure");
    updater.dismiss_notice();
    assert_eq!(updater.notice(), None, "read");
}
