//! A state folder as a copy that went back left it (an older app's record over a newer app's journals), then a launch of this app.

use super::super::*;
use recto_session::TabId;
use recto_session::edit::marks::{MarkOp, MarkPage, MarkRect};
use recto_session::record::{AuthorName, FileRecord, MARKED_JOURNAL_VERSION, PathField, TabEntry};

/// A state folder and a file in it.
struct Bed {
    root: recto_testpdf::TempDir,
    paths: StatePaths,
    file: PathBuf,
    stamp: FileStamp,
}

impl Bed {
    fn new() -> Self {
        let root = recto_testpdf::TempDir::new_in(&std::env::temp_dir().canonicalize().unwrap(), "rwb");
        let paths = StatePaths::overridden(&root).unwrap();
        paths.prepare().unwrap();
        let file = root.join("gamma.pdf");
        std::fs::write(&file, b"the file the edits were made on").unwrap();
        let stamp = file_stamp(&file).unwrap();
        Self { root, paths, file, stamp }
    }

    fn journals(&self) -> PathBuf {
        self.root.join("session").join(JOURNALS)
    }

    /// A journal of tab 1 about the file, as an app writes one from the file as it is: its header (of `version`), then `ops`, under `name`. After
    /// [`Self::record`], which makes the folders.
    fn journal(&self, name: &str, version: u32, ops: &[JournalOp]) {
        let header = JournalHeader {
            version,
            ..JournalHeader::new(1, &self.file, Some(self.stamp), 5, 0)
        };
        let mut bytes = record::journal_start(&header, None);
        bytes.extend_from_slice(&record::journal_lines(ops));
        local::write_private_file(&self.journals().join(name), &bytes).unwrap();
    }

    /// The record an app wrote: tab 1 on the file, naming `journal`, saying `notice`.
    fn record(&self, journal: Option<&str>, notice: Option<FileNotice>) {
        let tab = TabEntry {
            id: 1,
            path: PathField::from_path(&self.file),
            file: Some(FileRecord::from(self.stamp)),
            page: 0,
            page_count: 5,
            place: None,
            view_rotation: 0,
            toc_open: true,
            layout: Default::default(),
            renderer_only: false,
            selection: None,
            notice,
            journal: journal.map(str::to_string),
            edits: None,
            generated_contents: false,
            contents_folds: Vec::new(),
            back: Vec::new(),
            forward: Vec::new(),
            password: false,
        };
        let session = SessionRecord {
            app: AppRecord::default(),
            view: ViewRecord::default(),
            find: FindRecord::default(),
            edit_mode: false,
            active: Some(1),
            next_id: 2,
            tabs: vec![tab],
            comparison: None,
            recent: Vec::new(),
            withheld_places: Default::default(),
        };
        RecordStore::open(&self.paths, Flavor::App)
            .write_record(&session, &PlaceDelta::default(), false)
            .unwrap();
    }

    /// A launch: what tab 1 says, and how many highlights its list has.
    fn launch(&self) -> (Option<&'static str>, usize) {
        let store = RecordStore::open(&self.paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!("a record") };
        let mut session = recto_session::Session::new(recto_session::DiskLoader);
        session.restore(&restored.record, restored.journals).unwrap();
        let code = session.tabs().tab_notice(TabId(1)).map(|notice| notice.code());
        let marks = session.lists().get(TabId(1)).map_or(0, |list| list.marks().len());
        (code, marks)
    }
}

fn highlight() -> JournalOp {
    JournalOp::Mark(MarkOp::Add {
        pages: vec![MarkPage {
            source: 0,
            page: 3,
            rects: vec![MarkRect {
                x: 10.0,
                y: 10.0,
                width: 100.0,
                height: 12.0,
            }],
        }],
        by: AuthorName::Owner,
    })
}

/// **Failure caught (`fault:D.notice.edits_unreadable`, `check-carry.sh` run D, 2026-11-15):** a copy went back to 0.1.0, which left this app's
/// version-2 journal (a highlight) alone and wrote the record with the tab naming no journal and saying `edits_unreadable`; coming forward, the journal
/// is read whole and its highlight shown, and the tab still says its edits couldn't be read back, the older app's words taken as they stood.
#[test]
fn a_newer_journal_an_older_app_left_alone_reads_whole_and_the_tab_says_nothing() {
    let bed = Bed::new();
    let name = record::journal_file_name(Some(bed.stamp), &bed.file, None);
    bed.record(None, Some(FileNotice::EditsUnreadable));
    bed.journal(&name, MARKED_JOURNAL_VERSION, &[highlight()]);
    assert_eq!(bed.launch(), (None, 1), "the highlight is there, and nothing is said of it");
}

/// **Failure caught: a notice that's still so dropped with the older app's.** This app's own `edits_unreadable` stays until the person has seen it:
/// on a record naming the journal it's about (one written clean after lines it couldn't use), and on a journal of the version every app reads that
/// the record doesn't name yet (a list started after another couldn't be read, the record not yet written; what couldn't be read is kept beside it).
#[test]
fn this_apps_own_unreadable_notice_stays() {
    for case in ["the record names the journal", "a journal every app reads"] {
        let bed = Bed::new();
        let name = record::journal_file_name(Some(bed.stamp), &bed.file, None);
        match case {
            "the record names the journal" => {
                bed.record(Some(&name), Some(FileNotice::EditsUnreadable));
                bed.journal(&name, MARKED_JOURNAL_VERSION, &[highlight()]);
            }
            _ => {
                bed.record(None, Some(FileNotice::EditsUnreadable));
                bed.journal(&name, record::CURRENT_VERSION, &[JournalOp::Rotate(0, 1, 1, AuthorName::Owner)]);
            }
        }
        assert_eq!(bed.launch().0, Some("edits_unreadable"), "{case}");
    }
}
