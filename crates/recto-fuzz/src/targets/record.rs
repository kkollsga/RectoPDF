//! The state record and journals, as whatever's on disk: they're hostile, whoever seems to have written them. Each input goes in as it is, and again with its checksums made good, since otherwise nearly nothing gets past them.
//!
//! What must hold: no panic, no bomb, caps hold, and what's accepted can be used: a session restored from it, and every step of a list undone and redone, and written back the same.

use crate::guard::{Budget, within};
use crate::targets::NamedLoader;
use recto_session::edit::{MAX_PAGES, PageEdits};
use recto_session::record::{self, CURRENT_VERSION, JOURNAL_KIND, MAIN_KIND, RecordError};
use recto_session::{RestoredEdits, Session};
use std::collections::HashMap;
use std::time::Duration;

const BUDGET: Budget = Budget { time: Duration::from_secs(120), memory: 1024 * 1024 * 1024 };

fn use_a_record(bytes: &[u8]) {
    let Ok(read) = record::decode_record(bytes, CURRENT_VERSION) else { return };
    assert!(read.record.tabs.len() <= recto_session::MAX_TABS);
    let mut session = Session::new(NamedLoader);
    session.restore(&read.record, HashMap::new()).expect("what validates can be restored");
    session.wake_active();
    // Writing it back gives what reads the same.
    let again = record::encode_record(&session.to_record(), read.seq);
    record::decode_record(&again, CURRENT_VERSION).expect("what the session writes reads");
    let _ = session.sidebar();
}

fn use_a_journal(bytes: &[u8]) {
    let parsed = match record::parse_journal(bytes, CURRENT_VERSION) {
        Ok(parsed) => parsed,
        Err(RecordError::Newer(_) | RecordError::Damaged(_)) => return,
    };
    assert!(parsed.good_bytes <= bytes.len() as u64);
    let Ok((mut edits, applied)) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops) else { return };
    assert!(applied <= parsed.ops.len());
    assert!(edits.slots().len() <= MAX_PAGES);
    // As good as a list made here: through a session, undone and redone all the way, and back to the same.
    let start = edits.slots().to_vec();
    let whole = edits.to_record();
    let mut back = PageEdits::from_record(&whole).expect("a list writes what reads");
    assert_eq!(back.to_record(), whole);
    let mut steps = 0;
    while edits.undo() {
        assert!(back.undo());
        assert_eq!(back.slots(), edits.slots());
        steps += 1;
    }
    for _ in 0..steps {
        assert!(edits.redo());
    }
    assert_eq!(edits.slots(), start.as_slice(), "redo undoes undo");
    let mut session = Session::new(NamedLoader);
    if let Ok(id) = session.open(std::path::Path::new("/fuzz/p12.pdf")) {
        let mut main = session.to_record();
        main.tabs[0].journal = Some(record::journal_file_name(None, std::path::Path::new("/fuzz/p12.pdf"), None));
        let name = main.tabs[0].journal.clone().unwrap();
        let list = RestoredEdits { edits, seq: applied as u64, lost_lines: parsed.lost_lines };
        session.restore(&main, HashMap::from([(name, list)])).expect("what a session wrote it can restore");
        let _ = session.page_source(id, 0);
        let _ = session.sidebar();
        session.discard_edits(id);
    }
}

fn use_a_log(bytes: &[u8]) {
    let mut main = recto_session::record::SessionRecord { app: record::AppRecord::default(), view: record::ViewRecord::default(), find: record::FindRecord::default(), edit_mode: false, active: None, next_id: 3, tabs: vec![record::TabEntry { id: 1, path: record::PathField::Text("/fuzz/p12.pdf".into()), file: None, page: 0, page_count: 12, place: None, view_rotation: 0, toc_open: true, renderer_only: false, selection: None, notice: None, journal: None, edits: None }] };
    record::apply_places_log(&mut main, 1, bytes);
    record::validate_record(&main).expect("a log can't break a record");
}

/// One input as a main record, a journal and a log, as it is and with what checks it made good.
pub fn check(data: &[u8]) {
    within("record", BUDGET, || {
        use_a_record(data);
        use_a_record(&record::seal(MAIN_KIND, 1, data));
        use_a_journal(data);
        use_a_log(data);
        // Each line of what's given, with its own checksum put right, under a good header.
        let header = record::JournalHeader::new(1, std::path::Path::new("/fuzz/p12.pdf"), None, 12, 0);
        let mut journal = record::journal_start(&header, None);
        let mut log = Vec::new();
        for line in data.split(|byte| *byte == b'\n') {
            journal.extend_from_slice(&record::journal_line(line));
            log.extend_from_slice(&record::journal_line(line));
        }
        use_a_journal(&journal);
        use_a_log(&log);
        let _ = JOURNAL_KIND;
    });
}
