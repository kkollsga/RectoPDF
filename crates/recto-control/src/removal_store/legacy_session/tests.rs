//! The survey lists real folders in a temp state root, written by the real record store where it can be, and the artifacts it adds are what a plan is
//! made from.

use super::*;
use crate::record_store::{Flavor, RecordStore};
use crate::removal_store::copy_fixtures::{
    MINE, complete, copy_tree, find, found_of, journal_bytes, journal_name, session_record, state, surveyed, write_session,
};
use crate::removal_store::testing::{mkfifo, put};
use recto_core::removal::Surveyed;
use recto_session::record::CURRENT_VERSION as VERSION;
use recto_testpdf::returns_in_time;
use std::sync::Arc;

fn survey_with(root: &Path, caps: &LegacyCaps) -> Result<Surveyed, Refusal> {
    surveyed(&[Store::LegacySession], |lineage, survey| {
        survey_legacy_sessions_with(root, lineage, TargetMode::Save, survey, caps)
    })
}

fn survey(root: &Path) -> Result<Surveyed, Refusal> {
    survey_with(root, &LegacyCaps::default())
}

fn names(s: &Surveyed) -> Vec<(Owner, Format)> {
    found_of(s).into_iter().map(|f| (f.1, f.2)).collect()
}

/// Failure: a name is read as a copy when no build writes it (a leading zero, a sign, a number past a `u32`), so a stray folder is opened as one; or a
/// name that fits no rule is skipped as "not ours", so a folder that might hold a document's history reads as "none found". One table of every shape.
#[test]
fn the_names_of_the_copies_are_exactly_the_ones_the_store_writes() {
    let v = VERSION;
    let table: Vec<(String, Name)> = vec![
        // The live folders are surveyed elsewhere.
        ("session".into(), Name::NotOurs),
        ("session-windowless".into(), Name::NotOurs),
        // Copies a build writes.
        (format!("session.v{v}"), Name::Copy),
        (format!("session-windowless.v{v}"), Name::Copy),
        ("session.damaged".into(), Name::Copy),
        ("session.damaged.1".into(), Name::Copy),
        ("session.damaged.12".into(), Name::Copy),
        ("session-windowless.damaged.2".into(), Name::Copy),
        // From a newer build.
        (format!("session.v{}", v + 1), Name::Newer),
        ("session.v4294967295".into(), Name::Newer),
        // Starts with `session` and fits no rule: Unknown, never skipped.
        ("session.v0".into(), Name::Other),
        (format!("session.v0{v}"), Name::Other),
        ("session.v".into(), Name::Other),
        ("session.v+1".into(), Name::Other),
        ("session.v-1".into(), Name::Other),
        ("session.v4294967296".into(), Name::Other),
        (format!("session.v{v}.bak"), Name::Other),
        ("session.damaged.0".into(), Name::Other),
        ("session.damaged.01".into(), Name::Other),
        ("session.damaged.".into(), Name::Other),
        ("session.damaged.1.x".into(), Name::Other),
        ("session.old".into(), Name::Other),
        ("sessions".into(), Name::Other),
        ("session-windowless.x".into(), Name::Other),
        ("session-windowless.v".into(), Name::Other),
        // Not this store's at all.
        ("ledger".into(), Name::NotOurs),
        ("update".into(), Name::NotOurs),
        ("run".into(), Name::NotOurs),
        (".DS_Store".into(), Name::NotOurs),
        // A case-insensitive volume reaches a folder by any case of its name, so a name that is `session` in any case is Unknown, never skipped.
        ("Session.v1".into(), Name::Other),
        ("SESSION".into(), Name::Other),
        ("Session-Windowless.damaged".into(), Name::Other),
        ("session.V1".into(), Name::Other),
        ("SeSsIoN-windowless.DAMAGED.2".into(), Name::Other),
        // A false refusal by design: it starts with `session` and is no name the app writes.
        ("Sessions.txt".into(), Name::Other),
        ("sessio".into(), Name::NotOurs),
        (String::new(), Name::NotOurs),
    ];
    for (name, want) in table {
        assert_eq!(legacy_kind(name.as_bytes()), want, "{name:?}");
    }
    assert_eq!(legacy_kind(b"session.v1\xff"), Name::Other, "a name that isn't text");
}

/// Failure: a copy's record slots are decoded or skipped, so a copy that holds a tab's path and the Find query reads as "none found" or as the target's
/// and is acted on without being proven; or its journal is left out, or put in the live journals' store; or the copy isn't marked complete after the
/// listing, so a plan says `Incomplete` for ever. A session written by the real store, as a going-forward copy leaves one.
#[test]
fn a_real_session_copied_aside_is_unknown_except_its_journal() {
    let state = state();
    write_session(&state);
    let from = state.session();
    copy_tree(&from, &state.root().join(format!("session.v{VERSION}")));
    let s = survey(state.root()).unwrap();
    let copy = format!("session.v{VERSION}");
    assert!(complete(&s, Store::LegacySession));
    for slot in ["record.a.json", "record.b.json", "places.log"] {
        if from.join(slot).exists() {
            assert_eq!(
                find(&s, Store::LegacySession, &format!("{copy}/{slot}")),
                Some((Owner::Unknown, Format::Current)),
                "{slot}"
            );
        }
    }
    assert!(
        from.join("record.a.json").exists() || from.join("record.b.json").exists(),
        "the real store wrote a slot"
    );
    assert_eq!(
        find(&s, Store::LegacySession, &format!("{copy}/journals/{}", journal_name(7))),
        Some((Owner::Target, Format::Current)),
        "the target's journal is the target's, in the copy's store"
    );
    assert!(found_of(&s).iter().all(|f| f.0 == Store::LegacySession), "nothing in a live journals' store");
}

/// Failure: a damaged record kept aside (the real store keeps three) is skipped as "damaged, so empty", but it holds paths and the Find text. Damaged
/// four times: the oldest went with the rotation, and every one left is listed and `Unknown`.
#[test]
fn every_damaged_record_the_store_kept_is_listed_unknown() {
    let state = state();
    for round in 0..4 {
        let store = state.store();
        let _ = store.load();
        store.write_record(&session_record(), &Default::default(), false).unwrap();
        drop(store);
        std::fs::write(state.session().join("record.a.json"), format!("garbage {round}")).unwrap();
        std::fs::write(state.session().join("record.b.json"), format!("garbage {round}")).unwrap();
        let store = RecordStore::open(&state.paths, Flavor::App);
        assert!(matches!(store.load(), crate::record_store::Loaded::Damaged));
    }
    let kept: Vec<String> = ["session.damaged", "session.damaged.1", "session.damaged.2"].map(String::from).to_vec();
    for name in &kept {
        assert!(state.root().join(name).join("record.a.json").exists(), "{name} was kept");
    }
    assert!(!state.root().join("session.damaged.3").exists());
    let s = survey(state.root()).unwrap();
    assert!(complete(&s, Store::LegacySession));
    for name in &kept {
        assert_eq!(
            find(&s, Store::LegacySession, &format!("{name}/record.a.json")),
            Some((Owner::Unknown, Format::Current)),
            "{name}"
        );
    }
    assert!(found_of(&s).iter().all(|f| f.1 == Owner::Unknown));
}

/// Failure: a copy from a newer build is opened (its names are read as this build's, or a pipe inside is waited on), so the survey hangs or places what
/// it can't read. One `Foreign` artifact, nothing inside is opened, and the survey returns.
#[test]
fn a_newer_builds_copy_is_one_foreign_artifact_and_nothing_inside_is_opened() {
    let state = state();
    let newer = state.root().join(format!("session.v{}", VERSION + 1));
    std::fs::create_dir_all(newer.join("journals")).unwrap();
    mkfifo(&newer.join("record.a.json"));
    mkfifo(&newer.join("journals").join(journal_name(7)));
    let root = state.root().to_path_buf();
    let s = returns_in_time("the survey waited on something", move || survey(&root)).unwrap();
    assert!(complete(&s, Store::LegacySession));
    assert_eq!(names(&s), vec![(Owner::Unknown, Format::Foreign)]);
    assert!(find(&s, Store::LegacySession, &format!("session.v{}", VERSION + 1)).is_some());
}

/// Failure: a link at a copy's name is followed (the survey lists what it leads to, which may be a person's own folder, and reads it as a copy's
/// content). One `Unknown` artifact for the link, and nothing under it.
#[test]
fn a_link_at_a_copys_name_is_one_unknown_and_nothing_under_it_is_listed() {
    let state = state();
    let away = recto_testpdf::TempDir::new("recto-away-");
    put(away.path(), "record.a.json", b"{}");
    std::fs::create_dir(away.join("journals")).unwrap();
    put(&away.join("journals"), &journal_name(7), &journal_bytes(MINE, 7));
    let name = format!("session.v{VERSION}");
    std::os::unix::fs::symlink(away.path(), state.root().join(&name)).unwrap();
    let s = survey(state.root()).unwrap();
    assert!(complete(&s, Store::LegacySession));
    assert_eq!(names(&s), vec![(Owner::Unknown, Format::Current)]);
    assert!(find(&s, Store::LegacySession, &name).is_some());
}

/// Failure: a copy that is a plain file, or a name that is no copy's (`session.old`) is skipped or opened, so what a person keeps there is deleted
/// unseen or the survey reads a file as a folder. One `Unknown` artifact each.
#[test]
fn a_file_or_a_stray_name_that_starts_with_session_is_unknown() {
    let state = state();
    put(state.root(), &format!("session.v{VERSION}"), b"not a folder");
    std::fs::create_dir(state.root().join("session.old")).unwrap();
    put(&state.root().join("session.old"), "record.a.json", b"{}");
    put(state.root(), "ledger", b"another store's");
    let s = survey(state.root()).unwrap();
    assert!(complete(&s, Store::LegacySession));
    assert_eq!(names(&s), vec![(Owner::Unknown, Format::Current); 2]);
    assert!(find(&s, Store::LegacySession, &format!("session.v{VERSION}")).is_some());
    assert!(find(&s, Store::LegacySession, "session.old").is_some());
}

/// Failure: a state root that isn't there is read as unfinished (a plan that says `Incomplete` for a user with nothing kept), or one that is a link is
/// followed.
#[test]
fn a_missing_root_is_complete_and_empty_and_a_link_is_not_marked() {
    let state = state();
    let s = survey(&state.root().join("nowhere")).unwrap();
    assert!(complete(&s, Store::LegacySession) && found_of(&s).is_empty());
    let link = state.root().join("link");
    std::os::unix::fs::symlink(state.root(), &link).unwrap();
    let s = survey(&link).unwrap();
    assert!(!complete(&s, Store::LegacySession) && found_of(&s).is_empty());
}

/// The plain markers the app writes are left out, and only those: `went-back` of one link and Finder's `.DS_Store`. Failure: a marker is `Unknown` and
/// a plan refuses for ever after a going back; or a folder, a link or a second link to a document's file named `went-back` is skipped by its name.
#[test]
fn went_back_is_left_out_only_as_a_plain_file_of_one_link() {
    let state = state();
    let copy = state.root().join(format!("session.v{VERSION}"));
    std::fs::create_dir(&copy).unwrap();
    put(&copy, "went-back", b"1");
    put(&copy, ".DS_Store", b"");
    let s = survey(state.root()).unwrap();
    assert!(
        found_of(&s).is_empty() && complete(&s, Store::LegacySession),
        "both are the app's or Finder's own"
    );
    // Another name for the same file is another way to reach a file: not left out.
    std::fs::hard_link(copy.join("went-back"), state.root().join("elsewhere")).unwrap();
    let s = survey(state.root()).unwrap();
    assert_eq!(names(&s), vec![(Owner::Unknown, Format::Current)]);
    std::fs::remove_file(copy.join("went-back")).unwrap();
    std::fs::create_dir(copy.join("went-back")).unwrap();
    let s = survey(state.root()).unwrap();
    assert_eq!(
        find(&s, Store::LegacySession, &format!("session.v{VERSION}/went-back")),
        Some((Owner::Unknown, Format::Current))
    );
}

/// Failure: a copy's `journals` that is a link, or a file, is skipped as "none", so a history sits where nothing looks at it. The store isn't marked.
#[test]
fn a_journals_that_is_no_folder_leaves_the_store_unmarked() {
    let state = state();
    let copy = state.root().join(format!("session.v{VERSION}"));
    std::fs::create_dir(&copy).unwrap();
    std::os::unix::fs::symlink(state.root(), copy.join("journals")).unwrap();
    let s = survey(state.root()).unwrap();
    assert!(!complete(&s, Store::LegacySession));
    // And a copy with no `journals` at all is whole and has none.
    std::fs::remove_file(copy.join("journals")).unwrap();
    let s = survey(state.root()).unwrap();
    assert!(complete(&s, Store::LegacySession) && found_of(&s).is_empty());
}

/// Failure: a cap that is passed by one is let through (`>` for `>=`), so the survey reads a cut listing as whole; or one that is exactly met is
/// refused; or the root's cap skips the names that aren't this store's, so a root of a million other entries is listed in full. Each cap is made small, and tried at the cap and at one past it: the root's entries, the copies opened, a copy's entries and its journals'.
#[test]
fn each_cap_refuses_one_past_it_and_not_at_it() {
    // A bare root, so every entry in it is counted here: two copies and one name that is no copy's (another store's) make three.
    let dir = recto_testpdf::TempDir::new("recto-caps-");
    for k in 1..=2 {
        std::fs::create_dir(dir.path().join(format!("session.damaged.{k}"))).unwrap();
    }
    put(dir.path(), "ledger", b"another store's");
    let root_entries = |entries| LegacyCaps {
        entries,
        ..LegacyCaps::default()
    };
    assert!(survey_with(dir.path(), &root_entries(3)).is_ok(), "three entries at a cap of three");
    assert_eq!(survey_with(dir.path(), &root_entries(2)).unwrap_err(), Refusal::TooLarge);
    let copies = |copies| LegacyCaps {
        copies,
        ..LegacyCaps::default()
    };
    assert!(survey_with(dir.path(), &copies(2)).is_ok());
    assert_eq!(survey_with(dir.path(), &copies(1)).unwrap_err(), Refusal::TooLarge);
    // A copy's own entries, and its journals' (the same per-folder cap).
    let one = dir.path().join("session.damaged.1");
    put(&one, "a", b"");
    put(&one, "b", b"");
    let per_folder = |entries| LegacyCaps {
        folder: Caps { entries, ..Caps::default() },
        ..LegacyCaps::default()
    };
    assert!(survey_with(dir.path(), &per_folder(2)).is_ok());
    assert_eq!(survey_with(dir.path(), &per_folder(1)).unwrap_err(), Refusal::TooLarge);
    let two = dir.path().join("session.damaged.2");
    std::fs::create_dir(two.join("journals")).unwrap();
    for name in ["x", "y", "z"] {
        put(&two.join("journals"), name, b"");
    }
    assert_eq!(
        survey_with(dir.path(), &per_folder(2)).unwrap_err(),
        Refusal::TooLarge,
        "the journals folder is capped too"
    );
    assert!(survey_with(dir.path(), &per_folder(3)).is_ok());
}

/// Failure: a copy that vanishes between the listing and its opening is read as an empty copy, so the survey says "none found" for a folder that was
/// there a moment ago. The store isn't marked.
#[test]
fn a_copy_gone_between_the_listing_and_its_opening_leaves_the_store_unmarked() {
    let state = state();
    let copy = state.root().join(format!("session.v{VERSION}"));
    std::fs::create_dir(&copy).unwrap();
    put(&copy, "record.a.json", b"{}");
    let hook: crate::removal_store::testing::Hook = Arc::new(|path| {
        let _ = std::fs::remove_dir_all(path);
    });
    let caps = LegacyCaps {
        before_open: Some(hook),
        ..LegacyCaps::default()
    };
    let s = survey_with(state.root(), &caps).unwrap();
    assert!(!complete(&s, Store::LegacySession));
}

/// Failure: a copy swapped for a link after the listing is followed. It is opened with no link followed, so it is one `Unknown` artifact.
#[test]
fn a_copy_swapped_for_a_link_after_the_listing_is_never_followed() {
    let state = state();
    let away = recto_testpdf::TempDir::new("recto-away-");
    put(away.path(), "record.a.json", b"{}");
    // Two files there, so a link that was followed is told from the one artifact the link itself is.
    put(away.path(), "record.b.json", b"{}");
    let copy = state.root().join(format!("session.v{VERSION}"));
    std::fs::create_dir(&copy).unwrap();
    let target = away.path().to_path_buf();
    let hook: crate::removal_store::testing::Hook = Arc::new(move |path| {
        std::fs::remove_dir_all(path).unwrap();
        std::os::unix::fs::symlink(&target, path).unwrap();
    });
    let caps = LegacyCaps {
        before_open: Some(hook),
        ..LegacyCaps::default()
    };
    let s = survey_with(state.root(), &caps).unwrap();
    assert_eq!(names(&s), vec![(Owner::Unknown, Format::Current)]);
    assert!(complete(&s, Store::LegacySession));
}

/// Failure: a copy's `journals` that was listed and has vanished when it is opened reads as "none", so the survey says complete for a folder of
/// histories that was there a moment ago. The store isn't marked. (A copy that never had one is complete: `a_journals_that_is_no_folder_…`.)
#[test]
fn a_journals_folder_gone_between_the_listing_and_its_opening_leaves_the_store_unmarked() {
    let state = state();
    let copy = state.root().join(format!("session.v{VERSION}"));
    std::fs::create_dir_all(copy.join("journals")).unwrap();
    let hook: crate::removal_store::testing::Hook = Arc::new(|path| {
        if path.file_name().is_some_and(|name| name == "journals") {
            std::fs::remove_dir_all(path).unwrap();
        }
    });
    let caps = LegacyCaps {
        folder: Caps {
            before_open: Some(hook),
            ..Caps::default()
        },
        ..LegacyCaps::default()
    };
    let s = survey_with(state.root(), &caps).unwrap();
    assert!(!complete(&s, Store::LegacySession));
}
