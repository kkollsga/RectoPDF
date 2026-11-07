//! Fixed samples of version 1's files, as they're written today, so a change can't quietly break reading what an older build left. A new version gets samples of its own, and these stay.

use recto_session::Author;
use recto_session::edit::PageEdits;
use recto_session::record::{self, CURRENT_VERSION, FileNotice, FocusView, PathField, RecordError};

const RECORD: &[u8] = include_bytes!("fixtures/record-v1.json");
const JOURNAL: &[u8] = include_bytes!("fixtures/journal-v1.log");

/// Failure: a change to the format means a record written by the version people have can't be read by the next, so an update loses everything. This is one, byte for byte, and everything in it is checked.
#[test]
fn a_version_1_record_still_reads() {
    let read = record::decode_record(RECORD, CURRENT_VERSION).expect("version 1 reads");
    assert_eq!((read.version, read.seq), (1, 7));
    let saved = read.record;
    assert_eq!(saved.tabs.len(), 2);
    assert_eq!(saved.active, Some(2));
    assert_eq!(saved.next_id, 3);
    assert!(saved.edit_mode);
    assert_eq!((saved.find.open, saved.find.query.as_str(), saved.find.draft.as_str()), (true, "ran this", "typed but not run"));
    assert_eq!(saved.view.paper, Some(record::PaperName::Warm));
    assert!(saved.view.agent_icon_hidden);
    let app = &saved.app;
    assert_eq!(app.appearance, record::Appearance::Dark);
    assert_eq!(app.sidebar_width, Some(318.0));
    assert!(app.panel_open);
    assert_eq!(app.focus.view, FocusView::Find);
    assert!(!app.focus.app_frontmost);
    let window = app.window.as_ref().unwrap();
    assert_eq!((window.frame.x, window.frame.width), (100.0, 1300.0));
    assert_eq!(window.screen.as_ref().unwrap().name, "Built-in");
    assert_eq!(app.contents_scroll.unwrap().y, 280.0);
    let tab = &saved.tabs[0];
    assert_eq!(tab.path, PathField::Text("/docs/report.pdf".into()));
    assert_eq!(tab.file.unwrap().inode, 424_242);
    let place = tab.place.unwrap();
    assert_eq!((place.page, place.y, place.zoom, place.x), (39, 38_000.5, 1.7, 12.5));
    assert_eq!(tab.view_rotation, 1);
    assert!(!tab.toc_open);
    assert_eq!(tab.selection.unwrap().to_char, 20);
    assert_eq!(tab.journal.as_deref(), Some("journal-00112233445566ff.log"));
    assert!(tab.edits.unwrap().owner_steps_undone);
    assert_eq!(saved.tabs[1].notice, Some(FileNotice::Moved { from: PathField::Text("/old/moved.pdf".into()) }));
    assert!(saved.tabs[1].renderer_only);
}

/// Failure: the same, for a journal: what's in one today is the only copy of someone's unsaved edits. It replays to the same pages, with the same steps undone and whose they are.
#[test]
fn a_version_1_journal_still_replays() {
    let parsed = record::parse_journal(JOURNAL, CURRENT_VERSION).expect("version 1 reads");
    assert_eq!(parsed.ops.len(), 8);
    assert!(!parsed.lost_lines);
    let (edits, applied) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops).unwrap();
    assert_eq!(applied, 8);
    assert_eq!(edits.len(), 61, "60, and 2 added, and 1 deleted");
    assert_eq!(edits.added_sources().len(), 1);
    assert!(edits.holds_owner_edits());
    assert!(edits.owner_steps_undone(), "the owner's move is undone");
    assert!(edits.can_redo() && edits.can_undo());
    let _ = Author::Owner;
}

/// Failure: a later format is taken for this one. Whatever's in it, it's only recognised.
#[test]
fn a_later_version_is_only_recognised() {
    let later = record::seal_as(record::MAIN_KIND, CURRENT_VERSION + 1, 1, RECORD);
    assert_eq!(record::decode_record(&later, CURRENT_VERSION).unwrap_err(), RecordError::Newer(CURRENT_VERSION + 1));
    let mut journal = String::from_utf8(JOURNAL.to_vec()).unwrap();
    let first_end = journal.find('\n').unwrap();
    let header = journal[9..first_end].replace("\"version\":1", &format!("\"version\":{}", CURRENT_VERSION + 1));
    journal.replace_range(..first_end + 1, std::str::from_utf8(&record::journal_line(header.as_bytes())).unwrap());
    assert!(matches!(record::parse_journal(journal.as_bytes(), CURRENT_VERSION), Err(RecordError::Newer(_))));
}
