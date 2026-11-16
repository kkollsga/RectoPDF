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
            set_aside_earlier: false,
        };
        session
            .restore(&main, HashMap::from([(name, list)]))
            .expect("what a session wrote it can restore");
        let _ = session.page_source(id, 0);
        let _ = session.sidebar();
        session.discard_edits(id);
    }
}

/// A sleep file: refused, or one of facts within every cap, which reads back as it was once it's been written again. Whatever the bytes, the format the
/// pass after a launch reads from their start is read without a panic, and one that says it's older is never decoded (whose facts could have pictures after
/// them). A file whose start says nothing the pass reads as older too, and removes; that one may still decode, a forged one, which is harmless.
fn use_a_sleep_file(bytes: &[u8]) {
    let format = recto_session::sleep::sleep_format_of(bytes);
    let Ok(sleep) = recto_session::sleep::decode_sleep(bytes, CURRENT_VERSION) else {
        return;
    };
    assert!(
        format.is_none_or(|format| format >= recto_session::sleep::SLEEP_FORMAT),
        "a file that says it's of an older format, as its start reads, was read: {format:?}"
    );
    assert!(sleep.sizes.len() <= recto_session::sleep::MAX_SIZES);
    assert!(sleep.sizes.iter().all(|(page, ..)| *page < sleep.pages));
    let again = recto_session::sleep::decode_sleep(&recto_session::sleep::encode_sleep(&sleep), CURRENT_VERSION).expect("what was read can be written");
    assert_eq!(again, sleep);
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

/// One journal of a linked pair, as the input shapes it: its tab, path and version, where it started again from a whole list and that list's turns,
/// and the turns after.
struct Shape {
    tab: u32,
    path: PathBuf,
    stamp: Option<recto_platform::FileStamp>,
    base_seq: u64,
    base_turns: usize,
    turns: usize,
}

impl Shape {
    fn turn() -> record::JournalOp {
        record::JournalOp::Rotate(0, 1, 1, record::AuthorName::Owner)
    }

    fn base(&self) -> Option<record::EditsRecord> {
        (self.base_turns > 0)
            .then(|| {
                PageEdits::replay(12, None, &vec![Self::turn(); self.base_turns])
                    .ok()
                    .map(|(list, _)| list.to_record())
            })
            .flatten()
    }

    fn bytes(&self) -> Vec<u8> {
        let header = record::JournalHeader::new(self.tab, &self.path, self.stamp, 12, self.base_seq);
        let mut bytes = record::journal_start(&header, self.base().as_ref());
        bytes.extend_from_slice(&record::journal_lines(&vec![Self::turn(); self.turns]));
        bytes
    }

    /// Its steps, and whether it holds unsaved edits.
    fn steps(&self) -> (u64, bool) {
        let edited = PageEdits::replay(12, self.base().as_ref(), &vec![Self::turn(); self.turns]).is_ok_and(|(list, _)| list.is_edited());
        (self.base_seq + self.turns as u64, edited)
    }
}

/// The record of a linked pair: tab 1 on `file` at `first`, naming `journal`, saying its file changed if `changed`.
fn pair_record(file: &Path, first: recto_platform::FileStamp, journal: &str, changed: bool) -> record::SessionRecord {
    record::SessionRecord {
        app: record::AppRecord::default(),
        view: record::ViewRecord::default(),
        find: record::FindRecord::default(),
        edit_mode: false,
        active: Some(1),
        next_id: 3,
        tabs: vec![record::TabEntry {
            id: 1,
            path: record::PathField::from_path(file),
            file: Some(first.into()),
            page: 0,
            page_count: 12,
            place: None,
            view_rotation: 0,
            toc_open: true,
            layout: Default::default(),
            renderer_only: false,
            selection: None,
            notice: changed.then_some(record::FileNotice::Changed),
            journal: Some(journal.to_string()),
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
    }
}

/// **Two journals of one tab, their headers from the input** (slice 2's crash rule, `record_store::followed`, review M2): the second journal's tab id, path,
/// version (none, the record's, the file as it is now, or a third), where it started again from a whole list (up or down from the first's steps) and
/// its own steps; the file replaced by another program or not; and the record saying it changed or not. What must hold: [`linked_pair_holds`].
fn use_a_linked_pair(data: &[u8]) {
    let byte = |at: usize| data.get(at).copied().unwrap_or(0);
    // One input in eight: a launch is most of a round's cost (about 25 ms in a debug build), the shapes are few (some 10^5), and the smoke test's
    // 400 rounds took 12 s more with every input (49 s to 61 s, under load).
    if byte(10) % 8 != 0 {
        return;
    }
    let root = folder();
    let Ok(paths) = StatePaths::overridden(&root) else { return };
    if paths.prepare().is_err() {
        return;
    }
    let journals = root.join("session").join("journals");
    let _ = local::ensure_private_dir(&root.join("session"));
    let _ = local::ensure_private_dir(&journals);
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
    let third = recto_platform::FileStamp {
        size: first.size.wrapping_add(7),
        ..first
    };
    let own = Shape {
        tab: 1,
        path: file.clone(),
        stamp: Some(first),
        base_seq: u64::from(byte(1) % 4),
        base_turns: usize::from(byte(2) % 3),
        turns: usize::from(byte(3) % 3),
    };
    let other = Shape {
        tab: if byte(4) & 1 == 0 { 1 } else { 2 },
        path: if byte(4) & 2 == 0 { file.clone() } else { root.join("other.pdf") },
        stamp: [None, Some(first), Some(now), Some(third)][usize::from(byte(5) % 4)],
        base_seq: u64::from(byte(6) % 6),
        base_turns: usize::from(byte(7) % 3),
        turns: usize::from(byte(8) % 3),
    };
    let pair = [
        (record::journal_file_name(Some(first), &file, None), own),
        (record::journal_file_name(other.stamp, &other.path, Some(2)), other),
    ];
    // Written as a launch finds them, with no store's write (and its flushes) to pay for each input.
    let main = pair_record(&file, first, &pair[0].0, byte(9) & 1 == 1);
    let _ = local::write_private_file(&root.join("session").join("record.a.json"), &record::encode_record(&main, 1));
    for (name, shape) in &pair {
        let _ = local::write_private_file(&journals.join(name), &shape.bytes());
    }
    if let Loaded::Restored(restored) = RecordStore::open(&paths, Flavor::App).load() {
        linked_pair_holds(*restored, &pair, &journals, now);
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A journal with unsaved edits never leaves the folder (it's a tab's list, or set aside under its `.not-merged` name), and one set aside with edits not
/// all in the list kept (by sequence) that isn't the old list a follow superseded leaves its notice on the tab, which is never written into the record.
fn linked_pair_holds(restored: recto_control::record_store::Restored, pair: &[(String, Shape); 2], journals: &Path, now: recto_platform::FileStamp) {
    let is_now = |stamp: Option<recto_platform::FileStamp>| stamp.is_some_and(|stamp| stamp.same_contents(now) && stamp.inode == now.inode);
    for (name, shape) in pair {
        let (seq, edited) = shape.steps();
        let claimed = restored.journals.contains_key(name.as_str());
        if !edited || claimed {
            continue;
        }
        assert!(
            journals.join(name).with_extension("not-merged").exists(),
            "a journal with unsaved edits is a tab's or set aside: {name}"
        );
        // Set aside: the list kept for tab 1 says so, unless every step of this is in it, or it's the old list a follow superseded.
        let kept = restored.record.tabs.iter().find(|tab| tab.id == 1).and_then(|tab| tab.journal.as_deref());
        let Some((kept_shape, list)) = kept.and_then(|kept| Some((&pair.iter().find(|(name, _)| name == kept)?.1, restored.journals.get(kept)?))) else {
            continue;
        };
        let all_in = shape.stamp.is_some() && shape.stamp == kept_shape.stamp && seq <= kept_shape.base_seq;
        let superseded = is_now(kept_shape.stamp) && !is_now(shape.stamp) && kept_shape.base_seq >= seq;
        assert!(list.set_aside_beside || all_in || superseded, "a list with edits set aside unsaid: {name}");
    }
    // Only where there's a notice to see: a restore for every input is most of what this would cost.
    if !restored.journals.values().any(|list| list.set_aside_beside) {
        return;
    }
    let mut session = Session::new(NamedLoader);
    if session.restore(&restored.record, restored.journals).is_ok() {
        let written = serde_json::to_string(&session.to_record()).unwrap_or_default();
        assert!(!written.contains("edits_set_aside_after_quit"), "never in the record");
        let shown = session.tabs().iter().any(|tab| {
            session
                .tabs()
                .tab_notice(tab.id)
                .is_some_and(|notice| notice.code() == "edits_set_aside_after_quit")
        });
        assert!(shown, "the notice is on its tab");
    }
}

/// **A tab's two lists after a copy went back to an older app** (`record_store::went_back`), their shapes from the input's bytes 12 to 19: where the
/// newer list (N, with a highlight) is (under its own name, set aside by 0.1.1 as `.not-merged`, kept as `.earlier`, or something that isn't a
/// journal under N's set-aside name), whose id and path it has, a turn of its own or not, and its version; the older list's (O's) version (one, two,
/// or none), its base and where it starts, its turns and a save begun; the file changed on disk or not; and what the record says. What must hold:
/// nothing a person made leaves the folder (N's bytes are a file's first bytes, merged or kept; O's are a file's, kept or aged as a closed history),
/// a launch restores, and the set-aside notice is never written into a record.
fn use_a_went_back_pair(data: &[u8]) {
    let byte = |at: usize| data.get(at).copied().unwrap_or(0);
    // One input in 32 (the seeds reach each shape): a launch is most of a round's cost, and at one in eight the smoke test's 400 rounds took
    // about 4 s more under load (22 s to 27 s, 2026-11-16).
    if byte(12) % 32 != 0 {
        return;
    }
    let root = folder();
    let Ok(paths) = StatePaths::overridden(&root) else { return };
    if paths.prepare().is_err() {
        return;
    }
    let journals = root.join("session").join("journals");
    let _ = local::ensure_private_dir(&root.join("session"));
    let _ = local::ensure_private_dir(&journals);
    let file = root.join("doc.pdf");
    if std::fs::write(&file, b"the version the edits were made on").is_err() {
        return;
    }
    let Ok(stamp) = recto_platform::file_stamp(&file) else { return };
    let highlight = record::JournalOp::Mark(recto_session::edit::marks::MarkOp::Add {
        pages: vec![recto_session::edit::marks::MarkPage {
            source: 0,
            page: 3,
            rects: vec![recto_session::edit::marks::MarkRect {
                x: 1.0,
                y: 1.0,
                width: 50.0,
                height: 10.0,
            }],
        }],
        by: record::AuthorName::Owner,
    });
    let n_header = record::JournalHeader {
        version: if byte(15) & 2 == 0 { record::MARKED_JOURNAL_VERSION } else { CURRENT_VERSION },
        ..record::JournalHeader::new(
            if byte(14) & 1 == 0 { 1 } else { 2 },
            &if byte(14) & 2 == 0 { file.clone() } else { root.join("other.pdf") },
            Some(stamp),
            12,
            0,
        )
    };
    let mut n_ops = vec![highlight];
    if byte(15) & 1 == 1 {
        n_ops.push(Shape::turn());
    }
    let n = [record::journal_start(&n_header, None), record::journal_lines(&n_ops)].concat();
    let o_header = record::JournalHeader {
        version: [CURRENT_VERSION, record::MARKED_JOURNAL_VERSION, 0][usize::from(byte(16) % 3)],
        ..record::JournalHeader::new(1, &file, Some(stamp), 12, u64::from(byte(17) % 3))
    };
    let o_base = (byte(17) & 4 != 0).then(|| PageEdits::new(12).to_record());
    let mut o_ops = vec![Shape::turn(); usize::from(byte(18) % 3)];
    if byte(18) & 8 != 0 {
        o_ops.push(record::JournalOp::Saving(recto_platform::FileStamp { size: 1, ..stamp }.into()));
    }
    let o = [record::journal_start(&o_header, o_base.as_ref()), record::journal_lines(&o_ops)].concat();
    let newer = record::journal_file_name(Some(stamp), &file, None);
    let older = record::journal_file_name(Some(stamp), &file, Some(1));
    let stem = newer.trim_end_matches(".log");
    let (n_name, n_bytes) = match byte(13) % 4 {
        0 => (newer.clone(), n),
        1 => (format!("{stem}.not-merged"), n),
        2 => (format!("{stem}.earlier"), n),
        _ => (format!("{stem}.not-merged"), b"not a journal\n".repeat(usize::from(byte(13)))),
    };
    let mut main = pair_record(&file, stamp, &older, false);
    main.tabs[0].notice = (byte(19) & 2 != 0).then_some(record::FileNotice::EditsUnreadable);
    let _ = local::write_private_file(&root.join("session").join("record.a.json"), &record::encode_record(&main, 1));
    let _ = local::write_private_file(&journals.join(&n_name), &n_bytes);
    let _ = local::write_private_file(&journals.join(&older), &o);
    if byte(19) & 1 == 1 {
        let _ = std::fs::write(&file, b"another program's version, longer than the first");
    }
    if let Loaded::Restored(restored) = RecordStore::open(&paths, Flavor::App).load() {
        let mut session = Session::new(NamedLoader);
        assert!(
            session.restore(&restored.record, restored.journals).is_ok(),
            "what a load gives can be restored"
        );
        let written = serde_json::to_string(&session.to_record()).unwrap_or_default();
        assert!(!written.contains("went_back"), "never in the record");
    }
    let held: Vec<Vec<u8>> = std::fs::read_dir(&journals)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .collect();
    assert!(held.iter().any(|there| there.starts_with(&n_bytes)), "N's bytes are still a file's first bytes");
    assert!(held.contains(&o), "O's bytes are still a file's");
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
        use_a_went_back_pair(data);
    });
}
