use super::*;

/// Failure (D3b): a new version that keeps ending is gone back from while an unfinished removal's record is there, to a kept version that doesn't read it. That
/// version would restore everything at its start, with the data the removal is purging, and an installer started for it would only refuse, quitting and
/// reopening this one again and again. With a kept version that reads the record, it goes back as before.
#[test]
fn going_back_with_a_removal_pending_starts_only_for_a_build_that_reads_it() {
    for (marked, record_there) in [(false, true), (true, true), (false, false)] {
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
        let arrive = || {
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
        };
        let launch = || {
            let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
            let at = updater.launched(Facts {
                version: "0.2.0".into(),
                app: bed.app.clone(),
                system: "15.0".into(),
            });
            updater.first_frame(updater.now_ms());
            at
        };
        arrive();
        for _ in 0..EARLY_ENDS_LIMIT {
            launch();
        }
        let at = launch();
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
    assert_eq!(
        said,
        "Couldn't update to 0.2.0: a removal of data from a document was left unfinished, and is finished first. RectoPDF is unchanged."
    );
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
