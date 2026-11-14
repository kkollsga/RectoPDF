//! **The updater's own file, `updates.json`, and a switch chosen before the updater has read it** (review of R3.7 S4, 2026-11-14): until the updater's told of
//! the launch it hasn't read what's remembered, so a write then would put a fresh file's defaults over what an update proved, what it's watching and what failed.
//! A switch chosen then holds at once and is kept, and goes to disk with what was read, once it has been.

use super::late_lock::{next_try_fails, the_copy_before};
use crate::core::controls::UiAction;
use crate::core::updates::{AUTO_UPDATE_ROW, DEV_BUILDS_ROW};
use crate::lifecycle::keeping::{Keeping, TRY_EVERY_MS};
use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{OLD, UpdateBed};
use recto_platform::slow::Seam;
use recto_update::saved::{LastUpdate, Probation, Saved};

/// A version that failed to install, which is never tried again by itself while it's remembered.
const FAILED: &str = "0.1.5";

/// What the updater remembers before the launch: this version proven by an update a minute ago and watched for early ends (one so far), the update itself, a
/// version that failed, a check, and *Update automatically* turned off by the person.
fn remembered() -> Saved {
    let minute_ago = recto_update::wall_ms().saturating_sub(60_000);
    Saved {
        last_check_ms: Some(minute_ago),
        failed: vec![FAILED.to_owned()],
        proven: Some(OLD.to_owned()),
        probation: Some(Probation {
            version: OLD.to_owned(),
            since_ms: minute_ago,
            running_since_ms: None,
            early_ends: 1,
        }),
        last_update: Some(LastUpdate {
            from: "0.0.9".to_owned(),
            to: OLD.to_owned(),
            gap_ms: 1_500,
            at_ms: minute_ago,
        }),
        auto_update: Some(false),
        ..Saved::default()
    }
}

/// A disk with this version installed and [`remembered`] in its `updates.json`, and that file's bytes.
fn installed_remembering() -> (Disk, Vec<u8>) {
    let disk = Disk::new();
    disk.install_app(OLD);
    let file = disk.update_files().saved;
    recto_platform::local::ensure_private_dir(file.parent().expect("the update folder")).expect("the update folder");
    remembered().write(&file);
    let bytes = std::fs::read(&file).expect("updates.json written");
    (disk, bytes)
}

/// The person's choice in Settings, as the window sends it.
fn person_chooses(app: &HeadlessApp, id: &str, on: bool) {
    let acted = app.act(UiAction::SettingChosen { id: id.to_owned(), on });
    assert!(acted.understood, "{id} is a switch");
    app.settle();
}

/// What was remembered is all still there, on disk and in the updater, with the person's switch on top: the two switches as they should now be.
fn kept_with(app: &HeadlessApp, dev_builds: Option<bool>, auto_update: Option<bool>) {
    let file = app.disk().update_files().saved;
    app.wait_until("the launch told and what it read written", || {
        let saved = Saved::read(&file);
        saved.dev_builds == dev_builds && saved.auto_update == auto_update
    });
    let saved = Saved::read(&file);
    assert_eq!(saved.proven.as_deref(), Some(OLD), "the version proven: {saved:?}");
    let probation = saved.probation.as_ref().expect("its watch for early ends");
    assert_eq!((probation.version.as_str(), probation.early_ends), (OLD, 1), "{saved:?}");
    assert_eq!(saved.last_update.as_ref().map(|update| update.to.as_str()), Some(OLD), "{saved:?}");
    assert_eq!(saved.failed, [FAILED], "{saved:?}");
    assert!(saved.last_check_ms.is_some(), "{saved:?}");
    let status = app.update_status();
    assert_eq!(status["failed_versions"], serde_json::json!([FAILED]), "{status}");
    assert_eq!(status["last_update"]["to"], OLD, "{status}");
    assert_eq!(status["auto_update"], auto_update.unwrap_or(true), "{status}");
    assert_eq!(status["dev_builds"], dev_builds.unwrap_or(false), "{status}");
}

/// Failure (review of R3.7 S4, 2026-11-14): while the launch is held back for the record's lock (up to a minute after an update's hand-off), the person flips
/// a switch in Settings, which is accepted, and the updater writes it to `updates.json` with a fresh file's defaults around it, as it hadn't read the file yet:
/// the version proven, its watch for early ends, the last update and the versions that failed are lost, and *Update automatically*, which the person had turned
/// off, is on again; the launch, told once the lock comes, then reads that. While held nothing is written, the switch shows as chosen; once the copy keeps the
/// record and the updater's told, the file and the updater hold what was remembered and the person's switch. A copy that ends before it's told writes nothing.
#[test]
fn a_switch_chosen_while_the_launch_is_held_writes_nothing_over_what_the_updater_remembers() {
    let bed = UpdateBed::new();

    // Ended while still held: nothing of the updater's is written, the switch included.
    let (disk, was) = installed_remembering();
    let copy = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    next_try_fails(&app);
    person_chooses(&app, DEV_BUILDS_ROW, true);
    assert!(app.core().updates.dev_builds_on(), "the switch shows as the person set it");
    let disk = app.kill();
    assert!(
        std::fs::read(disk.update_files().saved).ok() == Some(was),
        "updates.json as it was: {:?}",
        Saved::read(&disk.update_files().saved)
    );
    drop(copy);

    // Held, then kept: what was remembered, and the person's switch on top.
    let (disk, _) = installed_remembering();
    let copy = the_copy_before(&disk);
    let app = disk.builder().update(&bed).another_keeps().start();
    next_try_fails(&app);
    person_chooses(&app, DEV_BUILDS_ROW, true);
    drop(copy);
    app.advance(TRY_EVERY_MS);
    app.wait_until("the record kept", || app.core().life.keeping() == Keeping::Keeps);
    app.settle();
    kept_with(&app, Some(true), Some(false));
}

/// Failure (the same, on any launch): a switch chosen in the moment before the updater's read its file at the launch (an agent's `settings` as the app comes up,
/// or a starved machine's late restore thread) writes a fresh file's defaults and the switch over it, and the launch reads that. The launch held at the
/// updater's look, before it reads: the person turns *Update automatically* on; once the look goes on, the file and the updater hold what was remembered and
/// the switch.
#[test]
fn a_switch_chosen_before_the_launch_is_read_is_kept_on_top_of_what_was_remembered() {
    let bed = UpdateBed::new();
    let (disk, _) = installed_remembering();
    let app = disk.builder().update(&bed).hold(&[Seam::Launch, Seam::State]).start();
    person_chooses(&app, AUTO_UPDATE_ROW, true);
    assert!(app.core().updates.switch_on(), "the switch shows as the person set it");
    app.let_through(Seam::Launch, 1);
    app.wait_until("the state's read to wait at its seam", || app.waiting(Seam::State) > 0);
    app.let_state_in();
    kept_with(&app, None, Some(true));
}
