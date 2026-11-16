//! A state folder as a copy that went back left it (an older app's record over a newer app's journals), then a launch of this app.

use super::super::*;
use super::{MAX_SET_ASIDE_NAMES, SET_ASIDE_LIMIT, STEPS};
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
    fn journal(&self, name: &str, version: u32, ops: &[JournalOp]) -> Vec<u8> {
        let header = JournalHeader {
            version,
            ..JournalHeader::new(1, &self.file, Some(self.stamp), 5, 0)
        };
        let mut bytes = record::journal_start(&header, None);
        bytes.extend_from_slice(&record::journal_lines(ops));
        self.put(name, &bytes);
        bytes
    }

    /// Any bytes under a name in the journals' folder.
    fn put(&self, name: &str, bytes: &[u8]) {
        local::write_private_file(&self.journals().join(name), bytes).unwrap();
    }

    /// The names in the journals' folder.
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.journals())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The name in the journals' folder whose bytes are these, if one is.
    fn holding(&self, bytes: &[u8]) -> Option<String> {
        self.names()
            .into_iter()
            .find(|name| std::fs::read(self.journals().join(name)).is_ok_and(|there| there == bytes))
    }

    /// A launch, and the session it gives.
    fn session(&self) -> recto_session::Session<recto_session::DiskLoader> {
        let store = RecordStore::open(&self.paths, Flavor::App);
        let Loaded::Restored(restored) = store.load() else { panic!("a record") };
        let mut session = recto_session::Session::new(recto_session::DiskLoader);
        session.restore(&restored.record, restored.journals).unwrap();
        session
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
        let session = self.session();
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
/// the record doesn't name yet (a list started after another couldn't be read, the record not yet written; what couldn't be read is kept beside it);
/// and (the review, item 5) one of the newer version with what couldn't be read of this file's journal kept beside it as `.unreadable`.
#[test]
fn this_apps_own_unreadable_notice_stays() {
    for case in [
        "the record names the journal",
        "a journal every app reads",
        "what couldn't be read kept beside it",
    ] {
        let bed = Bed::new();
        let name = record::journal_file_name(Some(bed.stamp), &bed.file, None);
        match case {
            "the record names the journal" => {
                bed.record(Some(&name), Some(FileNotice::EditsUnreadable));
                bed.journal(&name, MARKED_JOURNAL_VERSION, &[highlight()]);
            }
            "a journal every app reads" => {
                bed.record(None, Some(FileNotice::EditsUnreadable));
                bed.journal(&name, record::CURRENT_VERSION, &[JournalOp::Rotate(0, 1, 1, AuthorName::Owner)]);
            }
            _ => {
                // A journal of this file that couldn't be read, kept, and the next edit a highlight, its journal not yet in the record.
                bed.record(None, Some(FileNotice::EditsUnreadable));
                bed.put(&name.replace(".log", ".unreadable"), b"what couldn't be read");
                bed.journal(&name, MARKED_JOURNAL_VERSION, &[highlight()]);
            }
        }
        assert_eq!(bed.launch().0, Some("edits_unreadable"), "{case}");
    }
}

fn turn(page: u32) -> JournalOp {
    JournalOp::Rotate(page, 1, 1, AuthorName::Owner)
}

/// The two lists' names: the newer app's own (N) and the one the older app started beside it for tab 1 (O).
fn names(bed: &Bed) -> (String, String) {
    (
        record::journal_file_name(Some(bed.stamp), &bed.file, None),
        record::journal_file_name(Some(bed.stamp), &bed.file, Some(1)),
    )
}

/// What tab 1 shows: its notice, its highlights, and each page's turns.
fn shown(session: &recto_session::Session<recto_session::DiskLoader>) -> (Option<&'static str>, usize, Vec<u8>) {
    let code = session.tabs().tab_notice(TabId(1)).map(|notice| notice.code());
    let list = session.lists().get(TabId(1)).expect("a list");
    (code, list.marks().len(), list.slots().iter().map(|slot| slot.quarter_turns).collect())
}

/// The state run E of `check-carry.sh` leaves: 0.1.0's record naming its own list (O, a turn of page 1) and saying `edits_unreadable` of N, which
/// holds the highlight; or, `aside`, as 0.1.1 since `build/page-matcher-2` leaves it after that: N set aside as `.not-merged`, the record naming O.
/// O's and N's bytes.
fn went_back_with_a_turn(bed: &Bed, aside: bool, older_ops: &[JournalOp]) -> (Vec<u8>, Vec<u8>) {
    let (newer, older) = names(bed);
    let notice = (!aside).then_some(FileNotice::EditsUnreadable);
    bed.record(Some(&older), notice);
    let n = bed.journal(&newer, MARKED_JOURNAL_VERSION, &[highlight()]);
    if aside {
        std::fs::rename(bed.journals().join(&newer), bed.journals().join(&newer).with_extension("not-merged")).unwrap();
    }
    let o = bed.journal(&older, record::CURRENT_VERSION, older_ops);
    (n, o)
}

/// **Failure caught (`fault:F.highlight_hidden` and `fault:F.notice.*`, `check-carry.sh` runs E and F, 2026-11-15 and 16):** a copy went back to
/// 0.1.0, which started a list of its own for a turn made there beside 0.1.1's with a highlight; coming forward, the turn was shown and the highlight
/// wasn't, and the tab said its lists were set aside "after an unexpected quit" (or, before, "open in two tabs"), neither so. Both lists combine: the
/// turn and the highlight are shown, nothing is said, and undo goes back through them in the order they were made. On disk, N's bytes are its new
/// self's prefix, and O is kept byte for byte under a closed history's name. As found under N's own name and as 0.1.1 sets it aside, and a second
/// launch finds the same (nothing appended twice).
#[test]
fn two_lists_that_combine_give_every_edit_and_say_nothing() {
    for aside in [false, true] {
        let bed = Bed::new();
        let (n, o) = went_back_with_a_turn(&bed, aside, &[turn(0)]);
        let (newer, _) = names(&bed);
        let mut session = bed.session();
        assert_eq!(
            shown(&session),
            (None, 1, vec![1, 0, 0, 0, 0]),
            "aside {aside}: the turn and the highlight, nothing said"
        );
        assert!(
            std::fs::read(bed.journals().join(&newer)).unwrap().starts_with(&n),
            "aside {aside}: N's bytes its prefix"
        );
        assert!(
            bed.holding(&o).is_some_and(|name| name.starts_with("closed-")),
            "aside {aside}: O kept, aged as closed histories"
        );
        assert!(session.undo_edit(TabId(1)));
        let (_, marks, turns) = shown(&session);
        assert_eq!((marks, turns[0]), (1, 0), "aside {aside}: the later step, the turn, undone first");
        drop(session);
        assert_eq!(shown(&bed.session()), (None, 1, vec![1, 0, 0, 0, 0]), "aside {aside}: again, the same");
        let parsed = record::parse_journal(&std::fs::read(bed.journals().join(&newer)).unwrap(), MARKED_JOURNAL_VERSION).unwrap();
        assert_eq!(parsed.ops.len(), 2, "aside {aside}: the highlight and the turn, once each");
    }
}

/// **Failure caught: a merge a crash cut short brings back a step twice or not at all.** O has two turns. At each point: N back under its own name
/// with nothing appended; one turn appended and the second line torn half way; both appended and O not yet aside; O aside and the record still
/// naming it. Each launch ends with each turn once, the highlight, nothing said, and O's bytes kept.
#[test]
fn a_merge_cut_short_at_each_point_is_taken_up_where_it_stopped() {
    for point in ["back under its own name", "part appended", "all appended", "O aside"] {
        let bed = Bed::new();
        let (n, o) = went_back_with_a_turn(&bed, false, &[turn(0), turn(1)]);
        let (newer, older) = names(&bed);
        let lines = record::journal_lines(&[turn(0), turn(1)]);
        let first = record::journal_lines(&[turn(0)]).len();
        let appended = match point {
            "back under its own name" => 0,
            "part appended" => first + (lines.len() - first) / 2,
            _ => lines.len(),
        };
        bed.put(&newer, &[n.as_slice(), &lines[..appended]].concat());
        if point == "O aside" {
            std::fs::rename(bed.journals().join(&older), bed.journals().join(record::closed_journal_name(&older).unwrap())).unwrap();
        }
        let session = bed.session();
        assert_eq!(shown(&session), (None, 1, vec![1, 1, 0, 0, 0]), "{point}: each turn once, and the highlight");
        let parsed = record::parse_journal(&std::fs::read(bed.journals().join(&newer)).unwrap(), MARKED_JOURNAL_VERSION).unwrap();
        assert_eq!((parsed.ops.len(), parsed.lost_lines), (3, false), "{point}: N holds each step once");
        assert!(bed.holding(&o).is_some(), "{point}: O's bytes kept");
        assert!(!bed.journals().join(&older).exists(), "{point}: O isn't a journal any more");
    }
}

/// **Failure caught: two lists that can't be shown as one merged anyway, or one put out of reach unsaid, or said in words that aren't so.** Each
/// not provably case 1: N has a turn of its own; the file changed on disk since; O began a save. The tab keeps O (the turn made last), says the
/// earlier edits were kept (never "an unexpected quit" or "two tabs"), and N is kept byte for byte; the record written has no trace of the notice,
/// and the next launch says it again.
#[test]
fn two_lists_that_dont_combine_keep_the_later_and_say_so_each_launch() {
    for why in ["N turned a page", "the file changed", "O began a save"] {
        let bed = Bed::new();
        let (newer, older) = names(&bed);
        bed.record(Some(&older), Some(FileNotice::EditsUnreadable));
        let mut n_ops = vec![highlight()];
        if why == "N turned a page" {
            n_ops.push(turn(2));
        }
        let n = bed.journal(&newer, MARKED_JOURNAL_VERSION, &n_ops);
        let mut o_ops = vec![turn(0)];
        if why == "O began a save" {
            o_ops.push(JournalOp::Saving(FileRecord::from(FileStamp { size: 1, ..bed.stamp })));
        }
        bed.journal(&older, record::CURRENT_VERSION, &o_ops);
        if why == "the file changed" {
            std::fs::write(&bed.file, b"another program's version, longer than the first").unwrap();
        }
        let session = bed.session();
        let (code, marks, turns) = shown(&session);
        assert_eq!((code, marks, turns[0]), (Some("edits_set_aside_went_back"), 0, 1), "{why}: O shown, and said");
        assert!(bed.holding(&n).is_some_and(|name| name.ends_with(".earlier")), "{why}: N kept byte for byte");
        let written = session.to_record();
        assert!(!serde_json::to_string(&written).unwrap().contains("went_back"), "{why}: never in the record");
        RecordStore::open(&bed.paths, Flavor::App)
            .write_record(&written, &PlaceDelta::default(), false)
            .unwrap();
        drop(session);
        assert_eq!(bed.launch().0, Some("edits_set_aside_went_back"), "{why}: said again, from the list kept");
    }
}

/// **Failure caught: an earlier list kept forever, or its notice said after the tab's list stopped being the one it couldn't be combined with.**
/// Once the tab's list isn't O (the edits discarded here: the record names no journal), nothing is said, and N goes to a closed history's name with
/// its bytes, where the 30 days of closed histories run from now.
#[test]
fn an_earlier_list_kept_goes_with_closed_histories_once_its_tab_moves_on() {
    let bed = Bed::new();
    let (newer, older) = names(&bed);
    bed.record(Some(&older), None);
    let n = bed.journal(&newer, MARKED_JOURNAL_VERSION, &[highlight(), turn(2)]);
    bed.journal(&older, record::CURRENT_VERSION, &[turn(0)]);
    assert_eq!(bed.launch().0, Some("edits_set_aside_went_back"));
    std::fs::remove_file(bed.journals().join(&older)).unwrap();
    bed.record(None, None);
    assert_eq!(bed.launch(), (None, 0), "nothing said");
    let kept = bed.holding(&n).expect("N's bytes kept");
    assert!(
        record::is_closed_journal_name(&kept) && kept != record::closed_journal_name(&newer).unwrap(),
        "{kept}: closed, never taken as the file's history"
    );
}

/// **Failure caught: a second trip back after a merge** (0.1.0 again on the merged list, which it leaves alone, and a turn of its own): the merged
/// list has a page turned of its own now, so the two don't combine: the later list is shown, the earlier kept and said.
#[test]
fn a_second_trip_back_after_a_merge_keeps_the_later_and_says_so() {
    let bed = Bed::new();
    went_back_with_a_turn(&bed, false, &[turn(0)]);
    assert_eq!(shown(&bed.session()).0, None, "the first trip combines");
    let (newer, older) = names(&bed);
    let merged = std::fs::read(bed.journals().join(&newer)).unwrap();
    bed.record(Some(&older), Some(FileNotice::EditsUnreadable));
    bed.journal(&older, record::CURRENT_VERSION, &[turn(3)]);
    let (code, marks, turns) = shown(&bed.session());
    assert_eq!((code, marks, turns), (Some("edits_set_aside_went_back"), 0, vec![0, 0, 0, 1, 0]));
    assert!(
        bed.holding(&merged).is_some_and(|name| name.ends_with(".earlier")),
        "the merged list kept byte for byte"
    );
}

/// **Failure caught (rule 6): a hostile state folder panics a launch, reads without bound, or takes another tab's list.** N with another tab's id is
/// no list of this tab's (as `followed` judges it, unmerged); a `.not-merged` under N's name that isn't a journal is left as it is, the tab keeping O
/// and saying nothing; and past the most kept lists a launch looks at, the rest wait (one more than the limit, set to 4 here: one left).
#[test]
fn hostile_pairs_fall_to_not_this_tabs() {
    let bed = Bed::new();
    let (newer, older) = names(&bed);
    bed.record(Some(&older), None);
    let header = JournalHeader {
        version: MARKED_JOURNAL_VERSION,
        ..JournalHeader::new(7, &bed.file, Some(bed.stamp), 5, 0)
    };
    bed.put(&newer, &[record::journal_start(&header, None), record::journal_lines(&[highlight()])].concat());
    bed.journal(&older, record::CURRENT_VERSION, &[turn(0)]);
    let (_, marks, turns) = shown(&bed.session());
    assert_eq!((marks, turns[0]), (0, 1), "another tab's id: not merged into this tab");

    let bed = Bed::new();
    bed.record(Some(&older), None);
    bed.journal(&older, record::CURRENT_VERSION, &[turn(0)]);
    let garbage = b"not a journal at all\n".repeat(10);
    bed.put(&format!("{}.not-merged", newer.trim_end_matches(".log")), &garbage);
    assert_eq!(bed.launch(), (None, 0));
    assert!(bed.holding(&garbage).is_some_and(|name| name.ends_with(".not-merged")), "left as it is");

    let bed = Bed::new();
    bed.record(None, None);
    SET_ASIDE_LIMIT.store(4, std::sync::atomic::Ordering::Relaxed);
    for at in 0..=4 {
        bed.put(&format!("journal-{at:016x}.earlier"), b"x");
    }
    bed.launch();
    SET_ASIDE_LIMIT.store(MAX_SET_ASIDE_NAMES, std::sync::atomic::Ordering::Relaxed);
    let left = bed.names().iter().filter(|name| name.ends_with(".earlier")).count();
    assert_eq!(left, 1, "a bounded look: the limit's worth aged out, one waiting");
}

/// **Failure caught (the review, item 1): a power cut right after a merge hides O's edits with no notice.** O moved to a closed name could reach the
/// disk before O's lines in N did, which a plain `fsync` doesn't make the drive write. The order is held: N back under its own name and the folder
/// flushed, then O's lines in N flushed by the drive itself, then O moved and the folder flushed.
#[test]
fn a_merge_flushes_n_before_o_is_moved() {
    let bed = Bed::new();
    went_back_with_a_turn(&bed, true, &[turn(0)]);
    let (newer, older) = names(&bed);
    STEPS.with(|steps| steps.borrow_mut().clear());
    assert_eq!(shown(&bed.session()).0, None);
    let set_aside = newer.replace(".log", ".not-merged");
    let closed = record::closed_journal_name(&older).unwrap();
    assert_eq!(
        STEPS.with(|steps| steps.borrow().clone()),
        vec![
            format!("moved {set_aside} to {newer}, the folder flushed"),
            format!("appended to {newer}, flushed by the drive: true"),
            format!("moved {older} to {closed}, the folder flushed"),
        ]
    );
}

/// **Failure caught (the review, item 2): the set-aside notice stops, and the kept list is orphaned, while the tab's list is still O.** A highlight
/// added here makes O's journal the newer version; or the file moves, and the tab's path with it. Said each launch still, from the kept list's first
/// line; then, the edits discarded, nothing is said and N goes to a closed history's name.
#[test]
fn the_kept_list_is_said_while_the_tabs_list_is_o_whatever_o_becomes() {
    for change in ["a highlight added to O", "the file moved"] {
        let bed = Bed::new();
        let (newer, older) = names(&bed);
        bed.record(Some(&older), None);
        let n = bed.journal(&newer, MARKED_JOURNAL_VERSION, &[highlight(), turn(2)]);
        bed.journal(&older, record::CURRENT_VERSION, &[turn(0)]);
        assert_eq!(bed.launch().0, Some("edits_set_aside_went_back"), "{change}");
        match change {
            "a highlight added to O" => {
                let store = RecordStore::open(&bed.paths, Flavor::App);
                let Loaded::Restored(_) = store.load() else { panic!() };
                let work = JournalWork {
                    pages: Vec::new(),
                    retired: Vec::new(),
                    appends: vec![recto_session::JournalAppend {
                        tab: TabId(1),
                        name: older.clone(),
                        header: JournalHeader::new(1, &bed.file, Some(bed.stamp), 5, 0),
                        through: 2,
                        ops: vec![highlight()],
                        closing: false,
                        base: None,
                        replaces: None,
                    }],
                };
                assert!(store.write_journals(&work, false).1.is_none());
                let header = record::parse_journal(&std::fs::read(bed.journals().join(&older)).unwrap(), MARKED_JOURNAL_VERSION)
                    .unwrap()
                    .header;
                assert_eq!(header.version, MARKED_JOURNAL_VERSION, "O is of the newer version now");
            }
            _ => {
                let moved = bed.root.join("moved.pdf");
                std::fs::rename(&bed.file, &moved).unwrap();
                let mut record = match RecordStore::open(&bed.paths, Flavor::App).load() {
                    Loaded::Restored(restored) => restored.record,
                    _ => panic!(),
                };
                record.tabs[0].path = PathField::from_path(&moved);
                RecordStore::open(&bed.paths, Flavor::App)
                    .write_record(&record, &PlaceDelta::default(), false)
                    .unwrap();
            }
        }
        assert_eq!(bed.launch().0, Some("edits_set_aside_went_back"), "{change}: still said");
        std::fs::remove_file(bed.journals().join(&older)).unwrap();
        bed.record(None, None);
        assert_eq!(bed.launch().0, None, "{change}: discarded, nothing said");
        assert!(
            bed.holding(&n).is_some_and(|name| record::is_closed_journal_name(&name)),
            "{change}: N with closed histories"
        );
    }
}

/// **Failure caught (the review, item 3): a link left at a name of ours has the app change a file of the person's** (the time of a PDF, whose
/// journals then stop matching it). A link and a folder at a kept list's name, at 0.1.1's set-aside names and at a closed history's name, of no tab
/// and of the tab's own key: two launches touch nothing outside the journals' folder, and nothing panics.
#[test]
fn links_and_folders_at_our_names_touch_nothing_outside() {
    let bed = Bed::new();
    let (newer, older) = names(&bed);
    bed.record(Some(&older), None);
    bed.journal(&older, record::CURRENT_VERSION, &[turn(0)]);
    let outside = bed.root.join("outside.pdf");
    std::fs::write(&outside, b"a person's PDF").unwrap();
    let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    std::fs::File::options().write(true).open(&outside).unwrap().set_modified(long_ago).unwrap();
    let stem = newer.trim_end_matches(".log");
    let names = [
        format!("{stem}.earlier"),
        format!("{stem}-1.not-merged"),
        "journal-00000000000000aa.earlier".to_string(),
        "journal-00000000000000ab.not-merged".to_string(),
        "journal-00000000000000ac-2.not-merged".to_string(),
        "closed-00000000000000ad.log".to_string(),
    ];
    for (at, name) in names.iter().enumerate() {
        if at % 2 == 0 {
            std::os::unix::fs::symlink(&outside, bed.journals().join(name)).unwrap();
        } else {
            std::fs::create_dir(bed.journals().join(name)).unwrap();
        }
    }
    bed.launch();
    bed.launch();
    let meta = std::fs::metadata(&outside).unwrap();
    assert_eq!(meta.modified().unwrap(), long_ago, "its time as it was");
    assert_eq!(std::fs::read(&outside).unwrap(), b"a person's PDF");
}

/// **Failure caught (the review, item 4): 0.1.1's crash rule run on the went-back state** took N for the newer by its steps (two highlights against
/// one turn), made it the tab's list and set O aside as `journal-<key>-<n>.not-merged`, which nothing read again: O's turn hidden. The same test
/// decides, in either order: combining, the turn and both highlights are shown and nothing is said; not combining (N turned a page of its own), O is
/// the tab's list again and N is kept and said.
#[test]
fn either_order_of_the_two_lists_is_decided_the_same() {
    for (order, n_turns) in [("O the tab's, N aside", false), ("N the tab's, O aside", false), ("N the tab's, O aside", true)] {
        let bed = Bed::new();
        let (newer, older) = names(&bed);
        let mut n_ops = vec![highlight(), highlight()];
        if n_turns {
            n_ops.push(turn(2));
        }
        let o_aside = older.replace(".log", ".not-merged");
        if order.starts_with('O') {
            bed.record(Some(&older), None);
            bed.journal(&newer.replace(".log", ".not-merged"), MARKED_JOURNAL_VERSION, &n_ops);
            bed.journal(&older, record::CURRENT_VERSION, &[turn(0)]);
        } else {
            bed.record(Some(&newer), None);
            bed.journal(&newer, MARKED_JOURNAL_VERSION, &n_ops);
            bed.journal(&o_aside, record::CURRENT_VERSION, &[turn(0)]);
        }
        let (code, marks, turns) = shown(&bed.session());
        if n_turns {
            assert_eq!(
                (code, marks, turns[0]),
                (Some("edits_set_aside_went_back"), 0, 1),
                "{order}, N turned a page: O shown and N said"
            );
        } else {
            assert_eq!((code, marks, turns[0]), (None, 2, 1), "{order}: the turn and both highlights");
        }
        assert!(!bed.journals().join(&o_aside).exists(), "{order}: O isn't left aside unread");
    }
}

/// **Failure caught (the review, item 7): O's move refused after a merge** (not a crash), the record then naming N: the next launch set O aside
/// "after an unexpected quit", said once, and nothing pruned it. Its every step at N's end, it's moved to a closed history's name then, and nothing is
/// said.
#[test]
fn an_older_list_left_after_a_merge_is_moved_at_the_next_launch() {
    let bed = Bed::new();
    let (newer, older) = names(&bed);
    bed.record(Some(&newer), None);
    let n = bed.journal(&newer, MARKED_JOURNAL_VERSION, &[highlight(), turn(0)]);
    let o = bed.journal(&older, record::CURRENT_VERSION, &[turn(0)]);
    assert_eq!(shown(&bed.session()), (None, 1, vec![1, 0, 0, 0, 0]), "the merged list, nothing said");
    assert!(
        bed.holding(&o).is_some_and(|name| record::is_closed_journal_name(&name)),
        "O with closed histories"
    );
    assert_eq!(std::fs::read(bed.journals().join(&newer)).unwrap(), n, "N as it was: nothing appended twice");
}
