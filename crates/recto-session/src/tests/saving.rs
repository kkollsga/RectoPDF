use super::*;

/// Failure (review round 2): the close a prompt's Save asked for goes ahead though an edit came in after the save, so that edit is lost unasked; or undone steps hold the close up after a Save that was asked to close. An edit or an undo since keeps the tab open with it standing; with none, it closes over undone steps.
#[test]
fn a_close_after_save_keeps_what_came_in_since() {
    let mut s = session(3);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(id, |edits| edits.rotate(0, 1, 1)).unwrap();
    s.saved(
        id,
        FileStamp {
            size: 2,
            mtime_ns: 2,
            device: 2,
            inode: 2,
        },
        3,
        None,
        None,
    );
    s.edit_pages(id, |edits| edits.rotate(1, 1, 1)).unwrap();
    assert_eq!(s.close_after_save(id).map(|closed| closed.is_some()), Ok(false), "an edit since: it stays");
    assert!(s.tabs().tab(id).is_some() && s.is_edited(id), "and the edit stands");
    s.undo_edit(id);
    s.undo_edit(id);
    assert!(s.is_edited(id), "an undo past the save is a change from the file too");
    assert_eq!(s.close_after_save(id).map(|closed| closed.is_some()), Ok(false));
    s.redo_edit(id);
    assert!(!s.is_edited(id) && s.has_undone_owner_steps(id), "back at the saved point, with a step undone");
    assert_eq!(
        s.close_after_save(id).map(|closed| closed.is_some()),
        Ok(true),
        "closes over the undone step, as the prompt said"
    );
    assert!(s.tabs().tab(id).is_none());
}

/// Failure (the owner's bench file, 2026-11-08): after a save the page view can't tell the new version of the file from any other, so it lays the pages out afresh and the zoom shown and the place change. The session says which version a save moved the tab on from while the tab's still on what that save wrote, and stops saying it once the file is anything else: a carry-over onto someone else's file would show it at the wrong size.
#[test]
fn a_save_says_which_version_it_moved_on_from_and_only_while_it_holds() {
    let stamp = |n: u64| FileStamp {
        size: n,
        mtime_ns: n as i64,
        device: 1,
        inode: 1,
    };
    let mut s = session(3);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    s.apply_file_status(id, FileStatus::Same(stamp(1)));
    assert_eq!(s.saved_from(id), None, "nothing's been saved");
    s.edit_pages(id, |edits| edits.rotate(0, 1, 1)).unwrap();
    s.saved(id, stamp(2), 3, None, None);
    assert_eq!(s.saved_from(id), Some(stamp(1)));
    s.saved(id, stamp(3), 3, None, None);
    assert_eq!(s.saved_from(id), Some(stamp(2)), "the last save's");
    s.apply_file_status(id, FileStatus::Changed(stamp(4)));
    assert_eq!(s.saved_from(id), None, "the file changed outside since");
    // However many tabs are saved, what's kept stays small.
    for n in 0..(MAX_SAVED_OVER as u64 * 3) {
        let other = s.open(Path::new(&format!("/d/o{n}.pdf"))).unwrap();
        s.apply_file_status(other, FileStatus::Same(stamp(100 + n)));
        s.saved(other, stamp(1000 + n), 3, None, None);
    }
    assert!(s.saves.remembered() <= MAX_SAVED_OVER);
}

/// Failure (review): an edit made while a save writes comes back reading as saved, with its turn not in the file. Every way of changing a tab's edits waits while it's saved; a save that fails lets go and forgets what it noted, and the edits stand.
#[test]
fn nothing_changes_a_tab_while_it_is_saved() {
    let mut s = session(3);
    let id = s.open(Path::new("/d/a.pdf")).unwrap();
    assert!(s.begin_save(id, false).unwrap().is_none(), "nothing unsaved, nothing to write");
    s.edit_pages(id, |edits| edits.rotate(0, 1, 1)).unwrap();
    s.edit_pages(id, |edits| edits.rotate(2, 1, 1)).unwrap();
    s.undo_edit(id);
    let snapshot = s.begin_save(id, false).unwrap().unwrap();
    assert_eq!((snapshot.slots.len(), snapshot.slots[0].quarter_turns, snapshot.file_pages), (3, 1, 3));
    assert!(snapshot.holds_owner_edits);
    assert_eq!(s.edit_pages(id, |edits| edits.rotate(1, 1, 1)), Err(SessionError::Saving));
    assert_eq!(s.edit_pages_as(id, Author::Agent, |edits| edits.delete(0, 1)), Err(SessionError::Saving));
    assert!(!s.undo_edit(id) && !s.redo_edit(id));
    assert_eq!(s.undo_edit_as(id, Author::Agent), Err(SessionError::Saving));
    assert_eq!(s.redo_edit_as(id, Author::Owner), Err(SessionError::Saving));
    assert!(!s.clear_history(id, ClearReason::Discard), "a discard waits too");
    assert_eq!(s.close(id), Err(SessionError::Saving));
    assert_eq!(s.begin_save(id, true).err(), Some(SessionError::Saving), "one save at a time");
    assert!(s.any_saving() && !s.can_go_dormant(id), "and it's held, so it can't go dormant");
    s.save_prepared(
        id,
        FileStamp {
            size: 9,
            mtime_ns: 9,
            device: 9,
            inode: 9,
        },
        None,
    );
    assert!(s.page_edits(id).unwrap().saving().is_some());
    s.end_save(id);
    assert!(!s.is_saving(id) && !s.any_saving());
    assert!(s.page_edits(id).unwrap().saving().is_none(), "what a failed save noted goes");
    assert!(s.is_edited(id), "and the edits stand");
    s.edit_pages(id, |edits| edits.rotate(1, 1, 1)).unwrap();
}

/// Failure (decision 25): a Save ends the history, so there's no changing your mind after it, or the tab still reads as edited, or undoing past the save reads as saved, or coming back reads as unsaved. A failed save, which calls nothing, changes nothing.
#[test]
fn a_save_marks_a_point_that_undo_goes_past_and_comes_back_to() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
    let mut files = HashMap::new();
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    let keeping = kept(KEPT_A, 100);
    assert!(s.page_edits(a).unwrap().needs_own_pages(), "the delete's way back names a page of this file");
    s.save_prepared(a, target, Some(&keeping));
    write_journal(&mut s, &mut files);
    let parsed = record::parse_journal(files.values().next().unwrap(), record::CURRENT_VERSION).unwrap();
    assert_eq!(parsed.ops.last(), Some(&JournalOp::Saving(target.into())), "said ahead of it, in the journal");
    assert!(
        matches!(parsed.ops.iter().rev().nth(1), Some(JournalOp::Keeping(said)) if said.kept),
        "and what's kept for it"
    );
    // A failed save: nothing's called, and all of it is as it was.
    assert!(s.is_edited(a) && s.page_edits(a).unwrap().can_undo());
    let before: Vec<_> = (0..4).map(|at| s.page_source(a, at).unwrap().page).collect();
    assert_eq!(s.saved(a, target, 4, None, Some(keeping)), 0, "nothing had to go");
    assert!(!s.is_edited(a), "it reads as saved");
    assert!(!s.sidebar().edited);
    assert_eq!(s.pages_in(a), 4);
    assert_eq!(s.tabs().own_stamp(a), Some(target));
    assert_eq!(s.tabs().tab(a).unwrap().page_count, 4);
    assert_eq!(before, [1, 2, 3, 4], "those were pages of the old file");
    assert_eq!(
        (0..4).map(|at| s.page_source(a, at).unwrap().page).collect::<Vec<_>>(),
        [0, 1, 2, 3],
        "and the new file has them in order"
    );
    assert!(s.page_edits(a).unwrap().can_undo(), "the history's still there");
    // Past it, one step at a time, and back.
    assert!(s.undo_edit(a));
    assert!(s.is_edited(a), "undone past the saved point, it's unsaved again");
    assert_eq!(
        s.page_source(a, 0).unwrap().quarter_turns,
        3,
        "the turn's in the file now, and undoing it is a turn back"
    );
    assert!(s.undo_edit(a));
    assert_eq!(s.pages_in(a), 5);
    let back = s.page_source(a, 0).unwrap();
    assert_eq!(
        (back.path.as_path(), back.page, back.added),
        (Path::new("/state/journals/kept-00000000000000aa.pdf"), 0, true),
        "the page the save took out comes from what was kept, not the new file"
    );
    assert_eq!(
        (s.page_source(a, 1).unwrap().path.as_path(), s.page_source(a, 1).unwrap().page),
        (Path::new("/d/a.pdf"), 0),
        "and the rest from the file"
    );
    assert!(s.is_edited(a));
    assert!(!s.undo_edit(a));
    assert!(s.redo_edit(a));
    assert!(s.is_edited(a));
    assert!(s.redo_edit(a));
    assert!(!s.is_edited(a), "back at the saved point, it reads as saved");
    assert_eq!((0..4).map(|at| s.page_source(a, at).unwrap().page).collect::<Vec<_>>(), [0, 1, 2, 3]);
    assert_eq!(s.page_source(a, 0).unwrap().quarter_turns, 0);
}

/// Failure: after a save, the journal's still named for the old file and its page numbers, or a crash loses the history, so a PDF opened again gets the wrong steps or none. The new journal has the whole list under the new file's name, and replays to exactly what's in memory, and more edits go on from it.
#[test]
fn a_saved_history_is_a_new_journal_that_comes_back_the_same() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.delete(1, 2)).unwrap();
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    let old = files.keys().next().unwrap().clone();
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    s.saved(a, target, 3, None, Some(kept(KEPT_A, 100)));
    assert!(
        s.has_retired_journals(),
        "it wants writing at once, as a crash would otherwise find the old one"
    );
    // Not written yet: the old one's still the only one, and still there.
    assert!(files.contains_key(&old));
    write_journal(&mut s, &mut files);
    assert!(!files.contains_key(&old), "it goes once the new one's there");
    assert_eq!(files.len(), 1);
    assert!(!s.has_retired_journals());
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    write_journal(&mut s, &mut files);
    let parsed = record::parse_journal(files.values().next().unwrap(), record::CURRENT_VERSION).unwrap();
    assert_eq!(parsed.header.file, Some(target.into()), "it's about the new file");
    assert_eq!(parsed.header.file_pages, 3);
    let (replayed, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
    assert_eq!(applied, parsed.ops.len());
    assert_eq!(replayed.to_record(), s.edits_record(a).unwrap());
    assert_eq!(replayed.is_edited(), s.is_edited(a));
}

/// Failure (decision 25): a step undoes wrongly because the pages it needs are gone from the file, or the app says nothing about undo stopping short. It rolls off, with everything older, as a step's way back depends on those after it, and the tab says so. What came after it stays, and nothing about the document changes.
#[test]
fn a_step_whose_pages_cant_be_kept_rolls_off_and_says_so() {
    let mut s = journaled(6);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.edit_pages(a, |e| e.delete(5, 1)).unwrap();
    s.edit_pages(a, |e| e.rotate(1, 1, 1)).unwrap();
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    assert_eq!(s.saved(a, target, 5, None, None), 2, "the delete, and the older rotation with it");
    assert!(!s.is_edited(a));
    assert_eq!(
        s.tabs().tab(a).unwrap().file.notice.as_ref().map(|notice| notice.code()),
        Some("history_rolled_off")
    );
    assert!(s.tab_notice_message(a).is_some_and(|words| words.contains("Undo")));
    assert_eq!(s.page_edits(a).unwrap().summary().undo, 1);
    assert!(s.undo_edit(a), "the newer step still works");
    assert!(!s.undo_edit(a), "and undo stops rather than undoing wrongly");
    assert_eq!(s.pages_in(a), 5);
    assert_eq!(s.page_source(a, 1).unwrap().quarter_turns, 3);
    assert!(s.redo_edit(a));
    assert!(!s.is_edited(a));
    // Going to another tab has said it.
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.tap(a).unwrap();
    assert!(s.tabs().tab(a).unwrap().file.notice.is_some(), "not yet");
    s.tap(b).unwrap();
    assert_eq!(s.tabs().tab(a).unwrap().file.notice, None);
    // Only a turn and a move in the history: no pages named, so no copy's needed, and nothing rolls off.
    let mut s = journaled(6);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.move_pages(0, 1, 2)).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    assert!(!s.page_edits(a).unwrap().needs_own_pages());
    assert_eq!(s.saved(a, target, 6, None, None), 0);
    assert_eq!(s.tabs().tab(a).unwrap().file.notice, None);
    assert!(s.undo_edit(a) && s.undo_edit(a) && s.is_edited(a));
}

/// Failure: copies kept for a PDF go past the limit a history has, over many saves. The oldest goes, with the steps that need it, and its file's let go of.
#[test]
fn copies_kept_stay_within_the_limit() {
    let mut s = journaled(10);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let big = edit::MAX_HISTORY_BYTES as u64 / 2 + 1;
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
    assert_eq!(s.saved(a, target, 9, None, Some(kept(KEPT_A, big))), 0);
    s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
    assert_eq!(s.page_edits(a).unwrap().kept_sources().count(), 1);
    assert_eq!(
        s.saved(a, target, 8, None, Some(kept(KEPT_B, big))),
        1,
        "two wouldn't fit, so the first's step goes"
    );
    let list = s.page_edits(a).unwrap();
    assert_eq!(
        list.kept_sources()
            .map(|(_, added)| added.path.file_name().unwrap().to_str().unwrap().to_string())
            .collect::<Vec<_>>(),
        [KEPT_B]
    );
    assert_eq!(list.summary().undo, 1);
    assert!(s.journal_work().retired.iter().any(|name| name == KEPT_A), "and its file goes");
}

/// Failure: Save As loses the history, or its way back names the new file, or the tab's not on the new file. The old file is still where it was, so that's where the pages come from, with no copy.
#[test]
fn save_as_takes_the_history_along() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    assert_eq!(s.saved(a, target, 4, Some(Path::new("/e/copy.pdf")), None), 0);
    assert_eq!(s.tabs().path_of(a).unwrap(), Path::new("/e/copy.pdf"));
    assert_eq!(&*s.tabs().tab(a).unwrap().name, "copy");
    assert!(!s.is_edited(a));
    assert!(s.undo_edit(a));
    assert_eq!(
        (s.page_source(a, 0).unwrap().path.as_path(), s.page_source(a, 0).unwrap().page),
        (Path::new("/d/a.pdf"), 0)
    );
    assert_eq!(
        (s.page_source(a, 1).unwrap().path.as_path(), s.page_source(a, 1).unwrap().page),
        (Path::new("/e/copy.pdf"), 0)
    );
    assert_eq!(s.page_edits(a).unwrap().kept_sources().count(), 0, "the old file isn't the history's to remove");
}

/// Failure: what was written isn't this list, as its length says, and the steps get laid on a file they're not about. All of it goes, and it says so.
#[test]
fn a_save_of_something_else_ends_the_history() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    assert_eq!(s.saved(a, target, 3, None, Some(kept(KEPT_A, 100))), 1);
    assert!(s.page_edits(a).is_none());
    assert_eq!(s.pages_in(a), 3);
    assert_eq!(
        s.tabs().tab(a).unwrap().file.notice.as_ref().map(|notice| notice.code()),
        Some("history_rolled_off")
    );
    assert_eq!(s.journal_work().retired.iter().filter(|name| *name == KEPT_A).count(), 1);
    // A tab with no list has nothing to carry, and a file kept for it goes.
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    assert_eq!(s.saved(b, target, 5, None, Some(kept(KEPT_B, 100))), 0);
    assert_eq!(s.journal_work().retired, [KEPT_B]);
}

/// Failure (decision 25): clearing only forgets a history, so its file stays on disk and what was removed can be got back; or unsaved work goes with it; or an agent or a changed file takes what shouldn't go. Each reason does what it's for, and the files go.
#[test]
fn clearing_a_history_removes_its_files_and_keeps_unsaved_work() {
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    // After a confirmed redaction, at the saved point: nothing's left.
    for reason in [
        ClearReason::Redaction,
        ClearReason::HiddenDataRemoval,
        ClearReason::UserAsked,
        ClearReason::Discard,
    ] {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        let mut files = HashMap::new();
        write_journal(&mut s, &mut files);
        s.saved(a, target, 4, None, Some(kept(KEPT_A, 100)));
        write_journal(&mut s, &mut files);
        assert!(s.page_edits(a).is_some_and(PageEdits::can_undo));
        assert_eq!(files.len(), 1);
        assert!(s.clear_history(a, reason));
        assert!(s.page_edits(a).is_none(), "{reason:?}");
        assert!(s.has_retired_journals());
        let work = s.journal_work();
        assert_eq!(
            work.retired.iter().filter(|name| !record::is_sleep_name(name)).count(),
            2,
            "the journal and the kept file: {reason:?}"
        );
        assert!(work.retired.iter().any(|name| name == KEPT_A));
        assert_eq!(
            work.retired.iter().any(|name| record::is_sleep_name(name)),
            matches!(reason, ClearReason::Redaction | ClearReason::HiddenDataRemoval),
            "a picture of what's removed goes with it: {reason:?}"
        );
        assert_eq!(s.to_record().tabs[0].journal, None);
    }
    // Unsaved edits and Clear Edit History: the page order stays, and the way back goes. The journal starts again in one step, with nothing in it that was only history.
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
    s.insert_pdf(a, 0, Path::new("/d/secret.pdf")).unwrap();
    assert!(s.undo_edit(a));
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    assert!(
        files.values().next().unwrap().windows(10).any(|bytes| bytes == b"secret.pdf"),
        "it was in the journal"
    );
    assert!(s.clear_history(a, ClearReason::UserAsked));
    assert!(s.is_edited(a), "unsaved work stays");
    assert_eq!(s.pages_in(a), 4);
    assert!(!s.page_edits(a).unwrap().can_undo() && !s.page_edits(a).unwrap().can_redo());
    assert_eq!(s.page_edits(a).unwrap().added_sources().len(), 0, "the added file was only history");
    write_journal(&mut s, &mut files);
    assert_eq!(files.len(), 1, "the same name, replaced");
    let bytes = files.values().next().unwrap();
    assert!(
        !bytes.windows(10).any(|bytes| bytes == b"secret.pdf"),
        "it's gone from the file, not only from memory"
    );
    let parsed = record::parse_journal(bytes, record::CURRENT_VERSION).unwrap();
    let (replayed, _) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
    assert_eq!(replayed.to_record(), s.edits_record(a).unwrap());
    let none = s.open(Path::new("/d/none.pdf")).unwrap();
    assert!(!s.clear_history(none, ClearReason::UserAsked), "nothing to clear");
    // A file changed outside: with only history, its steps don't apply, and they go, whichever way it was noticed.
    let now = FileStamp {
        size: 1,
        mtime_ns: 2,
        device: 1,
        inode: 3,
    };
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.saved(a, target, 5, None, None);
    assert!(s.page_edits(a).is_some_and(PageEdits::can_undo));
    s.apply_file_status(a, FileStatus::Changed(now));
    assert!(s.page_edits(a).is_none());
    assert_eq!(s.tabs().own_stamp(a), Some(now));
    assert_eq!(s.tabs().tab(a).unwrap().file.notice.as_ref().map(|notice| notice.code()), Some("file_changed"));
    // With unsaved edits, they stay and say so, as decision 23 has it.
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.apply_file_status(a, FileStatus::Changed(target));
    assert!(s.is_edited(a) && s.page_edits(a).unwrap().can_undo());
    assert_eq!(s.tabs().own_stamp(a), Some(now), "they're about the file they were made on");
}

/// Failure (decision 25): a history stays on a file changed outside the app, when a tab's woken with nothing else having looked, and its steps get laid on the wrong pages.
#[test]
fn a_file_changed_under_a_history_clears_it_on_waking() {
    let dir = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "recto-changed-history-");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("a.pdf");
    std::fs::write(&file, b"as it was").unwrap();
    let mut s = journaled(5);
    let a = s.open(&file).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    assert!(s.undo_edit(a), "so there's history and nothing unsaved");
    write_journal(&mut s, &mut HashMap::new());
    let b = s.open(&dir.join("b.pdf")).unwrap();
    s.tap(b).unwrap();
    std::fs::write(&file, b"another version altogether").unwrap();
    s.warm.clear_contents();
    s.tap(a).unwrap();
    assert!(s.page_edits(a).is_none());
    assert_eq!(s.tabs().tab(a).unwrap().file.notice.as_ref().map(|notice| notice.code()), Some("file_changed"));
    assert_eq!(
        s.journal_work().retired.iter().filter(|name| record::is_journal_name(name)).count(),
        1,
        "and its file goes"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Failure (review R2): a copy kept for a save that was never confirmed is named only by the note of that save, so clearing the history leaves it. It holds the whole old file for up to 30 days, and a later redaction wouldn't touch it.
#[test]
fn clearing_takes_a_copy_kept_for_an_unconfirmed_save() {
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    // With unsaved edits, so the list stays and its history goes; and with none, so the list goes too.
    for unsaved in [true, false] {
        let mut s = journaled(5);
        let a = s.open(Path::new("/d/a.pdf")).unwrap();
        s.edit_pages(a, |e| e.delete(0, 1)).unwrap();
        if !unsaved {
            assert!(s.undo_edit(a));
        }
        write_journal(&mut s, &mut HashMap::new());
        let keeping = kept(KEPT_A, 100);
        s.save_prepared(a, target, Some(&keeping));
        assert!(s.clear_history(a, ClearReason::UserAsked));
        assert!(s.journal_work().retired.iter().any(|name| name == KEPT_A), "unsaved: {unsaved}");
        // Once it's gone, the next journal's first line doesn't name it either.
        if let Some(list) = s.page_edits(a) {
            assert!(list.keeping().is_none());
        }
    }
}

/// Failure: a save under way when an agent's `session restore` replaced the session finishes on the restored tab that has its tab's number: the restored tab takes the old file's version, its edit list is rebased on a save it never made and reads as saved, or a new save of it begins while the old one's word can still land. A save belongs to the session it began in.
#[test]
fn a_save_left_under_way_by_a_restore_says_nothing_of_the_tab_with_its_number() {
    use super::restore::{another_session_with, replaced_stamp};
    let mut s = session(10);
    let old = s.open(Path::new("/d/old.pdf")).unwrap();
    s.edit_pages(old, |edits| edits.rotate(0, 1, 1)).unwrap();
    s.begin_save(old, false).unwrap().expect("something to save");
    s.restore(&another_session_with(1), HashMap::new()).unwrap();
    let restored = old;
    s.edit_pages(restored, |edits| edits.delete(2, 1)).unwrap();
    let list = s.edits_record(restored);
    let version = s.tabs().own_stamp(restored);
    assert_eq!(
        s.begin_save(restored, false).unwrap_err(),
        SessionError::Saving,
        "a new save waits for the old one's end"
    );
    // The old save finishes, as the saver of the replaced session goes on to.
    s.save_prepared(old, stamp_for(7), None);
    assert_eq!(s.saved(old, stamp_for(7), 9, None, None), 0);
    s.end_save(old);
    assert_eq!(s.tabs().own_stamp(restored), version, "the restored tab keeps its version");
    assert_eq!(s.edits_record(restored), list, "and its list, with nothing noted of a save");
    assert!(s.is_edited(restored), "and it's still unsaved");
    assert_eq!(s.saved_from(restored), None);
    assert!(!s.any_saving() && !s.saves.abandoned_has(old), "nothing's left marked as saving");
    assert!(
        s.begin_save(restored, false).unwrap().is_some(),
        "once the old save has ended, a new one begins"
    );
    s.saved(restored, replaced_stamp(1), 9, None, None);
    s.end_save(restored);
    assert_eq!(s.tabs().own_stamp(restored), Some(replaced_stamp(1)), "and its word is believed");
}

/// Failure: a restore that leaves more saves under way than are kept by name forgets the oldest, whose word is then believed of a restored tab; or the list of them grows without a cap.
#[test]
fn saves_left_under_way_past_the_cap_are_all_doubted() {
    use super::restore::another_session_with;
    let mut s = session(10);
    let ids: Vec<TabId> = (0..=saves::MAX_ABANDONED_SAVES)
        .map(|n| s.open(Path::new(&format!("/d/old-{n}.pdf"))).unwrap())
        .collect();
    for id in &ids {
        s.begin_save(*id, true).unwrap().expect("a Save As has something to save");
    }
    s.restore(&another_session_with(ids.len()), HashMap::new()).unwrap();
    assert_eq!(s.saves.abandoned_len(), saves::MAX_ABANDONED_SAVES, "the list holds its cap");
    for id in &ids {
        let version = s.tabs().own_stamp(*id);
        assert_eq!(s.saved(*id, stamp_for(99), 10, None, None), 0);
        assert_eq!(s.tabs().own_stamp(*id), version, "no old save is believed, the oldest or the newest");
        assert_eq!(s.begin_save(*id, true).unwrap_err(), SessionError::Saving);
    }
}

fn stamp_for(n: u64) -> FileStamp {
    FileStamp {
        size: n,
        mtime_ns: n as i64,
        device: 3,
        inode: n,
    }
}
