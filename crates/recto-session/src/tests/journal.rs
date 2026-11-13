use super::*;

/// Failure: an unsaved edit depends on a step that's rolled off, so going past the limit and crashing brings back a different document. Also the step-by-step promise: past the limit, undoing goes back exactly as far as the limit, and at each point the document is what it was then. Long, as real use isn't: 3,000 steps, against the limit of 1,000.
#[test]
fn past_the_history_limit_the_document_is_still_exact() {
    const STEPS: u32 = 3000;
    let mut s = journaled(40);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let mut seen: Vec<Vec<edit::PageSlot>> = vec![
        (0..40)
            .map(|page| edit::PageSlot {
                source: 0,
                page,
                quarter_turns: 0,
            })
            .collect(),
    ];
    for step in 0..STEPS {
        let at = step % 39;
        match step % 3 {
            0 => s.edit_pages(a, |e| e.rotate(at, 1, 1)).unwrap(),
            1 => s.edit_pages(a, |e| e.move_pages(at, 1, 39 - at)).unwrap_or(()),
            // An agent's turn is refused while the owner's step is what would go, and that's a step not taken.
            _ => match s.edit_pages_as(a, Author::Agent, |e| e.rotate(39 - at, 1, 3)) {
                Ok(()) | Err(SessionError::Edit(EditError::HistoryFull)) => {}
                Err(error) => panic!("{error:?}"),
            },
        }
        seen.push(s.lists().get(a).unwrap().slots().to_vec());
    }
    assert!(s.lists().get(a).unwrap().history_dropped());
    // Crash here, with the journal as far as it's got, and compare the document exactly.
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    let mut back = journaled(40);
    back.restore(&s.to_record(), back_from(&files)).unwrap();
    assert_eq!(back.lists().edits_record(a), s.lists().edits_record(a), "the document and the rolled history");
    // Undoing goes back exactly the limit's worth, each stop as it was.
    let mut undone = 0;
    let mut at = seen.len() - 1;
    while back.undo_edit(a) {
        undone += 1;
        at -= 1;
        // A step that did nothing isn't a step, so what's being compared moves back past those.
        while seen[at] == seen[at + 1] {
            at -= 1;
        }
        assert_eq!(back.lists().get(a).unwrap().slots(), seen[at], "after {undone} undone");
    }
    assert_eq!(undone, edit::MAX_UNDO, "exactly the limit");
    assert!(back.lists().is_edited(a), "what rolled off stays in the document");
}

/// Failure: a journal grows without bound through undo and redo with no edit, or a checkpoint moved loses the document or the steps within the limit. A checkpoint is only a tighter way of writing the same thing: what comes back from it is the same list.
#[test]
fn moving_a_checkpoint_keeps_everything_within_the_limit() {
    let mut s = journaled(10);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    for step in 0..edit::MAX_UNDO as u32 + 50 {
        s.edit_pages(a, |e| e.rotate(step % 10, 1, 1)).unwrap();
    }
    assert!(s.undo_edit(a));
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    let old_bytes = files.values().next().unwrap().len();
    let name = files.keys().next().unwrap().clone();
    let checkpoint = s.lists().edits_record(a).unwrap();
    let seq = s.lists().get(a).unwrap().journal_seq();
    let header = JournalHeader::new(a.0, Path::new("/d/a.pdf"), None, 10, seq);
    files.insert(name, record::journal_start(&header, Some(&checkpoint)));
    let mut back = journaled(10);
    back.restore(&s.to_record(), back_from(&files)).unwrap();
    assert_eq!(back.lists().edits_record(a), s.lists().edits_record(a));
    assert!(back.lists().get(a).unwrap().can_redo());
    assert_eq!(back.lists().get(a).unwrap().journal_seq(), seq);
    assert!(files.values().next().unwrap().len() < old_bytes * 4, "no bigger than a few times what it was");
}

/// Failure: discarding leaves the journal behind, so what was removed stays recoverable, or a reopened PDF gets it back. Or a tab's edits undone back to the file, with no steps left, keep a file for nothing. Or a tab closed with history still to go back through loses it, which is what's kept for the PDF's next opening.
#[test]
fn a_journal_goes_when_its_edits_do() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let mut files = HashMap::new();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    write_journal(&mut s, &mut files);
    assert_eq!(files.len(), 1);
    s.discard_edits(a);
    assert!(s.journals().has_pending());
    write_journal(&mut s, &mut files);
    assert!(files.is_empty(), "gone with the discard");
    // An agent's step, undone: closes with no prompt, and its way forward stays for next time.
    s.edit_pages_as(a, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
    assert!(s.undo_edit(a));
    write_journal(&mut s, &mut files);
    s.close(a).unwrap();
    write_journal(&mut s, &mut files);
    assert_eq!(files.len(), 1, "kept for the PDF's next opening");
    let reopened = s.open(Path::new("/d/a.pdf")).unwrap();
    let (name, restored) = back_from(&files).into_iter().next().unwrap();
    s.take_up_history(reopened, &name, restored.edits, restored.seq);
    assert!(s.lists().get(reopened).unwrap().can_redo(), "picked up");
    // A journal has one owner: another tab asking for it gets nothing.
    let other = s.open(Path::new("/d/b.pdf")).unwrap();
    let (name, restored) = back_from(&files).into_iter().next().unwrap();
    s.take_up_history(other, &name, restored.edits, restored.seq);
    assert!(s.lists().get(other).is_none());
    assert!(s.redo_edit(reopened));
}

/// Failure (review B3): a tab closing drops what hadn't gone to its journal, while the file stays, so an undone edit comes back live on a later launch. It all goes out, marked as closing.
#[test]
fn closing_keeps_what_hasnt_gone_out() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.rotate(0, 1, 1)).unwrap();
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    assert!(s.undo_edit(a));
    s.close(a).unwrap();
    assert!(s.has_journal_work());
    let work = s.journal_work();
    assert_eq!(work.appends.len(), 1);
    assert!(work.appends[0].closing);
    assert_eq!(work.appends[0].ops, vec![JournalOp::Undo]);
    // The disk refused: it's asked for again.
    s.journal_outcome(JournalOutcome::default());
    let again = s.journal_work();
    assert_eq!(again.appends.len(), 1, "not lost");
    s.journal_outcome(JournalOutcome {
        written: vec![(a, again.appends[0].through, again.appends[0].name.clone())],
        ..JournalOutcome::default()
    });
    assert!(!s.has_journal_work());
}

/// Failure (review B6): one journal failing makes the others go out again, as nothing says they got there. Each goes by what happened to it.
#[test]
fn a_journal_failing_doesnt_send_the_others_again() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    let b = s.open(Path::new("/d/b.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.edit_pages(b, |e| e.rotate(0, 1, 1)).unwrap();
    let work = s.journal_work();
    let done = work.appends.iter().find(|append| append.tab == a).unwrap();
    s.journal_outcome(JournalOutcome {
        written: vec![(a, done.through, done.name.clone())],
        not_retired: Vec::new(),
    });
    let next = s.journal_work();
    assert_eq!(next.appends.len(), 1);
    assert_eq!(next.appends[0].tab, b);
}

/// Failure (review T10): a checkpoint taken just as a save is under way loses that it was, and the edits get laid on the saved file after a crash.
#[test]
fn a_checkpoint_keeps_a_save_in_progress() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    let target = FileStamp {
        size: 9,
        mtime_ns: 1,
        device: 1,
        inode: 2,
    };
    let keeping = kept(KEPT_A, 100);
    s.save_prepared(a, target, Some(&keeping));
    let mut files = HashMap::new();
    write_journal(&mut s, &mut files);
    let (header, base) = s.checkpoint_if_caught_up(a).unwrap();
    assert_eq!(header.saving, Some(target.into()));
    assert_eq!(header.keeping, Some(keeping.to_record()), "nor what it kept");
    let parsed = record::parse_journal(&record::journal_start(&header, Some(&base)), record::CURRENT_VERSION).unwrap();
    assert_eq!(parsed.header.saving, Some(target.into()));
}

/// **Failure (R3.5b): a page edit made through the handle a caller's closure gets isn't marked by who made it, or doesn't go to the journal**: an
/// agent's step taken for the owner's would be one an agent below Full could then push out of the history past the owner's-edits rule, and a step not
/// journaled is lost in a crash. The owner's step is the owner's, an agent's the agent's, each in the journal as it was made.
#[test]
fn a_step_through_the_edit_handle_is_its_authors_and_journaled() {
    let mut s = journaled(5);
    let a = s.open(Path::new("/d/a.pdf")).unwrap();
    s.edit_pages(a, |e| e.rotate(0, 1, 1)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.move_pages(0, 1, 2)).unwrap();
    s.edit_pages_as(a, Author::Agent, |e| e.delete(e.len() - 1, 1)).unwrap();
    let ops: Vec<JournalOp> = s.journal_work().appends.into_iter().flat_map(|append| append.ops).collect();
    assert_eq!(
        ops,
        vec![
            JournalOp::Rotate(0, 1, 1, record::AuthorName::Owner),
            JournalOp::Move(0, 1, 2, record::AuthorName::Agent),
            JournalOp::Delete(4, 1, record::AuthorName::Agent),
        ]
    );
    assert!(s.lists().get(a).is_some_and(edit::PageEdits::holds_owner_edits));
}
