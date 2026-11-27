//! The updater's look at an unfinished removal's record: each test names the failure it catches.

use super::*;
use crate::stage::tests::{fake_app, temp_dir};

/// Failure: a build that doesn't read the record is gone back to while a removal is pending (it restores everything, the purged data with it), or a build
/// that does is refused, or the kept copy of the session is put back over a removal's record, or a going back with no record changes from what it was.
#[test]
fn going_back_is_decided_by_the_record_and_by_whether_the_kept_build_reads_it() {
    use GoingBack::{LeaveRecord, Refuse, Restore};
    let table = [
        // (record there, kept build reads it, plan keeps the record) -> what is done
        ((false, false, false), Restore),
        ((false, false, true), LeaveRecord),
        ((false, true, false), Restore),
        ((false, true, true), LeaveRecord),
        ((true, false, false), Refuse),
        ((true, false, true), Refuse),
        ((true, true, false), LeaveRecord),
        ((true, true, true), LeaveRecord),
    ];
    for ((record, reads, keep), want) in table {
        assert_eq!(going_back(record, reads, keep), want, "record {record}, reads {reads}, keep {keep}");
    }
}

/// Failure: a record that can't be read, or a link, or a folder, at the record's name is taken for "nothing pending" (the install then copies the history being
/// purged, and a going back restores it); or a leftover temporary or the ledger, which are no record, stops every update for good.
#[test]
fn anything_at_the_records_name_is_a_record_and_nothing_else_is() {
    let dir = temp_dir("removal-record-names");
    let root = dir.join("state");
    std::fs::create_dir_all(&root).unwrap();
    assert!(!anything_there(&root), "no removal folder");
    std::fs::create_dir(root.join(DIR)).unwrap();
    assert!(!anything_there(&root), "an empty one");
    std::fs::write(root.join(DIR).join("ledger.v1"), b"x").unwrap();
    std::fs::write(root.join(DIR).join("pending.v1.new-0123456789abcdef"), b"x").unwrap();
    assert!(!anything_there(&root), "the ledger and a leftover temporary are no record");
    std::fs::write(root.join(DIR).join(NAME), b"garbage").unwrap();
    assert!(anything_there(&root), "bytes that don't decode");
    std::fs::remove_file(root.join(DIR).join(NAME)).unwrap();
    std::os::unix::fs::symlink(root.join("nowhere"), root.join(DIR).join(NAME)).unwrap();
    assert!(anything_there(&root), "a link to nothing");
    std::fs::remove_file(root.join(DIR).join(NAME)).unwrap();
    std::fs::create_dir(root.join(DIR).join(NAME)).unwrap();
    assert!(anything_there(&root), "a folder");
    std::fs::remove_dir_all(root.join(DIR)).unwrap();
    std::fs::write(root.join(DIR), b"x").unwrap();
    assert!(anything_there(&root), "removal is a file");
    std::fs::remove_file(root.join(DIR)).unwrap();
    std::os::unix::fs::symlink(&dir.path(), root.join(DIR)).unwrap();
    assert!(anything_there(&root), "removal is a link");
}

/// Failure: a build is taken for one that reads the record when its plist doesn't say so (an older build), or the other way about.
#[test]
fn a_build_reads_the_record_only_when_its_plist_says_so() {
    let dir = temp_dir("removal-record-plist");
    let app = fake_app(dir.path(), "0.1.0");
    assert!(!reads_the_record(&app), "no key");
    let add = |value: &str| {
        let plist = app.join("Contents/Info.plist");
        let _ = std::process::Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", &format!("Delete :{GATE_KEY}")])
            .arg(&plist)
            .output();
        assert!(
            std::process::Command::new("/usr/libexec/PlistBuddy")
                .args(["-c", &format!("Add :{GATE_KEY} string {value}")])
                .arg(&plist)
                .status()
                .unwrap()
                .success()
        );
    };
    add("1");
    assert!(reads_the_record(&app));
    add("0");
    assert!(!reads_the_record(&app), "a key that isn't 1");
    assert!(!reads_the_record(&dir.join("not-an-app")), "no app");
}

/// Failure: `restore_record`, whoever calls it, puts the update's kept copy of the session over a removal's record (it may hold the history being purged), or
/// stops working for the one place it is for when no record is there.
#[test]
fn the_kept_copy_is_never_put_back_while_a_record_is_there() {
    let dir = temp_dir("removal-record-belt");
    let root = dir.join("state");
    let (session, before) = (root.join("session"), root.join("update").join("before"));
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("record.a.json"), "the state").unwrap();
    assert!(crate::install::copy_record(&session, &before, &mut |_| {}));
    let touch = |text: &str| std::fs::write(session.join("record.a.json"), text).unwrap();
    let read = || std::fs::read_to_string(session.join("record.a.json")).unwrap();
    touch("what the removal left");
    std::fs::create_dir_all(root.join(DIR)).unwrap();
    std::fs::write(root.join(DIR).join(NAME), b"a record").unwrap();
    crate::install::restore_record(&before, &session);
    assert_eq!(read(), "what the removal left", "a record is there: nothing is put back");
    std::fs::remove_file(root.join(DIR).join(NAME)).unwrap();
    // A copy that isn't where the installer keeps it is never put back either.
    let elsewhere = root.join("somewhere");
    assert!(crate::install::copy_record(&session, &elsewhere, &mut |_| {}));
    touch("changed again");
    crate::install::restore_record(&elsewhere, &session);
    assert_eq!(read(), "changed again", "not the installer's own copy");
    crate::install::restore_record(&before, &session);
    assert_eq!(read(), "the state", "no record: the kept copy comes back as it always did");
}

/// Failure: the key the updater looks for and the key the app's build writes drift apart (a rename on one side), and no build is ever one that reads the
/// record, so a going back is refused for good while a removal is pending, or, the other way, a build is trusted that doesn't carry it.
#[test]
fn the_apps_build_writes_the_key_the_updater_looks_for() {
    let build = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/build.sh")).expect("the app's build script");
    assert!(
        build.contains(&format!("<key>{GATE_KEY}</key><string>1</string>")),
        "app/build.sh doesn't write {GATE_KEY}"
    );
}
