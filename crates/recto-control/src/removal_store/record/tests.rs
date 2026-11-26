//! The adapter lists real folders in a temp root, decodes the records in them and places by their tabs; the plan made from it is what a person
//! would be shown.

use super::*;
use crate::removal_store::testing::{Restore, bounded, limits, mkfifo, mode, put, version};
use recto_core::removal::{DESIGN_LIMITS, Kinds, LEDGER_LIMITS, Ledger, Link, Plan, Request, Surveyed, lineage};
use recto_session::record::{
    AppRecord, FileRecord, FindRecord, MAIN_KIND, PathField, SessionRecord, TabEntry, ViewRecord, WithheldPlaces, encode_record, seal_as,
};
use recto_testpdf::TempDir;
use std::path::PathBuf;

const MINE: &str = "/docs/mine.pdf";
const THEIRS: &str = "/docs/theirs.pdf";

/// This document at `MINE` (inode 7), and another at `THEIRS` (inode 8), from the beginning of time.
fn ledger() -> Ledger {
    let at = |inode, path: &str| Link::At {
        version: version(inode),
        path: PathBuf::from(path),
    };
    Ledger {
        links: vec![at(7, MINE), at(8, THEIRS)],
        began_ns: 0,
        gap: None,
        stopped: None,
        good_len: 0,
        frames: 0,
        overflowed: false,
    }
}

/// A tab of the file `inode` (size 10, mtime 1, as `version` makes it) at `path`.
fn tab(id: u32, path: &str, inode: Option<u64>) -> TabEntry {
    TabEntry {
        id,
        path: PathField::Text(path.to_string()),
        file: inode.map(|inode| FileRecord {
            size: 10,
            mtime_ns: 1,
            device: 0,
            inode,
        }),
        page: 0,
        page_count: 5,
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
    }
}

fn mine() -> TabEntry {
    tab(1, MINE, Some(7))
}

fn theirs() -> TabEntry {
    tab(2, THEIRS, Some(8))
}

fn body(tabs: Vec<TabEntry>) -> SessionRecord {
    SessionRecord {
        app: AppRecord::default(),
        view: ViewRecord::default(),
        find: FindRecord::default(),
        edit_mode: false,
        active: None,
        next_id: 100,
        tabs,
        comparison: None,
        recent: Vec::new(),
        withheld_places: WithheldPlaces::default(),
    }
}

/// A record file's bytes holding `tabs`.
fn record(tabs: Vec<TabEntry>, seq: u64) -> Vec<u8> {
    encode_record(&body(tabs), seq)
}

/// A root with both sessions' folders.
fn root() -> TempDir {
    let dir = TempDir::new("recto-record-");
    for flavor in FLAVORS {
        std::fs::create_dir_all(dir.join(flavor)).unwrap();
    }
    dir
}

fn folder(root: &Path) -> PathBuf {
    root.join("session")
}

fn windowless(root: &Path) -> PathBuf {
    root.join("session-windowless")
}

/// A survey of `root` with every store but `SessionRecord` marked complete, so the plan decides on the records alone.
fn survey_with(root: &Path, mode: TargetMode, max: u32, caps: &RecordCaps) -> Result<Surveyed, Refusal> {
    let ledger = ledger();
    let lineage = lineage(&ledger, version(7), Path::new(MINE), &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(max));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && *s != Store::SessionRecord) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    survey_record_with(root, &lineage, mode, &mut survey, caps)?;
    survey.finish()
}

fn survey(root: &Path, mode: TargetMode) -> Result<Surveyed, Refusal> {
    survey_with(root, mode, 64, &RecordCaps::default())
}

fn plan_of(s: &Surveyed, mode: TargetMode) -> Result<Plan, Refusal> {
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), mode, false);
    recto_core::removal::plan(&request, &s.inventory(), &DESIGN_LIMITS)
}

fn refusal(root: &Path, mode: TargetMode) -> Refusal {
    plan_of(&survey(root, mode).unwrap(), mode).unwrap_err()
}

/// How many session-record artifacts have this owner.
fn count(s: &Surveyed, owner: Owner) -> usize {
    s.inventory()
        .artifacts()
        .iter()
        .filter(|a| a.store == Store::SessionRecord && a.owner == owner)
        .count()
}

fn total(s: &Surveyed) -> usize {
    s.inventory().artifacts().iter().filter(|a| a.store == Store::SessionRecord).count()
}

const INCOMPLETE: Refusal = Refusal::Incomplete(Store::SessionRecord);

/// A root whose `session` holds one slot with these tabs; what the one artifact is.
fn one(tabs: Vec<TabEntry>, mode: TargetMode) -> Owner {
    let dir = root();
    put(&folder(&dir), "record.a.json", &record(tabs, 1));
    let s = survey(&dir, mode).unwrap();
    assert_eq!(total(&s), 1);
    s.inventory().artifacts().iter().find(|a| a.store == Store::SessionRecord).unwrap().owner
}

/// Failure: a link is followed (a symlink to a record that would place as the target's is taken for the target's own and rewritten or read), a
/// pipe is waited on (the survey hangs), or a `session` that is a link is listed and marked complete so the plan says "none found".
#[test]
fn a_pipe_or_a_link_at_a_slot_or_places_is_unknown_and_never_followed() {
    for name in ["record.a.json", "record.b.json", "places.log"] {
        let dir = root();
        mkfifo(&folder(&dir).join(name));
        let path = dir.path().to_path_buf();
        assert_eq!(bounded(move || refusal(&path, TargetMode::Save)), INCOMPLETE, "a pipe at {name}");

        let dir = root();
        let outside = TempDir::new("recto-record-out-");
        // What a followed link would place as the target's.
        let precious = put(&outside, "precious", &record(vec![mine()], 1));
        std::os::unix::fs::symlink(&precious, folder(&dir).join(name)).unwrap();
        let path = dir.path().to_path_buf();
        assert_eq!(bounded(move || refusal(&path, TargetMode::Save)), INCOMPLETE, "a link at {name}");
    }
    let real = TempDir::new("recto-record-real-");
    let dir = TempDir::new("recto-record-");
    std::os::unix::fs::symlink(real.path(), dir.join("session")).unwrap();
    assert_eq!(refusal(&dir, TargetMode::Save), INCOMPLETE, "session as a link");
}

/// Failure: a slot that is a plain file at the listing and is swapped for a pipe or a link before it is opened (the listing's look is only a first
/// one) is opened with a plain `File::open`: the pipe is waited on, or the link is followed to a record that places as the target's. Without the
/// swap the slot is the target's (asserted first), so only a followed link or a waited-on pipe could change the answer.
#[test]
fn a_slot_swapped_for_a_pipe_or_a_link_before_it_is_opened_is_unknown() {
    for pipe in [true, false] {
        let dir = root();
        let outside = TempDir::new("recto-record-out-");
        let precious = put(&outside, "precious", &record(vec![mine()], 1));
        put(&folder(&dir), "record.a.json", &record(vec![mine()], 1));
        assert_eq!(count(&survey(&dir, TargetMode::Save).unwrap(), Owner::Target), 1);
        let hook: Hook = Arc::new(move |path| {
            std::fs::remove_file(path).unwrap();
            if pipe {
                mkfifo(path);
            } else {
                std::os::unix::fs::symlink(&precious, path).unwrap();
            }
        });
        let caps = RecordCaps {
            before_open: Some(hook),
            ..RecordCaps::default()
        };
        let root = dir.path().to_path_buf();
        let s = bounded(move || survey_with(&root, TargetMode::Save, 64, &caps).unwrap());
        assert_eq!(count(&s, Owner::Unknown), 1, "swapped for a {}", if pipe { "pipe" } else { "link" });
    }
}

/// Failure: another document's record is rewritten (a record of only other documents' tabs read as the target's), or the target's references
/// are left (one tab among several not looked at, a Save As source read as the target).
#[test]
fn a_slot_is_the_targets_if_any_tab_is_and_unrelated_if_none_is() {
    assert_eq!(one(vec![mine()], TargetMode::Save), Owner::Target);
    assert_eq!(one(vec![mine()], TargetMode::SaveAs), Owner::SaveAsSource);
    assert_eq!(one(vec![theirs()], TargetMode::Save), Owner::Unrelated);
    assert_eq!(one(vec![], TargetMode::Save), Owner::Unrelated);
    // Only the first tab looked at would say Unrelated here; the last tab alone would say Target.
    assert_eq!(one(vec![theirs(), mine()], TargetMode::Save), Owner::Target);
    assert_eq!(one(vec![mine(), theirs()], TargetMode::Save), Owner::Target);
}

/// Failure: a tab is guessed. Next to a tab that is the target's, each of these makes the whole slot `Unknown`: the version right and the path
/// wrong (a copy elsewhere), the path right and the version wrong, no file stamp, and inode 0.
#[test]
fn a_tab_that_isnt_found_by_both_its_version_and_its_path_makes_the_slot_unknown() {
    let broken = [
        ("version right, path wrong", tab(3, "/docs/elsewhere.pdf", Some(7))),
        ("path right, version wrong", tab(3, MINE, Some(99))),
        ("no file", tab(3, MINE, None)),
        ("inode 0", tab(3, MINE, Some(0))),
    ];
    for (what, bad) in broken {
        assert_eq!(one(vec![mine(), bad], TargetMode::Save), Owner::Unknown, "{what}");
    }
}

fn newer() -> Vec<u8> {
    seal_as(MAIN_KIND, CURRENT_VERSION + 1, 1, &serde_json::to_vec(&body(vec![mine()])).unwrap())
}

fn only(bytes: &[u8]) -> Surveyed {
    let dir = root();
    put(&folder(&dir), "record.a.json", bytes);
    survey(&dir, TargetMode::Save).unwrap()
}

/// Failure: a hostile slot is placed (a bad checksum or garbage read as a record of the target), a newer record is read as plain damage (so it
/// isn't `Foreign` and a rewrite is tried on it), or a read goes past the cap.
#[test]
fn a_hostile_slot_is_unknown_a_newer_one_foreign_and_the_cap_is_exact() {
    let good = record(vec![mine()], 1);
    let mut flipped = good.clone();
    let last = flipped.len() - 2;
    flipped[last] ^= 1;
    for (what, bytes) in [("bad checksum", flipped), ("garbage", b"garbage".to_vec()), ("empty", Vec::new())] {
        let s = only(&bytes);
        assert_eq!((count(&s, Owner::Unknown), total(&s)), (1, 1), "{what}");
        assert_eq!(s.inventory().artifacts()[0].format, Format::Current, "{what}");
    }
    let s = only(&newer());
    assert_eq!((count(&s, Owner::Unknown), s.inventory().artifacts()[0].format), (1, Format::Foreign));
    // At the cap it decodes; one byte over is Unknown, and the file isn't read past the cap.
    for (cap, owner) in [(good.len(), Owner::Target), (good.len() - 1, Owner::Unknown)] {
        let dir = root();
        put(&folder(&dir), "record.a.json", &good);
        let caps = RecordCaps {
            record_bytes: cap,
            ..RecordCaps::default()
        };
        let s = survey_with(&dir, TargetMode::Save, 64, &caps).unwrap();
        assert_eq!(count(&s, owner), 1, "cap {cap} of {}", good.len());
    }
}

/// Failure: the survey touches a document: a tab whose path is a pipe with no writer is waited on, or one whose file is unreadable is opened. Both
/// paths are the lineage's own, so both tabs place as the target's and only an open could change the answer.
#[test]
fn a_tabs_path_is_never_opened() {
    let docs = TempDir::new("recto-record-docs-");
    let pipe = docs.join("pipe.pdf");
    mkfifo(&pipe);
    let locked = put(&docs, "locked.pdf", b"%PDF-1.4");
    mode(&locked, 0o000);
    let at = |inode, path: &Path| Link::At {
        version: version(inode),
        path: path.to_path_buf(),
    };
    let ledger = Ledger {
        links: vec![at(7, &pipe), at(7, &locked)],
        ..ledger()
    };
    let dir = root();
    let tabs = vec![tab(1, pipe.to_str().unwrap(), Some(7)), tab(2, locked.to_str().unwrap(), Some(7))];
    put(&folder(&dir), "record.a.json", &record(tabs, 1));
    let root = dir.path().to_path_buf();
    let owner = bounded(move || {
        let current = root.join("none.pdf");
        let lineage = lineage(&ledger, version(7), &current, &LEDGER_LIMITS).unwrap();
        let mut survey = Survey::new(&limits(8));
        survey.history(&lineage);
        survey_record_with(&root, &lineage, TargetMode::Save, &mut survey, &RecordCaps::default()).unwrap();
        survey.finish().unwrap().inventory().artifacts()[0].owner
    });
    assert_eq!(owner, Owner::Target);
    mode(&locked, 0o600);
}

/// Failure: the target tab's places read as Unrelated (the newest slot alone is used), a places log with no record beside it is skipped as
/// harmless, or a hard-linked one is purged by name while its other name keeps the bytes.
#[test]
fn places_log_takes_the_strictest_of_the_slots() {
    // The older slot is the target's, the newer isn't: the log is the target's.
    let dir = root();
    put(&folder(&dir), "record.a.json", &record(vec![mine()], 1));
    put(&folder(&dir), "record.b.json", &record(vec![theirs()], 2));
    put(&folder(&dir), "places.log", b"x");
    let s = survey(&dir, TargetMode::Save).unwrap();
    assert_eq!((count(&s, Owner::Target), count(&s, Owner::Unrelated), total(&s)), (2, 1, 3));
    // Only others': Unrelated. A mode-000 log is placed all the same.
    let dir = root();
    put(&folder(&dir), "record.a.json", &record(vec![theirs()], 1));
    mode(&put(&folder(&dir), "places.log", b"x"), 0o000);
    let s = survey(&dir, TargetMode::Save).unwrap();
    assert_eq!((count(&s, Owner::Unrelated), total(&s)), (2, 2));
    // An unknown slot makes it unknown.
    let dir = root();
    put(&folder(&dir), "record.a.json", b"garbage");
    put(&folder(&dir), "places.log", b"x");
    assert_eq!(count(&survey(&dir, TargetMode::Save).unwrap(), Owner::Unknown), 2);
    // No slot at all: unknown.
    let dir = root();
    put(&folder(&dir), "places.log", b"x");
    assert_eq!(refusal(&dir, TargetMode::Save), INCOMPLETE);
    // Hard-linked: shared.
    let dir = root();
    let outside = TempDir::new("recto-record-out-");
    let other = put(&outside, "other", b"x");
    put(&folder(&dir), "record.a.json", &record(vec![theirs()], 1));
    std::fs::hard_link(&other, folder(&dir).join("places.log")).unwrap();
    assert_eq!(refusal(&dir, TargetMode::Save), Refusal::Shared(Store::SessionRecord));
    assert_eq!(std::fs::read(&other).unwrap(), b"x");
}

/// Failure: a file nobody can place is skipped as "none found" (so a removal leaves what it names), or `journals/` is counted here as well as by
/// the journals adapter, or Finder's own file and the record store's marker refuse every removal.
#[test]
fn the_rest_of_the_folder_is_unknown_except_what_is_known() {
    // (APFS refuses to create a name that isn't text, so that case is `names`' and the entry's own `is_file` rule, not made here.)
    let unknown: [(&str, fn(&Path)); 3] = [
        ("a text file", |p| {
            put(p, "notes.txt", b"hi");
        }),
        ("a record temp", |p| {
            put(p, "record.a.tmp123", b"{}");
        }),
        ("a folder", |p| std::fs::create_dir(p.join("old")).unwrap()),
    ];
    for (what, make) in unknown {
        let dir = root();
        make(&folder(&dir));
        assert_eq!(refusal(&dir, TargetMode::Save), INCOMPLETE, "{what}");
    }
    let dir = root();
    std::fs::create_dir(folder(&dir).join("journals")).unwrap();
    put(&folder(&dir), ".DS_Store", b"x");
    put(&folder(&dir), "went-back", b"1");
    let s = survey(&dir, TargetMode::Save).unwrap();
    assert!(s.inventory().artifacts().is_empty(), "journals, a plain .DS_Store and went-back add nothing");
    assert!(plan_of(&s, TargetMode::Save).is_ok());
}

/// Failure: a hard-linked slot is rewritten or purged by name while its other name keeps the old bytes.
#[test]
fn a_hard_linked_slot_is_shared() {
    let dir = root();
    let outside = TempDir::new("recto-record-out-");
    let other = put(&outside, "other", &record(vec![mine()], 1));
    std::fs::hard_link(&other, folder(&dir).join("record.a.json")).unwrap();
    assert_eq!(refusal(&dir, TargetMode::Save), Refusal::Shared(Store::SessionRecord));
}

/// Failure: a cut survey reads complete (a folder that couldn't be read, or a slot gone mid-survey, so the plan says "none found"), the cap is off
/// by one, a missing folder refuses, or the same name in both flavors is found twice (`Inconsistent`).
#[test]
fn errors_leave_the_store_unmarked_the_cap_is_exact_and_flavors_are_two_artifacts() {
    // No access to `session`.
    let dir = root();
    put(&folder(&dir), "record.a.json", &record(vec![theirs()], 1));
    let _restore = Restore(folder(&dir));
    mode(&folder(&dir), 0o000);
    assert_eq!(refusal(&dir, TargetMode::Save), INCOMPLETE, "no access");
    mode(&folder(&dir), 0o700);
    // A slot removed between the listing and its opening.
    let hook: Hook = Arc::new(|path| {
        let _ = std::fs::remove_file(path);
    });
    let caps = RecordCaps {
        before_open: Some(hook),
        ..RecordCaps::default()
    };
    let s = survey_with(&dir, TargetMode::Save, 64, &caps).unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), INCOMPLETE, "a slot gone mid-survey");
    // The second flavor can't be read either.
    let dir = root();
    let _restore = Restore(windowless(&dir));
    mode(&windowless(&dir), 0o000);
    assert_eq!(refusal(&dir, TargetMode::Save), INCOMPLETE, "the windowless folder");
    mode(&windowless(&dir), 0o700);
    // The cap: three entries are complete, a fourth is too large.
    let dir = root();
    put(&folder(&dir), "record.a.json", &record(vec![theirs()], 1));
    put(&folder(&dir), "record.b.json", &record(vec![theirs()], 2));
    put(&folder(&dir), "places.log", b"x");
    let caps = RecordCaps {
        entries: 3,
        ..RecordCaps::default()
    };
    assert!(plan_of(&survey_with(&dir, TargetMode::Save, 64, &caps).unwrap(), TargetMode::Save).is_ok());
    put(&folder(&dir), ".DS_Store", b"x");
    assert_eq!(survey_with(&dir, TargetMode::Save, 64, &caps).unwrap_err(), Refusal::TooLarge);
    // Missing folders are complete and empty.
    let none = TempDir::new("recto-record-");
    assert!(plan_of(&survey(&none, TargetMode::Save).unwrap(), TargetMode::Save).is_ok());
    std::fs::create_dir(none.join("session")).unwrap();
    assert!(plan_of(&survey(&none, TargetMode::Save).unwrap(), TargetMode::Save).is_ok());
    // The same names in both flavors.
    let dir = root();
    for flavor in [folder(&dir), windowless(&dir)] {
        put(&flavor, "record.a.json", &record(vec![mine()], 1));
    }
    let s = survey(&dir, TargetMode::Save).unwrap();
    assert_eq!((count(&s, Owner::Target), total(&s)), (2, 2));
}
