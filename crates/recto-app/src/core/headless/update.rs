//! **Updates, on the app's own core** (step R1.4): the updater joined to the app as the window has it, its look at what's in flight, the hand-off's freeze, write and
//! read back, and the launch that comes up after it, with the release server and the installer stood in for by [`crate::testkit::update_bed`]. Each test names the
//! app-launching test in `recto-cli`'s `restore::update` it stands in for. What only the real thing has (the zip, codesign, the installer process and the swap on a
//! real volume, Launch Services, the window) stays there.

use crate::testkit::Heard;
use crate::testkit::headless::{Disk, HeadlessApp};
use crate::testkit::update_bed::{DEV, Installed, NEW, OLD, UpdateBed};
use recto_agent::Level;
use recto_platform::slow::Seam;
use serde_json::Value;

/// An installed copy of [`OLD`] on a fresh disk, with one document open at a page, and the bed it updates from.
fn running_with_a_document(page: u32) -> (HeadlessApp, UpdateBed) {
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let file = disk.pdf("alpha.pdf", 20);
    let app = disk.builder().update(&bed).start();
    app.open(&file);
    app.agent(Level::Control, &format!(r#"{{"cmd":"goto","page":{page}}}"#)).expect("goto");
    app.settle();
    (app, bed)
}

/// An agent's command at Full, which an update's need, and its answer.
fn full(app: &HeadlessApp, line: &str) -> Value {
    app.agent(Level::Full, line).unwrap_or_else(|error| panic!("{line}: {error:?}"))
}

/// The page the first tab is on, counted from 0, as the record says.
fn page(app: &HeadlessApp) -> Value {
    app.session_json()["tabs"][0]["page"].clone()
}

/// Failure: an update by hand doesn't hand off and come back as it was: the plan isn't written for the installer, the app isn't told to quit, or the launch on
/// the new version doesn't find the session it left, say it updated, or keep the old version. The whole path through the app's core, on stand-ins for the server
/// and the installer. (What `an_update_in_the_middle_of_things_changes_nothing` holds on the real app, with the real swap, stays there.)
#[test]
fn an_update_by_hand_hands_off_and_comes_back_as_it_was() {
    let (app, bed) = running_with_a_document(9);
    let before = page(&app);
    bed.releases.offer(NEW);
    assert_eq!(full(&app, r#"{"cmd":"update_check"}"#)["status"], "available");
    assert_eq!(full(&app, r#"{"cmd":"update_install"}"#)["started"], true);
    let (plan, disk) = app.handed_off(&bed);
    assert_eq!((plan.from.as_str(), plan.to.as_str()), (OLD, NEW), "{plan:?}");
    assert_eq!(plan.handed_off_tabs, Some(1), "the tabs as written and read back");
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.disk().app_version().as_deref(), Some(NEW));
    assert_eq!(app.disk().kept_version().as_deref(), Some(OLD));
    assert_eq!(app.update_status()["status"], "updated", "{}", app.update_status());
    assert_eq!(app.update_status()["version"], NEW);
    assert_eq!(page(&app), before, "the session as it was");
}

/// Whether the app's taking what's done to it: not frozen by a hand-off, and an agent's command and a person's action both go through.
fn still_usable(app: &HeadlessApp) {
    assert!(!app.core().core.frozen.load(std::sync::atomic::Ordering::SeqCst), "the app's frozen");
    assert!(!app.core().host.handing_off.load(std::sync::atomic::Ordering::SeqCst), "commands are held back");
    app.agent(Level::Control, r#"{"cmd":"goto","page":2}"#).expect("an agent's command goes");
    app.settle();
    assert_eq!(page(app), 1, "and did what it said");
    assert_eq!(app.listener().count(&crate::testkit::Heard::HandOffQuit), 0, "never told to quit for an update");
}

/// Agent access Full, as an update by an agent needs.
fn at_full(app: &HeadlessApp) {
    recto_control::levels::store(app.disk().paths(), Level::Full).expect("the level's file");
}

/// Failure (owner, 2026-11-11: "for dev work, lets set the limit to 5 mins"): a development build goes in while the owner is using the app, or never goes. 5
/// minutes without use, by the app's own idle time on its clock, unless `--now`; then it goes by the same hand-off, and comes back as it was. (Replaces the app
/// test `a_development_build_waits_for_five_quiet_minutes`: a build of another app is refused in `recto-update`'s `what_isnt_this_apps_build_is_refused`.)
#[test]
fn a_development_build_waits_for_five_quiet_minutes() {
    let (app, bed) = running_with_a_document(7);
    at_full(&app);
    assert_eq!(full(&app, r#"{"cmd":"update_dev_builds","set":"on"}"#)["dev_builds"], true);
    let zip = app.disk().hand_over_build(DEV);
    app.touch();
    let answer = full(&app, &format!(r#"{{"cmd":"update_install","zip":{}}}"#, serde_json::json!(zip)));
    let waits = answer["waits_ms"].as_u64().expect("it says how long it waits");
    assert!(waits > 4 * 60_000 && waits <= 5 * 60_000, "5 minutes, not a release's 30: {answer}");
    assert_eq!(app.update_status()["status"], "build_ready");
    // Four minutes on, and they're back.
    app.advance(4 * 60_000);
    app.touch();
    app.update_tick();
    assert_eq!(app.update_status()["status"], "build_ready", "{}", app.update_status());
    assert!(bed.installer.plans().is_empty(), "not while it's in use");
    // Then five minutes with nobody there.
    app.advance(5 * 60_000 + 20_000);
    app.update_tick();
    let (plan, disk) = app.handed_off(&bed);
    assert_eq!(plan.to, DEV);
    assert!(plan.development_build, "{plan:?}");
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.disk().app_version().as_deref(), Some(DEV));
    assert_eq!(page(&app), 6, "page 7, counted from 0, as it was");
}

/// Failure: the 30-minute rule is broken: it installs while the user's in it, or never does, or a background app's update says it comes back in front. By the
/// app's idle time on its clock and the timer's own call. (Replaces the app test `an_automatic_update_waits_for_thirty_quiet_minutes`; that the installer reopens
/// a background app without taking the focus is `recto-update`'s `the_app_comes_back_as_it_was_frontmost_or_not`.)
#[test]
fn an_automatic_update_waits_for_thirty_quiet_minutes() {
    let (app, bed) = running_with_a_document(11);
    app.core().window.frontmost_changed(false, false, &app.core().saver, &app.core().core);
    assert_eq!(
        full(&app, r#"{"cmd":"settings"}"#)["auto_update"],
        true,
        "on by default in anything that behaves as a release"
    );
    bed.releases.offer(NEW);
    app.touch();
    assert_eq!(full(&app, r#"{"cmd":"update_check"}"#)["status"], "available");
    app.update_tick();
    let status = app.update_status();
    assert_eq!(status["status"], "available", "just touched: nothing happens");
    let wait = status["install_look_in_ms"].as_u64().expect("it looks again");
    assert!(wait > 25 * 60_000 && wait <= 30 * 60_000, "and it looks again when it could be time: {status}");
    // Twenty minutes on, and they're back.
    app.advance(20 * 60_000);
    app.touch();
    app.update_tick();
    assert_eq!(app.update_status()["status"], "available");
    assert!(bed.installer.plans().is_empty(), "not while it's in use");
    // Half an hour and more with nobody there.
    app.advance(30 * 60_000 + 1_000);
    app.update_tick();
    let (plan, disk) = app.handed_off(&bed);
    assert!(plan.background, "it wasn't frontmost, so it comes back behind: {plan:?}");
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.disk().app_version().as_deref(), Some(NEW));
    assert_eq!(page(&app), 10);
}

/// Failure: an update goes over an open sheet or a running command, or doesn't say what it's waiting for, or never goes when that's done. A busy app waits,
/// says so, is still in use, and then goes, by the same calls as the sheet opening and closing. (Replaces the app test `a_busy_app_waits_and_then_goes`.)
#[test]
fn a_busy_app_waits_and_then_goes() {
    let (app, bed) = running_with_a_document(7);
    app.core().sheet_changed(true);
    bed.releases.offer(NEW);
    assert_eq!(full(&app, r#"{"cmd":"update_check"}"#)["status"], "available");
    full(&app, r#"{"cmd":"update_install"}"#);
    let status = app.wait_for_status("waiting");
    assert!(status["words"].as_str().unwrap_or_default().contains("sheet"), "{status}");
    assert_eq!(status["menu"]["enabled"], false);
    let notice = app.core().updates.notice().expect("the user is told it's waiting");
    assert!(notice.text.contains("as soon as this is done"), "{notice:?}");
    // Waiting isn't frozen: it's still in use.
    app.agent(Level::Control, r#"{"cmd":"goto","page":8}"#).expect("still taking commands");
    app.settle();
    assert!(bed.installer.plans().is_empty() && app.listener().count(&crate::testkit::Heard::HandOffQuit) == 0);
    app.core().sheet_changed(false);
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).start();
    assert_eq!(page(&app), 7, "page 8, counted from 0, as it was when it went");
}

/// Failure: an installer killed partway leaves half an app, or loses the state. Killed right after the swap: the new version's in place and not running.
/// Starting it finds all of it, says it updated, and tidies what the installer left: the old app kept, and the plan gone. (Replaces the app test
/// `an_installer_killed_after_the_swap_loses_nothing`; the installer killed at every step is `recto-update`'s `killed_at_any_step_it_leaves_a_whole_app_and_the_whole_state`.)
#[test]
fn an_installer_killed_after_the_swap_loses_nothing() {
    let (app, bed) = running_with_a_document(9);
    app.agent(Level::Control, r#"{"cmd":"page_rotate","pages":"2","degrees":90}"#).expect("an edit");
    // What the journal's called is settled once something's asked to be written.
    full(&app, r#"{"cmd":"session_save"}"#);
    app.settle();
    let before = app.session_json();
    bed.releases.offer(NEW);
    full(&app, r#"{"cmd":"update_check"}"#);
    full(&app, r#"{"cmd":"update_install"}"#);
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::KilledAfterSwap);
    assert!(disk.update_files().plan().exists(), "what it left");
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.update_status()["status"], "updated");
    assert_eq!(
        app.disk().kept_version().as_deref(),
        Some(OLD),
        "the launch kept the old app, as the installer didn't get to"
    );
    assert!(!app.disk().update_files().plan().exists());
    assert_eq!(app.session_json()["tabs"], before["tabs"], "the tabs and their edits as they were");
}

/// Failure: a bad release gets anywhere near the app quitting, or a refused update leaves it frozen. Each way a release can be wrong: a description that can't
/// be read, a downgrade, a download that isn't the file described, a damaged zip. The app goes on as it was throughout, and the old app's still what's installed.
/// (Replaces the app test `a_bad_release_never_touches_the_running_app`; each refusal itself is `recto-update`'s `stage.rs` tests.)
#[test]
fn a_bad_release_never_touches_the_running_app() {
    let (app, bed) = running_with_a_document(5);
    bed.releases
        .answer(Err(recto_update::fetch::FetchError::Failed("the release's description can't be read")));
    let status = full(&app, r#"{"cmd":"update_check"}"#);
    assert_eq!(status["status"], "check_failed", "{status}");
    assert_eq!(status["menu"]["title"], "Check for Updates…");
    bed.releases.offer("0.0.9");
    assert_eq!(full(&app, r#"{"cmd":"update_check"}"#)["status"], "up_to_date", "a downgrade's no offer");
    let refused = app.agent(Level::Full, r#"{"cmd":"update_install"}"#).expect_err("nothing to install");
    assert_eq!(refused.code, "nothing_on_offer");
    for (code, why) in [
        ("checksum", "the download isn't the file the release describes"),
        ("unpack_failed", "the release couldn't be unpacked"),
    ] {
        bed.releases.offer(NEW);
        bed.releases.refuse(code, why, true);
        assert_eq!(full(&app, r#"{"cmd":"update_check"}"#)["status"], "available");
        full(&app, r#"{"cmd":"update_install"}"#);
        let status = app.wait_for_status("failed");
        assert!(status["words"].as_str().unwrap_or_default().contains(why), "{status}");
        still_usable(&app);
        assert!(bed.installer.plans().is_empty(), "no installer started");
    }
    assert_eq!(app.disk().app_version().as_deref(), Some(OLD));
    assert_eq!(app.disk().kept_version(), None);
}

/// Failure: a release that isn't to be installed, as one an ad-hoc build is under the real rule, is tried again by itself, or leaves the app anything but as it
/// was. Refused for good: the version's remembered as failed, and nothing's installed. (Replaces the app test `the_real_rule_refuses_an_ad_hoc_build`; the rule
/// itself, on a real ad-hoc signature, is `recto-update`'s `the_real_rule_refuses_what_a_test_build_is`.)
#[test]
fn a_release_refused_for_good_isnt_tried_again_by_itself() {
    let (app, bed) = running_with_a_document(3);
    bed.releases.offer(NEW);
    bed.releases.refuse("not_the_team", "it isn't signed by RectoPDF's developer", true);
    full(&app, r#"{"cmd":"update_check"}"#);
    full(&app, r#"{"cmd":"update_install"}"#);
    let status = app.wait_for_status("failed");
    assert_eq!(status["failed_versions"], serde_json::json!([NEW]), "not tried again by itself: {status}");
    still_usable(&app);
    assert_eq!(app.disk().app_version().as_deref(), Some(OLD));
}

/// Failure: a hand-off that's good and then can't go on leaves the app frozen for good. The installer can't be started: the state's written and read back and the
/// app's frozen, then it has to come back, and what an agent and a person do both work again, and it says it failed. (Replaces the app test
/// `a_cancelled_hand_off_doesnt_leave_the_app_frozen`.)
#[test]
fn a_cancelled_hand_off_doesnt_leave_the_app_frozen() {
    let (app, bed) = running_with_a_document(4);
    bed.installer.fail_to_start();
    bed.releases.offer(NEW);
    full(&app, r#"{"cmd":"update_check"}"#);
    full(&app, r#"{"cmd":"update_install"}"#);
    let status = app.wait_for_status("failed");
    assert!(status["words"].as_str().unwrap_or_default().contains("installer"), "{status}");
    assert!(!app.disk().update_files().plan().exists(), "the plan taken back, so no installer acts on it");
    still_usable(&app);
    // A person's action, as the window sends it.
    let tab = app.session_json()["tabs"][0]["id"]
        .as_u64()
        .and_then(|id| u32::try_from(id).ok())
        .expect("the tab's id");
    app.person(&format!(r##"{{"cmd":"goto","tab":"#{tab}","page":1}}"##));
    app.settle();
    assert_eq!(page(&app), 0, "a person's action goes too");
    assert_eq!(app.disk().app_version().as_deref(), Some(OLD));
}

/// Failure: a lost or damaged record turns *Update automatically* back on, against what the user chose, and the app replaces itself. It's in the updater's own
/// file. (Replaces the app test `a_lost_record_doesnt_turn_automatic_updates_back_on`.)
#[test]
fn a_lost_record_doesnt_turn_automatic_updates_back_on() {
    let (app, bed) = running_with_a_document(1);
    full(&app, r#"{"cmd":"settings","auto_update":"off"}"#);
    let saved = app.disk().update_files().saved;
    app.wait_until("the switch written", || recto_update::saved::Saved::read(&saved).auto_update == Some(false));
    let (_, disk) = app.quit();
    std::fs::remove_dir_all(disk.update_files().session()).expect("the record gone");
    let app = disk.builder().update(&bed).start();
    assert_eq!(full(&app, r#"{"cmd":"settings"}"#)["auto_update"], false, "still off");
}

/// Failure: a development build that won't start strands the owner on it, or loses their work. It went in, ran, left the record so the version before couldn't
/// read it, and didn't come up, so the installer went back: the version before comes back with the state as it was handed off, says it went back, and doesn't try
/// it again by itself. (Replaces the app test `a_development_build_that_wont_start_goes_back`; the installer's going back, on a real app, stays in
/// `a_new_app_that_wont_start_goes_back_with_the_same_state`.)
#[test]
fn a_development_build_that_wont_start_goes_back() {
    let (app, bed) = running_with_a_document(12);
    at_full(&app);
    app.agent(Level::Control, r#"{"cmd":"page_rotate","pages":"3","degrees":90}"#).expect("an edit");
    full(&app, r#"{"cmd":"update_dev_builds","set":"on"}"#);
    full(&app, r#"{"cmd":"session_save"}"#);
    app.settle();
    let before = app.session_json();
    let zip = app.disk().hand_over_build(DEV);
    full(&app, &format!(r#"{{"cmd":"update_install","zip":{},"now":true}}"#, serde_json::json!(zip)));
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::Swapped);
    // What the build did as it ended, having shown nothing.
    for slot in ["record.a.json", "record.b.json"] {
        std::fs::write(disk.update_files().session().join(slot), "damaged by the version that wouldn't start").expect("damaged");
    }
    disk.went_back(&plan, "ended_before_up");
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.disk().app_version().as_deref(), Some(OLD), "the old app is back at the app's path");
    let status = app.update_status();
    assert_eq!(status["status"], "went_back", "{status}");
    assert_eq!(status["failed_versions"], serde_json::json!([DEV]));
    assert_eq!(app.session_json()["tabs"], before["tabs"], "the state as it was handed off");
}

/// Failure: a new version that misreads the record and starts empty writes its empty record over the user's, and going back brings back nothing. It came up with
/// fewer tabs than were handed off, so the installer went back with the record from before: the version before has the session as it was, and says why. (Replaces
/// the app test `a_new_version_that_misreads_the_record_goes_back`; the installer's count of the tabs is `recto-update`'s
/// `a_new_app_that_comes_up_empty_goes_back_with_the_record_as_it_was`.)
#[test]
fn a_new_version_that_misreads_the_record_goes_back() {
    let (app, bed) = running_with_a_document(9);
    let before = app.session_json();
    bed.releases.offer(NEW);
    full(&app, r#"{"cmd":"update_check"}"#);
    full(&app, r#"{"cmd":"update_install"}"#);
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::Swapped);
    disk.break_app(recto_update::TestBreakage::MisreadsTheRecord);
    // The new version comes up empty, and writes that as it quits.
    let new = disk.builder().update(&bed).start();
    assert_eq!(new.session_json()["tabs"].as_array().map(Vec::len), Some(0), "it misread the record");
    let (_, disk) = new.quit();
    disk.went_back(&plan, "misread");
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.disk().app_version().as_deref(), Some(OLD));
    let status = app.update_status();
    assert_eq!(status["status"], "went_back", "{status}");
    assert_eq!(status["failed_versions"], serde_json::json!([NEW]));
    let notice = app.core().updates.notice().expect("it says what happened");
    assert!(notice.text.contains("didn't read your last session"), "{notice:?}");
    assert_eq!(app.session_json()["tabs"], before["tabs"], "the session as it was");
}

/// Failure (full check under load, 2026-11-11: "still closed", and "still in the Dock", each once): the new version read its state after the window's second of
/// patience, so the window went up without it, and a closed one opened. A slow disk, on purpose: the new version's state comes in after the host's second, and a
/// launch an update opened waits for it, on the session thread's own deadline, so the window stays closed, and the session comes back with it. (Replaces the app
/// test `a_slow_disk_doesnt_bring_back_what_was_out_of_sight`; what a hidden app does is the window's, and every order of the state and the patience is
/// `lifecycle`'s tier-1 test.)
#[test]
fn a_slow_disk_doesnt_bring_back_what_was_out_of_sight() {
    let (app, bed) = running_with_a_document(5);
    app.core().window_closed();
    app.settle();
    bed.releases.offer(NEW);
    full(&app, r#"{"cmd":"update_check"}"#);
    full(&app, r#"{"cmd":"update_install"}"#);
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).state_late().start();
    assert_eq!(app.disk().app_version().as_deref(), Some(NEW));
    assert_eq!(app.session_json()["tabs"].as_array().map(Vec::len), Some(1), "the state's in");
    assert!(crate::lock(&app.core().window.app).window_closed, "still closed: heard {:?}", app.heard());
    assert_eq!(
        app.listener().count(&crate::testkit::Heard::RestoreReady),
        1,
        "told once, with the state: {:?}",
        app.heard()
    );
}

/// Failure (CI, 2026-11-11: an update's launch under slow conditions): a launch an update opened goes up without its state when what it is and the state are
/// both known late, on a slow machine, so it shows a window that isn't the one left, and what comes in later is made into a window again or lost. A launch an
/// update may have opened waits for its state (`UPDATE_RESTORE_PATIENCE`), whatever the host's second of patience says. The new version's look at what it is
/// held, then its state's read held: through both, past the host's second, the host is told nothing; the state let in, it's told once, with the state, no window
/// is made again, and the session is the one handed off, whole, its edits too. (What `update::slow::an_update_changes_nothing_when_every_wait_is_late` held for the slow stage;
/// the real swap and its first frame stay in that test's normal run, `an_update_in_the_middle_of_things_changes_nothing`.)
#[test]
fn a_launch_an_update_opened_waits_for_its_state_however_late() {
    let (app, bed) = running_with_a_document(9);
    let second = app.disk().pdf("beta.pdf", 6);
    app.open(&second);
    app.agent(Level::Control, r#"{"cmd":"page_rotate","pages":"2","degrees":90}"#).expect("an edit");
    app.act(crate::core::controls::UiAction::FindTyped { text: "beta page".into() });
    app.settle();
    let before = app.session_json();
    bed.releases.offer(NEW);
    full(&app, r#"{"cmd":"update_check"}"#);
    full(&app, r#"{"cmd":"update_install"}"#);
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).hold(&[Seam::Launch, Seam::State]).start();
    let told = || app.listener().count(&Heard::RestoreReady);
    assert_eq!(
        told(),
        0,
        "past the host's second, with what it is not yet known: told nothing: {:?}",
        app.heard()
    );
    app.let_through(Seam::Launch, 1);
    app.wait_until("the state's read to wait at its seam", || app.waiting(Seam::State) > 0);
    app.settle();
    assert_eq!(told(), 0, "known to be an update's launch, the state not in: told nothing: {:?}", app.heard());
    app.let_state_in();
    assert_eq!(told(), 1, "told once, with the state: {:?}", app.heard());
    assert!(app.core().life.shown_with_state(), "the window made from the state");
    assert_eq!(app.record_json()["remade"], false, "and not made again: {}", app.record_json());
    assert_eq!(app.disk().app_version().as_deref(), Some(NEW));
    assert_eq!(unjournalled(app.session_json()), unjournalled(before), "the session handed off, whole");
}

/// A session's `qa state` without the name of each tab's journal file: the edits are in `edits_full` either way, and a launch writes its journal again on its
/// own clock.
fn unjournalled(mut session: Value) -> Value {
    for tab in session["tabs"].as_array_mut().into_iter().flatten() {
        tab.as_object_mut().map(|tab| tab.remove("journal"));
    }
    session
}

/// Failure (review 1): going back from a version that kept ending deleted the record from before the update, the only good copy when what that version left
/// can't be read by the one before. The new version ran, ended early three times and left the record damaged, and the launch went back keeping the record as it
/// was: the version before can't read it, puts back the record from before, and says so. (Replaces the app test
/// `coming_back_to_a_record_it_cant_read_takes_the_one_from_before`; the crash loop itself, on the real app, stays in
/// `a_new_version_that_keeps_ending_goes_back_by_itself`.)
#[test]
fn coming_back_to_a_record_it_cant_read_takes_the_one_from_before() {
    let (app, bed) = running_with_a_document(12);
    app.agent(Level::Control, r#"{"cmd":"page_rotate","pages":"3","degrees":90}"#).expect("an edit");
    full(&app, r#"{"cmd":"session_save"}"#);
    app.settle();
    let before = app.session_json();
    bed.releases.offer(NEW);
    full(&app, r#"{"cmd":"update_check"}"#);
    full(&app, r#"{"cmd":"update_install"}"#);
    let (plan, disk) = app.handed_off(&bed);
    disk.install(&plan, Installed::Swapped);
    for slot in ["record.a.json", "record.b.json"] {
        std::fs::write(disk.update_files().session().join(slot), "damaged by the version that kept ending").expect("damaged");
    }
    // The launch guard's way back: the record as the new version left it.
    let back = recto_update::plan::Plan { keep_record: true, ..plan };
    disk.went_back(&back, "kept_ending");
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.disk().app_version().as_deref(), Some(OLD));
    assert_eq!(app.session_json()["tabs"], before["tabs"], "the session from before the update");
    let notice = app.core().updates.notice().expect("it says so");
    assert!(notice.text.contains("from before the update"), "{notice:?}");
    let log = std::fs::read_to_string(&app.disk().update_files().log).unwrap_or_default();
    assert!(log.contains("record from before the update was put back"), "{log}");
}

/// Failure (owner, 2026-11-12: "I dont see the update now button in settings"): with a development build waiting for its five idle minutes, Settings shows no way
/// to install it, the plain command installs it against the owner's rule, or the button does something the menu and `--now` don't. The build handed over by
/// `recto`'s own hand-over, the copy gone once it's read; Settings and the menu in Rust's words; the plain command only says what it waits for; the button goes at
/// once, through the same hand-off. (What the app test `settings_offers_update_now_for_a_waiting_build` held besides its shots; it keeps the shots.)
#[test]
fn settings_offers_update_now_for_a_waiting_build() {
    let (app, bed) = running_with_a_document(4);
    at_full(&app);
    full(&app, r#"{"cmd":"update_dev_builds","set":"on"}"#);
    assert_eq!(app.core().settings_model().action, None, "nothing waits, so no button");
    let zip = app.disk().hand_over_build(DEV);
    let answer = full(&app, &format!(r#"{{"cmd":"update_install","zip":{}}}"#, serde_json::json!(zip)));
    assert!(answer["waits_ms"].as_u64().is_some_and(|waits| waits > 4 * 60_000), "{answer}");
    let folder = std::path::PathBuf::from(app.update_status()["handover_dir"].as_str().expect("the hand-over folder"));
    assert_eq!(
        std::fs::read_dir(&folder).map(Iterator::count).ok(),
        Some(0),
        "the copy is gone once it was read"
    );
    // The plain command starts nothing: the five idle minutes are the owner's rule.
    let plain = full(&app, r#"{"cmd":"update_install"}"#);
    assert_eq!(plain["started"], false, "{plain}");
    assert!(plain["words"].as_str().unwrap_or_default().contains("Nothing was started"), "{plain}");
    assert_eq!(app.update_status()["status"], "build_ready");
    let action = app.core().settings_model().action.expect("Settings shows the waiting build");
    assert_eq!(action.title, format!("Development build {DEV} is ready"));
    assert_eq!((action.button.as_str(), action.enabled), ("Update now", true), "{action:?}");
    assert_eq!(
        app.update_status()["menu"],
        serde_json::json!({ "title": format!("Update to {DEV}…"), "enabled": true })
    );
    // The button is the menu's call and `--now`'s: it goes at once.
    app.act(crate::core::controls::UiAction::SettingChosen {
        id: crate::core::updates::UPDATE_NOW_ROW.to_owned(),
        on: true,
    });
    let (plan, disk) = app.handed_off(&bed);
    assert_eq!(plan.to, DEV);
    disk.install(&plan, Installed::Swapped);
    let app = disk.builder().update(&bed).start();
    assert_eq!(app.disk().app_version().as_deref(), Some(DEV));
}

/// Failure (review 1): a version being watched after an update counts a quit someone asked for as a run that ended early, so three ordinary quits in its first
/// minutes send a good version back; or a kill isn't counted, and a crash loop is never caught. A quit isn't counted; a kill is. (What the app test
/// `a_quit_isnt_an_early_end_and_a_kill_is` held besides SIGTERM's being a quit, which is the window's; it keeps that.)
#[test]
fn a_quit_isnt_an_early_end_and_a_kill_is() {
    let disk = Disk::new();
    disk.install_app(OLD);
    let bed = UpdateBed::new();
    let files = disk.update_files();
    recto_update::saved::Saved {
        proven: Some(OLD.to_owned()),
        probation: Some(recto_update::saved::Probation {
            version: OLD.to_owned(),
            since_ms: recto_update::wall_ms(),
            running_since_ms: None,
            early_ends: 0,
        }),
        ..recto_update::saved::Saved::default()
    }
    .write(&files.saved);
    let probation = || recto_update::saved::Saved::read(&files.saved).probation.expect("watched");
    let app = disk.builder().update(&bed).start();
    assert!(probation().running_since_ms.is_some(), "this run's watched");
    let (_, disk) = app.quit();
    assert_eq!(probation().running_since_ms, None, "the menu's quit isn't an early end");
    let app = disk.builder().update(&bed).start();
    assert_eq!(probation().early_ends, 0);
    let disk = app.kill();
    let app = disk.builder().update(&bed).start();
    assert_eq!(probation().early_ends, 1, "a kill is");
    drop(app);
}
