//! The state record and journals, and the activity logs `recto log` reads, as whatever's on disk: they're hostile, whoever seems to have written them. Each input goes in as it is, and again with its checksums made good, since otherwise nearly nothing gets past them.
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
use std::time::Duration;

const BUDGET: Budget = Budget {
    time: Duration::from_secs(240),
    memory: 1024 * 1024 * 1024,
};

fn use_a_record(bytes: &[u8]) {
    let Ok(read) = record::decode_record(bytes, CURRENT_VERSION) else { return };
    assert!(read.record.tabs.len() <= recto_session::MAX_TABS);
    let mut session = Session::new(NamedLoader);
    session.restore(&read.record, HashMap::new()).expect("what validates can be restored");
    // Nothing the record named comes back with its place given, and a list that couldn't be read withholds every tab's (the fourth review's item 4).
    let named = &read.record.withheld_places;
    let names = |id: u32| named.is_every() || named.ids().contains(&id);
    assert!(
        session
            .tabs()
            .iter()
            .all(|tab| !names(tab.id.0) || recto_session::compare::place_withheld(&session, tab.id))
    );
    session.wake_active();
    // What a person does next, on each tab: nothing a record says can make that panic or allocate past a cap (review B9).
    for id in session.tabs().iter().map(|tab| tab.id).collect::<Vec<_>>().into_iter().take(4) {
        let _ = session.edit_pages(id, |edits| edits.rotate(0, 1, 1));
        let _ = session.go_to(id, u32::MAX);
        let _ = session.undo_edit(id);
    }
    // Writing it back gives what reads the same, and withholds no tab's place it didn't name (the third review, D-C: a list read as hostile), each by its
    // id: the session never writes a list as unreadable.
    let written = session.to_record();
    assert!(!written.withheld_places.is_every() && written.withheld_places.ids().iter().all(|id| names(*id)));
    let again = record::encode_record(&written, read.seq);
    record::decode_record(&again, CURRENT_VERSION).expect("what the session writes reads");
    let _ = session.sidebar();
}

fn use_a_journal(bytes: &[u8]) {
    let parsed = match record::parse_journal(bytes, record::MARKED_JOURNAL_VERSION) {
        Ok(parsed) => parsed,
        Err(RecordError::Newer(_) | RecordError::Damaged(_)) => return,
    };
    assert!(parsed.good_bytes <= bytes.len() as u64);
    let Ok((mut edits, applied)) = PageEdits::replay(parsed.header.file_pages, parsed.base.as_ref(), &parsed.ops) else {
        return;
    };
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
    if saved.rebase_on_saved(pages, None, None).is_some() {
        PageEdits::from_record(&saved.to_record()).expect("a list after a save reads back");
    }
    let mut session = Session::new(NamedLoader);
    if let Ok(id) = session.open(std::path::Path::new("/fuzz/p12.pdf")) {
        let mut main = session.to_record();
        main.tabs[0].journal = Some(record::journal_file_name(None, std::path::Path::new("/fuzz/p12.pdf"), None));
        let name = main.tabs[0].journal.clone().unwrap();
        let list = RestoredEdits {
            edits,
            seq: applied as u64,
            lost_lines: parsed.lost_lines,
            rolled_off: false,
            set_aside_beside: false,
        };
        session
            .restore(&main, HashMap::from([(name, list)]))
            .expect("what a session wrote it can restore");
        let _ = session.page_source(id, 0);
        let _ = session.sidebar();
        session.discard_edits(id);
    }
}

/// A sleep file: refused, or one that's within every cap, and reads back as it was once it's been written again.
fn use_a_sleep_file(bytes: &[u8]) {
    let Ok(sleep) = recto_session::sleep::decode_sleep(bytes, CURRENT_VERSION) else {
        return;
    };
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
    let mut main = recto_session::record::SessionRecord {
        app: record::AppRecord::default(),
        view: record::ViewRecord::default(),
        find: record::FindRecord::default(),
        edit_mode: false,
        active: None,
        next_id: 3,
        tabs: vec![record::TabEntry {
            id: 1,
            path: record::PathField::Text("/fuzz/p12.pdf".into()),
            file: None,
            page: 0,
            page_count: 12,
            place: None,
            view_rotation: 0,
            toc_open: true,
            layout: Default::default(),
            renderer_only: false,
            selection: None,
            notice: None,
            journal: None,
            edits: None,
            generated_contents: false,
            contents_folds: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
            password: false,
        }],
        comparison: None,
        recent: Vec::new(),
        withheld_places: record::WithheldPlaces::default(),
    };
    record::apply_places_log(&mut main, 1, bytes);
    record::validate_record(&main).expect("a log can't break a record");
}

/// A state folder of its own, for each input. Under `RECTO_FUZZ_STATE` if there's one, as `tools/fuzz.sh` clears that up.
fn folder() -> recto_testpdf::TempDir {
    let base = std::env::var_os("RECTO_FUZZ_STATE").map_or_else(|| std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir()), PathBuf::from);
    recto_testpdf::TempDir::new_in(&base, "rec")
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
    // And again under another name: two journals with the same tab's id and path, a linked pair as a follow's crash leaves them (slice 2's
    // `record_store::followed`), so whichever way the input lands, the pair is judged and nothing comes back twice.
    let again = record::journal_file_name(None, Path::new("/fuzz/p12.pdf"), Some(2));
    let _ = local::write_private_file(&session_dir.join("journals").join(&again), journal_bytes);
    for _ in 0..2 {
        let store = RecordStore::open(&paths, Flavor::App);
        let loaded = store.load();
        let _ = notice_for(&loaded);
        let mut session = Session::new(NamedLoader);
        session.journals_mut().set_journaling(true);
        if let Loaded::Restored(restored) = loaded {
            assert!(
                session.restore(&restored.record, restored.journals).is_ok(),
                "what a load gives can be restored"
            );
            let first = session.tabs().first().map(|tab| tab.id);
            if let Some(id) = first {
                let _ = session.edit_pages(id, |edits| edits.rotate(0, 1, 1));
                let _ = session.undo_edit(id);
            }
        }
        let work = session.journal_desk().work();
        let (outcome, _) = store.write_journals(&work, false);
        session.journal_desk().outcome(outcome);
        let saved = session.to_record();
        if store.write_record(&saved, &PlaceDelta::default(), false).is_ok() {
            let lists = session.journal_look().lists_by_journal();
            store.next_write_whole();
            let _ = store.write_record(&saved, &PlaceDelta::default(), false);
            assert_eq!(store.verify(&saved, &lists), Ok(()), "what's written reads back");
        }
    }
    // `recto log`, on activity logs that are whatever's on disk: as many entries as asked for, and every word of ours cleaned to one line.
    let _ = local::write_private_file(&paths.log, log_bytes);
    let _ = local::write_private_file(&paths.other_log, record_bytes);
    for last in [None, Some(1), Some(u32::MAX)] {
        let recent = recto_control::log::recent(&paths, last);
        let entries = recent["entries"].as_array().expect("entries are a list");
        assert!(entries.len() <= last.map_or(50, |last| last as usize));
        for entry in entries {
            for key in ["cmd", "level", "outcome"] {
                let word = entry[key].as_str().expect("a word");
                assert_eq!(recto_core::clean_line(word), word, "{key} isn't clean: {word:?}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// **Two journals of one tab, their headers from the input** (slice 2's crash rule, `record_store::followed`, review M2): the second journal's tab id, path,
/// version (none, the record's, the file as it is now, or a third), where it started again from a whole list (up or down from the first's steps) and
/// its own steps; the file replaced by another program or not; and the record saying it changed or not. What must hold: a journal with unsaved edits
/// never leaves the folder (it's a tab's list, or set aside under its `.not-merged` name), and one set aside with edits not all in the list kept
/// (by sequence) that isn't the old list a follow superseded leaves its notice on the tab, which is never written into the record.
fn use_a_linked_pair(data: &[u8]) {
    use recto_platform::FileStamp;
    use recto_session::record::{AuthorName, JournalHeader, JournalOp};
    let byte = |at: usize| data.get(at).copied().unwrap_or(0);
    let root = folder();
    let Ok(paths) = StatePaths::overridden(&root) else { return };
    if paths.prepare().is_err() {
        return;
    }
    let folder = root.join("session").join("journals");
    let _ = local::ensure_private_dir(&root.join("session"));
    let _ = local::ensure_private_dir(&folder);
    let file = root.join("doc.pdf");
    if std::fs::write(&file, b"the version the edits were made on").is_err() {
        return;
    }
    let Ok(first) = recto_platform::file_stamp(&file) else { return };
    let now = if byte(0) & 1 == 1 {
        let new = file.with_extension("new");
        let _ = std::fs::write(&new, b"another program's version, longer than the first");
        let _ = std::fs::rename(&new, &file);
        recto_platform::file_stamp(&file).unwrap_or(first)
    } else {
        first
    };
    let third = FileStamp {
        size: first.size.wrapping_add(7),
        ..first
    };
    let turn = JournalOp::Rotate(0, 1, 1, AuthorName::Owner);
    // (tab, path, version, where it started again, the whole list's turns then, the turns after)
    type Shape = (u32, PathBuf, Option<FileStamp>, u64, usize, usize);
    let journal = |(tab, path, stamp, base_seq, base_turns, turns): &Shape| -> Vec<u8> {
        let header = JournalHeader::new(*tab, path, *stamp, 12, *base_seq);
        let base = (*base_turns > 0)
            .then(|| PageEdits::replay(12, None, &vec![turn.clone(); *base_turns]).ok().map(|(list, _)| list.to_record()))
            .flatten();
        let mut bytes = record::journal_start(&header, base.as_ref());
        bytes.extend_from_slice(&record::journal_lines(&vec![turn.clone(); *turns]));
        bytes
    };
    let own: Shape = (1, file.clone(), Some(first), u64::from(byte(1) % 4), usize::from(byte(2) % 3), usize::from(byte(3) % 3));
    let other: Shape = (
        if byte(4) & 1 == 0 { 1 } else { 2 },
        if byte(4) & 2 == 0 { file.clone() } else { root.join("other.pdf") },
        match byte(5) % 4 {
            0 => None,
            1 => Some(first),
            2 => Some(now),
            _ => Some(third),
        },
        u64::from(byte(6) % 6),
        usize::from(byte(7) % 3),
        usize::from(byte(8) % 3),
    );
    let own_name = record::journal_file_name(Some(first), &file, None);
    let other_name = record::journal_file_name(other.2, &other.1, Some(2));
    let store = RecordStore::open(&paths, Flavor::App);
    let mut tab = record::TabEntry {
        id: 1,
        path: record::PathField::from_path(&file),
        file: Some(first.into()),
        page: 0,
        page_count: 12,
        place: None,
        view_rotation: 0,
        toc_open: true,
        layout: Default::default(),
        renderer_only: false,
        selection: None,
        notice: None,
        journal: Some(own_name.clone()),
        edits: None,
        generated_contents: false,
        contents_folds: Vec::new(),
        back: Vec::new(),
        forward: Vec::new(),
        password: false,
    };
    if byte(9) & 1 == 1 {
        tab.notice = Some(record::FileNotice::Changed);
    }
    let main = recto_session::record::SessionRecord {
        app: record::AppRecord::default(),
        view: record::ViewRecord::default(),
        find: record::FindRecord::default(),
        edit_mode: false,
        active: Some(1),
        next_id: 3,
        tabs: vec![tab],
        comparison: None,
        recent: Vec::new(),
        withheld_places: record::WithheldPlaces::default(),
    };
    if store.write_record(&main, &PlaceDelta::default(), false).is_err() {
        return;
    }
    for (name, shape) in [(&own_name, &own), (&other_name, &other)] {
        let _ = local::write_private_file(&folder.join(name), &journal(shape));
    }
    drop(store);
    let Loaded::Restored(restored) = RecordStore::open(&paths, Flavor::App).load() else {
        let _ = std::fs::remove_dir_all(&root);
        return;
    };
    // Each journal's steps and edits, as written.
    let steps = |shape: &Shape| -> (u64, bool) {
        let base = (shape.4 > 0)
            .then(|| PageEdits::replay(12, None, &vec![turn.clone(); shape.4]).ok().map(|(list, _)| list.to_record()))
            .flatten();
        let edited = PageEdits::replay(12, base.as_ref(), &vec![turn.clone(); shape.5]).is_ok_and(|(list, _)| list.is_edited());
        (shape.3 + shape.5 as u64, edited)
    };
    let is_now = |stamp: Option<FileStamp>| stamp.is_some_and(|stamp| stamp.same_contents(now) && stamp.inode == now.inode);
    let set_aside = |name: &str| folder.join(name).with_extension("not-merged").exists();
    for (name, shape) in [(&own_name, &own), (&other_name, &other)] {
        let (seq, edited) = steps(shape);
        if !edited {
            continue;
        }
        assert!(
            restored.journals.contains_key(name.as_str()) || set_aside(name),
            "a journal with unsaved edits is a tab's or set aside: {name}"
        );
        if restored.journals.contains_key(name.as_str()) {
            continue;
        }
        // Set aside: the list kept for tab 1 says so, unless every step of this is in it, or it's the old list a follow superseded.
        let kept = restored.record.tabs.iter().find(|tab| tab.id == 1).and_then(|tab| tab.journal.clone());
        let Some((kept_name, list)) = kept.as_ref().and_then(|kept| Some((kept, restored.journals.get(kept)?))) else {
            continue;
        };
        let kept_shape = if kept_name == &own_name { &own } else { &other };
        let all_in = shape.2.is_some() && shape.2 == kept_shape.2 && seq <= kept_shape.3;
        let superseded = is_now(kept_shape.2) && !is_now(shape.2) && kept_shape.3 >= seq;
        assert!(
            list.set_aside_beside || all_in || superseded,
            "a list with edits set aside unsaid: {name} beside {kept_name}"
        );
    }
    let mut session = Session::new(NamedLoader);
    let said: Vec<bool> = restored.journals.values().map(|list| list.set_aside_beside).collect();
    if session.restore(&restored.record, restored.journals).is_ok() {
        let written = serde_json::to_string(&session.to_record()).unwrap_or_default();
        assert!(!written.contains("edits_set_aside_after_quit"), "never in the record");
        if said.iter().any(|said| *said) {
            assert!(
                session
                    .tabs()
                    .iter()
                    .any(|tab| session.tabs().tab_notice(tab.id).is_some_and(|notice| notice.code() == "edits_set_aside_after_quit")),
                "the notice is on its tab"
            );
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
        use_a_linked_pair(data);
    });
}
