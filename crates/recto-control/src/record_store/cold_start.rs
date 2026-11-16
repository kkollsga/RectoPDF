//! **A launch with a thousand tabs, measured** (`went_back`'s cost): the store's load and the session's restore of a record of 1,000 tabs, each with
//! one journal, and again with 1,000 lists set aside in the folder that are no tab's. Not a check: it asserts nothing about time, which is one
//! machine's, and runs only when asked, on a release build:
//!
//! `tools/sysguard/heavy.sh cargo nextest run --release --locked -p recto-control --run-ignored only --no-capture -E 'test(=record_store::cold_start::a_thousand_tabs)'`

use super::*;
use recto_session::record::{AuthorName, FileRecord, PathField};
use std::time::{Duration, Instant};

const TABS: u32 = 1000;
const RUNS: usize = 7;

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

#[test]
#[ignore = "a measurement, run on a release build when asked"]
fn a_thousand_tabs() {
    let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "cold");
    let paths = StatePaths::overridden(&root).unwrap();
    paths.prepare().unwrap();
    let pdfs = root.join("pdfs");
    std::fs::create_dir_all(&pdfs).unwrap();
    let mut tabs = Vec::new();
    let mut journals = Vec::new();
    for id in 1..=TABS {
        let file = pdfs.join(format!("{id}.pdf"));
        std::fs::write(&file, format!("file {id}")).unwrap();
        let stamp = file_stamp(&file).unwrap();
        let name = record::journal_file_name(Some(stamp), &file, None);
        let mut bytes = record::journal_start(&JournalHeader::new(id, &file, Some(stamp), 5, 0), None);
        bytes.extend_from_slice(&record::journal_lines(&[JournalOp::Rotate(0, 1, 1, AuthorName::Owner)]));
        journals.push((name.clone(), bytes));
        tabs.push(TabEntry {
            id,
            path: PathField::from_path(&file),
            file: Some(FileRecord::from(stamp)),
            page: 0,
            page_count: 5,
            place: None,
            view_rotation: 0,
            toc_open: true,
            layout: Default::default(),
            renderer_only: false,
            selection: None,
            notice: None,
            journal: Some(name),
            edits: None,
            generated_contents: false,
            contents_folds: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
            password: false,
        });
    }
    let session = SessionRecord {
        app: AppRecord::default(),
        view: ViewRecord::default(),
        find: FindRecord::default(),
        edit_mode: false,
        active: Some(1),
        next_id: TABS + 1,
        tabs,
        comparison: None,
        recent: Vec::new(),
        withheld_places: Default::default(),
    };
    RecordStore::open(&paths, Flavor::App)
        .write_record(&session, &PlaceDelta::default(), false)
        .unwrap();
    let folder = root.join("session").join(JOURNALS);
    for (name, bytes) in &journals {
        std::fs::write(folder.join(name), bytes).unwrap();
    }
    for set_aside in [false, true] {
        let mut times = Vec::new();
        for _ in 0..RUNS {
            if set_aside {
                for at in 0..TABS {
                    std::fs::write(folder.join(format!("journal-{:016x}.earlier", 0xfeed_0000u64 + u64::from(at))), b"x").unwrap();
                }
            }
            let began = Instant::now();
            let store = RecordStore::open(&paths, Flavor::App);
            let Loaded::Restored(restored) = store.load() else { panic!("a record") };
            let mut session = recto_session::Session::new(recto_session::DiskLoader);
            session.restore(&restored.record, restored.journals).unwrap();
            times.push(began.elapsed());
            assert_eq!(session.tabs().iter().count(), TABS as usize);
            drop(session);
            drop(store);
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.contains("feed") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        eprintln!(
            "cold start, {TABS} tabs with one journal each{}: median {:?} of {RUNS} (min {:?}, max {:?})",
            if set_aside { ", and 1,000 set-aside names" } else { "" },
            median(times.clone()),
            times.iter().min().unwrap(),
            times.iter().max().unwrap()
        );
    }
}
