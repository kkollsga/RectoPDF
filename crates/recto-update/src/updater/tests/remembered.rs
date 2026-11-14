use super::*;

/// Failure: the user's choice of off is lost with a record, or turned back on by a launch, so an app they told not to replaces itself anyway. The switch is in this updater's own file.
#[test]
fn the_switch_is_kept_in_the_updaters_own_file() {
    let bed = Bed::new("switch-file");
    assert!(bed.updater.switch_on(), "on, by default, in this build");
    bed.updater.choose_switch(false);
    let saved = bed.dir.join("state/updates.json");
    bed.wait_for("the write", |_| Saved::read(&saved).auto_update == Some(false));
    let again = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
    again.launched(Facts {
        version: "0.1.0".into(),
        app: bed.app.clone(),
        system: "15.0".into(),
    });
    assert!(!again.switch_on(), "a launch knows it, whatever's in the record");
    // And a damaged file errs towards not replacing anything.
    local::write_private_file(&saved, b"not json").unwrap();
    let damaged = Updater::new(&bed.dir.join("state"), bed.server.source(), Arc::clone(&bed.fake) as Arc<dyn Host>);
    damaged.launched(Facts {
        version: "0.1.0".into(),
        app: bed.app.clone(),
        system: "15.0".into(),
    });
    assert!(!damaged.switch_on());
}

/// Failure (review of R3.7 S4, 2026-11-14): a switch chosen before the launch is told (held back for the record's lock, up to a minute after an update)
/// is written with a fresh file's defaults around it, over the version proven, its watch, the last update and the versions that failed; and the launch,
/// reading that, or setting what it read over the switch, loses the switch too. Nothing's written before the launch has read the file; then it holds what
/// was read and both switches, in memory and on disk.
#[test]
fn nothing_is_written_before_the_launch_has_read_what_is_remembered() {
    let dir = temp_dir("u-early-switch");
    let root = dir.join("state");
    local::ensure_private_dir(&root).unwrap();
    let install = dir.join("install");
    std::fs::create_dir_all(&install).unwrap();
    let app = fake_app(&install, "0.1.0");
    let server = Server::start();
    let files = Files::in_state(&root);
    let remembered = Saved {
        failed: vec!["0.1.5".into()],
        proven: Some("0.1.0".into()),
        probation: Some(Probation {
            version: "0.1.0".into(),
            since_ms: crate::wall_ms(),
            running_since_ms: None,
            early_ends: 1,
        }),
        last_update: Some(LastUpdate {
            from: "0.0.9".into(),
            to: "0.1.0".into(),
            gap_ms: 1_500,
            at_ms: crate::wall_ms(),
        }),
        auto_update: Some(true),
        ..Saved::default()
    };
    remembered.write(&files.saved);
    let was = std::fs::read(&files.saved).unwrap();
    let updater = Updater::new(&root, server.source(), Arc::new(Fake::default()) as Arc<dyn Host>);
    updater.choose_switch(false);
    updater.choose_dev_builds(true);
    updater.save();
    assert_eq!(std::fs::read(&files.saved).unwrap(), was, "nothing written before the launch has read it");
    assert!(!updater.switch_on() && updater.dev_builds_on(), "the switches hold at once");
    updater.launched(Facts {
        version: "0.1.0".into(),
        app,
        system: "15.0".into(),
    });
    let on_disk = Saved::read(&files.saved);
    for saved in [lock(&updater.st).saved.clone(), on_disk] {
        assert_eq!(
            (saved.auto_update, saved.dev_builds),
            (Some(false), Some(true)),
            "the switches as chosen: {saved:?}"
        );
        assert_eq!(saved.proven.as_deref(), Some("0.1.0"), "{saved:?}");
        assert_eq!(saved.probation.as_ref().map(|probation| probation.early_ends), Some(1), "{saved:?}");
        assert!(saved.last_update.is_some(), "{saved:?}");
        assert_eq!(saved.failed, ["0.1.5"], "{saved:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
