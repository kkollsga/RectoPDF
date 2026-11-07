//! The state record and journals, as whatever's on disk: they're hostile, whoever seems to have written them. Each input goes in as it is, and again with its checksums made good, since otherwise nearly nothing gets past them.
//!
//! What must hold: no panic, no bomb, caps hold, and what's accepted can be used: a session restored from it, and every step of a list undone and redone, and written back the same.

use crate::guard::{Budget, within};
use crate::targets::NamedLoader;
use recto_control::StatePaths;
use recto_control::record_store::{Flavor, Loaded, RecordStore, notice_for};
use recto_platform::local;
use recto_session::edit::{MAX_PAGES, PageEdits};
use recto_session::record::{self, CURRENT_VERSION, MAIN_KIND, PlaceDelta, RecordError};
use recto_session::{RestoredEdits, Session};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

const BUDGET: Budget = Budget { time: Duration::from_secs(240), memory: 1024 * 1024 * 1024 };

fn use_a_record(bytes: &[u8]) {
    let Ok(read) = record::decode_record(bytes, CURRENT_VERSION) else { return };
    assert!(read.record.tabs.len() <= recto_session::MAX_TABS);
    let mut session = Session::new(NamedLoader);
    session.restore(&read.record, HashMap::new()).expect("what validates can be restored");
    session.wake_active();
    // What a person does next, on each tab: nothing a record says can make that panic or allocate past a cap (review B9).
    for id in session.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>().into_iter().take(4) {
        let _ = session.edit_pages(id, |edits| edits.rotate(0, 1, 1));
        let _ = session.go_to(id, u32::MAX);
        let _ = session.undo_edit(id);
    }
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
    // A save of whatever this is, and clearing it: each has to leave a list that reads back.
    let mut saved = edits.clone();
    let pages = saved.len();
    if saved.rebase_on_saved(pages, None).is_some() {
        PageEdits::from_record(&saved.to_record()).expect("a list after a save reads back");
    }
    let mut session = Session::new(NamedLoader);
    if let Ok(id) = session.open(std::path::Path::new("/fuzz/p12.pdf")) {
        let mut main = session.to_record();
        main.tabs[0].journal = Some(record::journal_file_name(None, std::path::Path::new("/fuzz/p12.pdf"), None));
        let name = main.tabs[0].journal.clone().unwrap();
        let list = RestoredEdits { edits, seq: applied as u64, lost_lines: parsed.lost_lines, rolled_off: false };
        session.restore(&main, HashMap::from([(name, list)])).expect("what a session wrote it can restore");
        let _ = session.page_source(id, 0);
        let _ = session.sidebar();
        session.discard_edits(id);
    }
}

/// A sleep file: refused, or one that's within every cap, and reads back as it was once it's been written again.
fn use_a_sleep_file(bytes: &[u8]) {
    let Ok(sleep) = recto_session::sleep::decode_sleep(bytes, CURRENT_VERSION) else { return };
    assert!(sleep.pictures.len() <= recto_session::sleep::MAX_SLEEP_PICTURES);
    for picture in &sleep.pictures {
        assert!(picture.bgra.len() <= recto_session::sleep::MAX_PICTURE_BYTES);
        assert_eq!(picture.bgra.len(), picture.width as usize * picture.height as usize * 4);
        assert!(picture.page < sleep.pages);
    }
    let again = recto_session::sleep::decode_sleep(&recto_session::sleep::encode_sleep(&sleep), CURRENT_VERSION).expect("what was read can be written");
    assert_eq!(again.pictures.len(), sleep.pictures.len());
}

fn use_a_log(bytes: &[u8]) {
    let mut main = recto_session::record::SessionRecord { app: record::AppRecord::default(), view: record::ViewRecord::default(), find: record::FindRecord::default(), edit_mode: false, active: None, next_id: 3, tabs: vec![record::TabEntry { id: 1, path: record::PathField::Text("/fuzz/p12.pdf".into()), file: None, page: 0, page_count: 12, place: None, view_rotation: 0, toc_open: true, renderer_only: false, selection: None, notice: None, journal: None, edits: None, generated_contents: false }] };
    record::apply_places_log(&mut main, 1, bytes);
    record::validate_record(&main).expect("a log can't break a record");
}

static NEXT_FOLDER: AtomicU32 = AtomicU32::new(0);

/// A state folder of its own, for each input. Under `RECTO_FUZZ_STATE` if there's one, as `tools/fuzz.sh` clears that up.
fn folder() -> PathBuf {
    let base = std::env::var_os("RECTO_FUZZ_STATE").map_or_else(|| std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()), PathBuf::from);
    base.join(format!("rec{}-{}", std::process::id(), NEXT_FOLDER.fetch_add(1, Ordering::Relaxed)))
}

/// The store itself, on what's on disk: whichever way the input lands, a launch comes back with something or says why, an edit made after it goes out and reads back, and nothing's left broken for the launch after.
fn use_a_store(record_bytes: &[u8], journal_bytes: &[u8], log_bytes: &[u8]) {
    let root = folder();
    let Ok(paths) = StatePaths::overridden(&root) else { return };
    if paths.prepare().is_err() {
        return;
    }
    let session_dir = root.join("session");
    let name = record::journal_file_name(None, Path::new("/fuzz/p12.pdf"), None);
    let _ = local::ensure_private_dir(&session_dir);
    let _ = local::ensure_private_dir(&session_dir.join("journals"));
    let _ = local::write_private_file(&session_dir.join("record.a.json"), record_bytes);
    let _ = local::write_private_file(&session_dir.join("places.log"), log_bytes);
    let _ = local::write_private_file(&session_dir.join("journals").join(&name), journal_bytes);
    for _ in 0..2 {
        let store = RecordStore::open(&paths, Flavor::App);
        let loaded = store.load();
        let _ = notice_for(&loaded);
        let mut session = Session::new(NamedLoader);
        session.set_journaling(true);
        if let Loaded::Restored(restored) = loaded {
            assert!(session.restore(&restored.record, restored.journals).is_ok(), "what a load gives can be restored");
            let first = session.tabs().first().map(|tab| tab.id);
            if let Some(id) = first {
                let _ = session.edit_pages(id, |edits| edits.rotate(0, 1, 1));
                let _ = session.undo_edit(id);
            }
        }
        let work = session.journal_work();
        let (outcome, _) = store.write_journals(&work, false);
        session.journal_outcome(outcome);
        let saved = session.to_record();
        if store.write_record(&saved, &PlaceDelta::default(), false).is_ok() {
            let lists = session.lists_by_journal();
            store.next_write_whole();
            let _ = store.write_record(&saved, &PlaceDelta::default(), false);
            assert_eq!(store.verify(&saved, &lists), Ok(()), "what's written reads back");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// One input as a main record, a journal and a log, as it is and with what checks it made good.
pub fn check(data: &[u8]) {
    within("record", BUDGET, || {
        use_a_record(data);
        use_a_record(&record::seal(MAIN_KIND, 1, data));
        use_a_journal(data);
        use_a_log(data);
        use_a_sleep_file(data);
        use_a_sleep_file(&record::seal(recto_session::sleep::SLEEP_KIND, 0, data));
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
        use_a_store(&record::seal(MAIN_KIND, 1, data), &journal, &log);
        use_a_store(data, data, data);
    });
}
