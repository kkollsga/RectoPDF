//! The survey lists a real update copy: `copy_record`, the installer's own function, run on a session the real record store wrote.

use super::*;
use crate::removal_store::copy_fixtures::{State, complete, copy_tree, find, found_of, journal_name, state, surveyed, write_session};
use crate::removal_store::journals::survey_journals;
use crate::removal_store::legacy_session::survey_legacy_sessions;
use crate::removal_store::testing::put;
use recto_core::removal::{Format, Owner, Surveyed};
use recto_testpdf::returns_in_time;
use std::sync::Arc;

fn survey_with(root: &Path, caps: &UpdateCaps) -> Result<Surveyed, Refusal> {
    surveyed(&[Store::UpdateCopy], |lineage, survey| {
        survey_update_copy_with(root, lineage, TargetMode::Save, survey, caps)
    })
}

fn survey(root: &Path) -> Result<Surveyed, Refusal> {
    survey_with(root, &UpdateCaps::default())
}

fn before(state: &State) -> std::path::PathBuf {
    state.root().join("update").join("before")
}

/// A state folder with a real session, and the installer's own copy of it in `update/before/`.
fn updated() -> State {
    let state = state();
    write_session(&state);
    std::fs::create_dir_all(state.root().join("update")).unwrap();
    assert!(recto_update::install::copy_record(&state.session(), &before(&state), &mut |_| {}));
    state
}

/// Failure: the copy an update keeps for going back holds the document's history and the last search, and is skipped (a plan removes the document
/// and leaves its edit history and a tab's path in a week-long copy), or `.whole` is taken for the target's. One whole update: the slots are `Unknown`
/// and never opened, the journal is the target's in the copy's own store, and the marker is left out.
#[test]
fn one_whole_update_leaves_the_slots_unknown_and_the_journal_the_targets() {
    let state = updated();
    assert!(before(&state).join(".whole").exists());
    let s = survey(state.root()).unwrap();
    assert!(complete(&s, Store::UpdateCopy));
    assert_eq!(find(&s, Store::UpdateCopy, "update/before/.whole"), None);
    let slot = ["record.a.json", "record.b.json"]
        .into_iter()
        .find(|slot| before(&state).join(slot).exists())
        .expect("a slot was copied");
    assert_eq!(
        find(&s, Store::UpdateCopy, &format!("update/before/{slot}")),
        Some((Owner::Unknown, Format::Current))
    );
    assert_eq!(
        find(&s, Store::UpdateCopy, &format!("update/before/journals/{}", journal_name(7))),
        Some((Owner::Target, Format::Current))
    );
    assert!(found_of(&s).iter().all(|f| f.0 == Store::UpdateCopy));
}

/// Failure: the live journals and the update's copy of the same journal are one survey's `Inconsistent`, or the copy's journal lands in the live
/// store, so a live purge takes the copy's and the copy's own purge finds nothing. Survey both into one survey: it finishes, and each store holds its own.
#[test]
fn a_journal_live_and_the_same_one_in_the_copy_are_two_artifacts_in_two_stores() {
    let state = updated();
    let root = state.root().to_path_buf();
    let s = surveyed(
        &[
            Store::UpdateCopy,
            Store::Journal,
            Store::JournalVariant,
            Store::PageIds,
            Store::SleepFile,
            Store::KeptPages,
            Store::LegacySession,
        ],
        |lineage, survey| {
            survey_journals(&root, lineage, TargetMode::Save, survey)?;
            survey_update_copy(&root, lineage, TargetMode::Save, survey)?;
            survey_legacy_sessions(&root, lineage, TargetMode::Save, survey)
        },
    )
    .unwrap();
    let key = format!("journals/{}", journal_name(7));
    assert_eq!(find(&s, Store::Journal, &format!("session/{key}")), Some((Owner::Target, Format::Current)));
    assert_eq!(
        find(&s, Store::UpdateCopy, &format!("update/before/{key}")),
        Some((Owner::Target, Format::Current))
    );
    assert_eq!(find(&s, Store::Journal, &format!("update/before/{key}")), None);
    assert!(complete(&s, Store::UpdateCopy) && complete(&s, Store::Journal) && complete(&s, Store::LegacySession));
}

/// Failure: a copy killed partway (no `.whole`) is read as empty or as not there, though nothing deletes it before the next install and it holds a
/// session's paths. Surveyed exactly as the whole one is.
#[test]
fn a_copy_without_its_marker_is_surveyed_the_same() {
    let state = updated();
    let whole = survey(state.root()).unwrap();
    std::fs::remove_file(before(&state).join(".whole")).unwrap();
    let partial = survey(state.root()).unwrap();
    assert!(complete(&partial, Store::UpdateCopy));
    assert_eq!(found_of(&partial), found_of(&whole));
    assert!(!found_of(&partial).is_empty());
}

/// Failure: `.whole` is left out by its name alone, so a second name for another file, or a folder, hides what the copy holds. Only a plain file of
/// one link is left out; anything else with that name is `Unknown`.
#[test]
fn the_marker_is_left_out_only_as_a_plain_file_of_one_link() {
    let state = updated();
    let marker = before(&state).join(".whole");
    std::fs::hard_link(&marker, state.root().join("update").join("elsewhere")).unwrap();
    let s = survey(state.root()).unwrap();
    assert_eq!(
        find(&s, Store::UpdateCopy, "update/before/.whole"),
        Some((Owner::Unknown, Format::Current)),
        "a second link"
    );
    std::fs::remove_file(&marker).unwrap();
    std::fs::remove_file(state.root().join("update").join("elsewhere")).unwrap();
    std::fs::create_dir(&marker).unwrap();
    let s = survey(state.root()).unwrap();
    assert_eq!(
        find(&s, Store::UpdateCopy, "update/before/.whole"),
        Some((Owner::Unknown, Format::Current)),
        "a folder"
    );
}

/// Failure: `update` or `before` that is a link is followed, so the survey lists a folder the app didn't make (or opens `before` by a path whose
/// middle is a link). Unmarked, and nothing under the link is listed.
#[test]
fn a_link_at_update_or_before_is_never_followed_and_leaves_the_store_unmarked() {
    let state = updated();
    let away = recto_testpdf::TempDir::new("recto-away-");
    std::fs::rename(state.root().join("update"), away.join("update")).unwrap();
    std::os::unix::fs::symlink(away.join("update"), state.root().join("update")).unwrap();
    let s = survey(state.root()).unwrap();
    assert!(!complete(&s, Store::UpdateCopy) && found_of(&s).is_empty(), "update is a link");
    std::fs::remove_file(state.root().join("update")).unwrap();
    std::fs::create_dir(state.root().join("update")).unwrap();
    std::os::unix::fs::symlink(away.join("update").join("before"), state.root().join("update").join("before")).unwrap();
    let s = survey(state.root()).unwrap();
    assert!(!complete(&s, Store::UpdateCopy) && found_of(&s).is_empty(), "before is a link");
}

/// Failure: a step that isn't there is read as unfinished, so a person who never updated can't remove anything; or an `update` with no `before`
/// (a week passed) is. Complete and empty at each step.
#[test]
fn a_missing_step_is_complete_and_empty() {
    let state = state();
    let s = survey(&state.root().join("nowhere")).unwrap();
    assert!(complete(&s, Store::UpdateCopy) && found_of(&s).is_empty());
    let s = survey(state.root()).unwrap();
    assert!(complete(&s, Store::UpdateCopy) && found_of(&s).is_empty(), "no update");
    std::fs::create_dir(state.root().join("update")).unwrap();
    put(&state.root().join("update"), "launch-ok", b"");
    let s = survey(state.root()).unwrap();
    assert!(
        complete(&s, Store::UpdateCopy) && found_of(&s).is_empty(),
        "no before: the rest of `update` is another store's"
    );
}

/// Failure: `before` past its cap is let through at the cap plus one (`>` for `>=`), or one exactly at it is refused. `before` holds three entries (a
/// file, the marker and `journals`) and its `journals` one, so only `before`'s own cap is what refuses at 2 (the journals' cap is held in
/// `legacy_session`'s test, and by the same function in `journals.rs`).
#[test]
fn the_caps_refuse_one_past_and_not_at() {
    let state = state();
    let copy = before(&state);
    std::fs::create_dir_all(copy.join("journals")).unwrap();
    put(&copy, "a", b"");
    put(&copy, ".whole", b"");
    put(&copy.join("journals"), "x", b"");
    let at = |entries| UpdateCaps {
        folder: Caps { entries, ..Caps::default() },
        ..UpdateCaps::default()
    };
    assert!(survey_with(state.root(), &at(3)).is_ok(), "three entries in before at a cap of three");
    assert_eq!(survey_with(state.root(), &at(2)).unwrap_err(), Refusal::TooLarge);
}

/// Failure: a `before` swapped for a link after `update` was opened is followed. `update` is opened first, and `before` from it, so the swap yields
/// an unmarked store, with nothing read through the link.
#[test]
fn a_before_swapped_for_a_link_between_the_steps_is_never_followed() {
    let state = updated();
    let away = recto_testpdf::TempDir::new("recto-away-");
    copy_tree(&before(&state), away.path());
    let target = away.path().to_path_buf();
    let hook: crate::removal_store::testing::Hook = Arc::new(move |path| {
        if path.ends_with("before") {
            std::fs::remove_dir_all(path).unwrap();
            std::os::unix::fs::symlink(&target, path).unwrap();
        }
    });
    let caps = UpdateCaps {
        before_open: Some(hook),
        ..UpdateCaps::default()
    };
    let root = state.root().to_path_buf();
    let s = returns_in_time("the survey waited on something", move || survey_with(&root, &caps)).unwrap();
    assert!(!complete(&s, Store::UpdateCopy));
    assert!(found_of(&s).is_empty());
}
