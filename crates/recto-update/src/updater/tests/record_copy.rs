//! **How long the update's copy of the session record is kept:** a week by the age of `.whole`, as PRIVACY.md says. Moved out of `updater.rs` for its size.

use super::*;

/// Failure: a new version that comes up and then misreads the record, starting empty or wrong, has already thrown away the record from before it, which was deleted at its first frame. It's kept for a week, and then it goes.
#[test]
fn the_record_from_before_an_update_is_kept_for_a_while() {
    let bed = Bed::new("before");
    let root = bed.dir.join("state");
    let files = Files::in_state(&root);
    local::ensure_private_dir(&files.work).unwrap();
    std::fs::create_dir_all(files.before()).unwrap();
    std::fs::write(files.before().join(".whole"), "").unwrap();
    std::fs::write(files.before().join("record.a.json"), "the state before").unwrap();
    let plan = Plan {
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
        beside: beside_app(&bed.app).unwrap(),
        root: root.clone(),
        quit_at_ms: 5,
        background: false,
        quit_limit_ms: 1,
        start_limit_ms: 1,
        handed_off_tabs: None,
        open: Vec::new(),
        keep_record: false,
        test: None,
    };
    plan.write(&files.plan()).unwrap();
    let launch = |version: &str| {
        let updater = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        updater.launched(Facts {
            version: version.into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        updater
    };
    launch("0.2.0").first_frame(100);
    assert!(files.before().join("record.a.json").exists(), "come up, and kept");
    launch("0.2.0").first_frame(100);
    assert!(files.before().join("record.a.json").exists(), "and at the next launch");
    let week_and_more = std::time::SystemTime::now() - Duration::from_millis(KEEP_RECORD_COPY_MS + 60_000);
    std::fs::File::options()
        .write(true)
        .open(files.before().join(".whole"))
        .unwrap()
        .set_modified(week_and_more)
        .unwrap();
    launch("0.2.0");
    assert!(!files.before().exists(), "a week on, it goes");
}

/// A clock that stands still, so a file's age is exact.
struct StandingClock(u64);
impl WallClock for StandingClock {
    fn wall_ms(&self) -> u64 {
        self.0
    }
}

/// Failure: while a rollback refusal stands (`app_exit_unproven`, its outcome and plan kept), the record copy in `update/before/` was never dropped, so PRIVACY.md's "a week" held for nothing: the edit history it holds stayed for good. A week by the age of `.whole` it goes, refusal or not, and the outcome and plan stay; at the week itself and a second short of it, it's kept.
#[test]
fn the_record_copy_goes_after_its_week_while_a_refusal_stands() {
    const NOW_MS: u64 = 4_000_000_000_000;
    for (age_ms, dropped) in [
        (KEEP_RECORD_COPY_MS + 1_000, true),
        (KEEP_RECORD_COPY_MS, false),
        (KEEP_RECORD_COPY_MS - 1_000, false),
    ] {
        let bed = Bed::new("week-refusal");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();
        std::fs::create_dir_all(files.before()).unwrap();
        std::fs::write(files.before().join("record.a.json"), "the state before").unwrap();
        let whole = std::fs::File::create(files.before().join(".whole")).unwrap();
        whole.set_modified(std::time::UNIX_EPOCH + Duration::from_millis(NOW_MS - age_ms)).unwrap();
        let plan = Plan {
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
            beside: beside_app(&bed.app).unwrap(),
            root: root.clone(),
            quit_at_ms: 5,
            background: false,
            quit_limit_ms: 1,
            start_limit_ms: 1,
            handed_off_tabs: None,
            open: Vec::new(),
            keep_record: false,
            test: None,
        };
        plan.write(&files.plan()).unwrap();
        Outcome::new(OutcomeKind::CouldntGoBack, "0.2.0", "0.1.0", "app_exit_unproven")
            .write(&files.outcome())
            .unwrap();
        let seams = Seams {
            wall: Arc::new(StandingClock(NOW_MS)),
            ..Seams::default()
        };
        let updater = Updater::with_seams(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>, seams);
        updater.launched(Facts {
            version: "0.1.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        assert_eq!(!files.before().exists(), dropped, "a copy {age_ms} ms old");
        assert!(
            files.outcome().exists() && files.plan().exists(),
            "the refusal and its plan stay, at {age_ms} ms"
        );
    }
}
