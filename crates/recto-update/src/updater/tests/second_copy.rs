use super::*;

/// Failure (third read, 2026-11-13): two copies of the new version an update opened (`open -n`): the one that keeps the record tells the installer it came up
/// with the tabs it restored, and the second copy, coming up a moment later, writes its launch-ok without a tab count over it, before the installer looks:
/// the installer's look at whether the new version misread the record is lost. A second copy leaves a launch-ok already there for this version as it is,
/// and writes its own over one for another version (an earlier update's).
#[test]
fn a_second_copy_leaves_the_first_copys_launch_ok_for_this_version() {
    for (there, kept) in [("0.2.0 7 5 tabs=4", true), ("0.1.5 7 5 tabs=4", false)] {
        let bed = Bed::new("second-after-first");
        let root = bed.dir.join("state");
        let files = Files::in_state(&root);
        local::ensure_private_dir(&files.work).unwrap();
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
            handed_off_tabs: Some(4),
            open: Vec::new(),
            keep_record: false,
            test: None,
        };
        plan.write(&files.plan()).unwrap();
        local::write_private_file(&files.launch_ok(), there.as_bytes()).unwrap();
        bed.fake.second_copy.store(true, Ordering::SeqCst);
        let second = Updater::new(&root, bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
        second.launched(Facts {
            version: "0.2.0".into(),
            app: bed.app.clone(),
            system: "15.0".into(),
        });
        second.first_frame(10);
        let said = std::fs::read_to_string(files.launch_ok()).unwrap();
        if kept {
            assert_eq!(said, there, "the first copy's, with its tabs");
        } else {
            assert_eq!(crate::plan::read_launch_ok(&said), Some(("0.2.0", None)), "{said}");
        }
    }
}
