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
            ..Names::default()
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
    for why in [Busy::Installer, Busy::Windowless, Busy::Starting, Busy::BesideNewer, Busy::RemovalPending] {
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

/// Failure: a word the window's sheet shows is empty or a button's title repeats another's (two buttons that read the same), or a sentence a person reads
/// shows the command's name in code marks, which a person doesn't read.
#[test]
fn the_sheets_words_are_set_and_a_person_reads_no_code_marks() {
    let titles = [
        MENU_TITLE,
        OFFER_TITLE,
        LOOKED_TITLE,
        STOPPED_TITLE,
        REFUSED_TITLE,
        CANCEL,
        OK,
        LOOK_AGAIN,
        COULDNT,
        BUTTON,
    ];
    assert!(titles.iter().all(|word| !word.trim().is_empty()), "{titles:?}");
    let mut distinct = titles.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), titles.len(), "{titles:?}");
    assert!(!busy(Busy::Windowless).contains('`'), "{}", busy(Busy::Windowless));
}

fn with(others: &[&str], missing: u32, more: bool) -> Names {
    Names {
        document: Some("report.pdf".into()),
        folder: Some("Documents".into()),
        others: others.iter().map(|name| (*name).to_owned()).collect(),
        missing,
        more,
    }
}

fn whose(words: &str) -> Option<&str> {
    words.lines().find(|line| line.starts_with("Among them"))
}

/// Failure: the confirmation says nothing of other documents' crashed saves (the owner's rule: they're "listed by name"), says it of the document's own saves
/// (where there is nothing to say), lists a long list whole, shows a raw temporary's name for a document that isn't there, claims a count it didn't make (past
/// the cap), or an agent's words carry names. The person's has one line more, only when another document has a save.
#[test]
fn the_persons_confirmation_names_other_documents_saves_and_an_agents_names_none() {
    assert_eq!(whose(&check(&with(&[], 0, false), &found())), None, "only its own saves: nothing to add");
    assert_eq!(
        whose(&check(&with(&["other.pdf"], 0, false), &found())),
        Some("Among them is an unfinished save of “other.pdf”.")
    );
    assert_eq!(
        whose(&check(&with(&["a.pdf", "b.pdf"], 0, false), &found())),
        Some("Among them are unfinished saves of “a.pdf” and “b.pdf”.")
    );
    assert_eq!(
        whose(&check(&with(&["a.pdf", "b.pdf", "c.pdf"], 0, false), &found())),
        Some("Among them are unfinished saves of “a.pdf”, “b.pdf” and “c.pdf”.")
    );
    let many: Vec<String> = (0..15).map(|n| format!("d{n:02}.pdf")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let long = check(&with(&many, 0, false), &found());
    assert!(
        whose(&long)
            .unwrap()
            .ends_with("“d00.pdf”, “d01.pdf”, “d02.pdf”, “d03.pdf”, “d04.pdf”, “d05.pdf”, “d06.pdf”, “d07.pdf”, “d08.pdf”, “d09.pdf” and 5 more."),
        "{long}"
    );
    // Past the cap the count isn't known: "and more", never a number that undercounts.
    let cut = check(&with(&many, 0, true), &found());
    assert!(whose(&cut).unwrap().ends_with("“d09.pdf” and more."), "{cut}");
    let missing = check(&with(&["other.pdf"], 1, false), &found());
    assert_eq!(
        whose(&missing),
        Some("Among them are unfinished saves of “other.pdf” and of 1 document that isn't there.")
    );
    let only_missing = check(&with(&[], 2, false), &found());
    assert_eq!(whose(&only_missing), Some("Among them are unfinished saves of 2 documents that aren't there."));
    let agent = check(&Names::default(), &found());
    assert_eq!(whose(&agent), None, "{agent}");
    assert_eq!(agent.lines().count() + 1, long.lines().count(), "one line more for the person");
}

/// Failure: the first line is the full removal's ("Before RectoPDF can remove anything…") though nothing is being removed; or it differs for an agent.
#[test]
fn the_opening_line_says_what_was_left_behind_for_the_person_and_an_agent_alike() {
    for names in [with(&[], 0, false), Names::default()] {
        let said = check(&names, &found());
        assert_eq!(said.lines().next(), Some("RectoPDF left these behind:"), "{said}");
        assert!(!said.contains("Before RectoPDF can remove"), "{said}");
    }
}

/// Failure: a file name forges the sheet: a curly quote inside it ends the quoted name early and what follows reads as RectoPDF's words, or a very long
/// name fills the sheet.
#[test]
fn a_name_cannot_close_its_quotes_and_a_long_one_is_cut() {
    let said = check(&with(&["x” and RectoPDF says all is well “y.pdf"], 0, false), &found());
    let line = whose(&said).expect("a line");
    assert_eq!(line.matches('“').count(), 1, "{line}");
    assert_eq!(line.matches('”').count(), 1, "{line}");
    let long = "n".repeat(200);
    let said = check(&with(&[long.as_str()], 0, false), &found());
    let line = whose(&said).expect("a line");
    assert!(line.chars().count() < 120 && line.contains("…”"), "{line}");
}

/// Failure: a long name is cut in the middle of a character the eye sees as one: its accent dropped, a flag left as one letter, an emoji joined to the one
/// that is cut away.
#[test]
fn a_long_name_is_cut_between_characters_the_eye_sees_not_inside_one() {
    let line = |name: &str| {
        let names = Names {
            others: vec![name.to_owned()],
            ..Names::default()
        };
        whose_saves(&names).expect("a line")
    };
    let accent = line(&format!("{}e\u{301}xyz", "a".repeat(59)));
    assert!(accent.contains(&format!("“{}…”", "a".repeat(59))), "{accent}");
    let flag = line(&format!("{}\u{1F1F3}\u{1F1F4}xyz", "a".repeat(59)));
    assert!(flag.contains(&format!("“{}…”", "a".repeat(59))), "{flag}");
    let joined = line(&format!("{}\u{1F468}\u{200D}\u{1F469}xyz", "a".repeat(59)));
    assert!(joined.contains(&format!("“{}…”", "a".repeat(59))), "{joined}");
    // A whole flag that fits is kept.
    let kept = line(&format!("{}\u{1F1F3}\u{1F1F4}xyz", "a".repeat(58)));
    assert!(kept.contains("\u{1F1F3}\u{1F1F4}…”"), "{kept}");
}
