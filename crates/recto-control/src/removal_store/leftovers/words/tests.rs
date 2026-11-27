use super::*;

fn found() -> Found {
    Found {
        saves_here: 1,
        saves_elsewhere: 2,
        other_folders: 2,
        copies: 3,
        waiting_here: 1,
        waiting_elsewhere: 0,
        waiting_folders: 0,
        no_trash: 1,
    }
}

/// Failure: an agent's words carry a file's name (document text reaching an agent as a sentence), or the person's lose the name that tells them which
/// document it is about. The same sentences otherwise.
#[test]
fn names_are_the_persons_and_an_agent_reads_the_same_words_without_them() {
    let person = check(
        &Names {
            document: Some("report.pdf".into()),
            folder: Some("Documents".into()),
        },
        &found(),
    );
    let agent = check(&Names::default(), &found());
    assert!(person.contains("“report.pdf”") && person.contains("“Documents”"), "{person}");
    assert!(!agent.contains('“'), "{agent}");
    assert_eq!(person.lines().count(), agent.lines().count(), "one sentence a thing for both");
}

/// Failure: the words say a save is "this document's" (a crashed save is found by its shape beside a document, so whose it is can't be said), or
/// that everything was cleared or nothing is left (the history gate is not asked for this scope).
#[test]
fn the_words_never_claim_whose_a_save_is_or_that_everything_is_gone() {
    let all = [
        check(&Names::default(), &found()),
        nothing(&Found::default()),
        done(&Did {
            trashed: 1,
            copies: 1,
            ..Did::default()
        }),
        stopped(&Did { trashed: 1, ..Did::default() }, 1, 2, "why"),
    ]
    .join("\n")
    .to_lowercase();
    for claim in ["this document's", "everything", "nothing is left", "nothing left", "all clear"] {
        assert!(!all.contains(claim), "{claim}: {all}");
    }
}

/// Failure: a refusal that cleared nothing doesn't say so, or one that is a partial result says "nothing was cleared"; the person is told the wrong state of
/// their Trash and their copies.
#[test]
fn a_refusal_says_nothing_was_cleared_and_a_stop_after_an_act_never_does() {
    for why in [
        ConfirmRefusal::Expired,
        ConfirmRefusal::Unknown,
        ConfirmRefusal::OtherTarget,
        ConfirmRefusal::Stale,
        ConfirmRefusal::PlanChanged,
        ConfirmRefusal::Full,
        ConfirmRefusal::Collision,
        ConfirmRefusal::Mismatched,
    ] {
        assert!(confirm_refusal(why).ends_with("Nothing was cleared."), "{why:?}");
    }
    for why in [Busy::Installer, Busy::Windowless, Busy::Starting, Busy::BesideNewer] {
        assert!(busy(why).ends_with("Nothing was cleared."), "{why:?}");
    }
    assert!(refusal(Refusal::TooLarge, false).ends_with("nothing was cleared."));
    let partly = stopped(&Did { trashed: 1, ..Did::default() }, 1, 3, "its folder changed");
    assert!(!partly.to_lowercase().contains("nothing was"), "{partly}");
    assert!(partly.contains("1 of 3"), "{partly}");
    assert!(stopped(&Did::default(), 0, 3, "its folder changed").contains("Nothing was cleared."));
}

/// Failure: a copy holding work of an earlier version is refused with the generic words, so the person is not told to quit and reopen to bring it in.
#[test]
fn a_copy_with_work_not_brought_in_says_to_reopen() {
    assert_eq!(refusal(Refusal::Incomplete(Store::LegacySession), true), WENT_BACK);
    assert_ne!(refusal(Refusal::Incomplete(Store::LegacySession), false), WENT_BACK);
    assert!(WENT_BACK.contains("Quit and reopen"));
}

/// Failure: with nothing to clear the check still offers a button, or hides what it left alone and why.
#[test]
fn nothing_to_clear_says_what_it_left_alone() {
    let left = Found {
        waiting_here: 2,
        no_trash: 1,
        ..Found::default()
    };
    assert!(left.is_empty());
    let words = check(&Names::default(), &left);
    assert!(words.starts_with("Nothing found to clear."), "{words}");
    assert!(words.contains("still being written") && words.contains("no Trash"), "{words}");
    assert!(!words.contains(BUTTON));
}

/// Failure (review round 1, 1): a stop that left a copy part deleted, or a save in doubt in the Trash, is told as nothing cleared.
#[test]
fn a_part_done_artifact_is_told_and_is_never_nothing() {
    use crate::removal_store::leftovers::act::Partly;
    for partly in [Partly::Copy, Partly::Save] {
        let did = Did {
            partly: Some(partly),
            ..Did::default()
        };
        assert!(!did.is_none());
        let said = stopped(&did, 0, 3, "the disk refused");
        assert!(!said.to_lowercase().contains("nothing was"), "{said}");
        assert!(said.contains("partly deleted") || said.contains("may be in the Trash"), "{said}");
    }
}

/// Failure (review round 1, 7): a save still being written in another folder is told as "in this folder", so the person looks for it where it isn't.
#[test]
fn a_waiting_save_is_said_to_be_in_this_folder_only_when_it_is() {
    let words = |found: Found| nothing(&found);
    let here = words(Found {
        waiting_here: 1,
        waiting_elsewhere: 0,
        waiting_folders: 0,
        ..Found::default()
    });
    assert!(here.contains("in this folder"), "{here}");
    let elsewhere = words(Found {
        waiting_elsewhere: 2,
        waiting_folders: 2,
        ..Found::default()
    });
    assert!(!elsewhere.contains("this folder") && elsewhere.contains("in 2 other folders"), "{elsewhere}");
    let both = words(Found {
        waiting_here: 1,
        waiting_elsewhere: 1,
        waiting_folders: 1,
        ..Found::default()
    });
    assert!(both.contains("in this folder") && both.contains("in 1 other folder."), "{both}");
}
