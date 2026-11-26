//! The adapter lists real folders in a temp root and places by headers and names; the plan made from it is what a person would be shown.

use super::*;
use crate::removal_store::placing::key_of;
use crate::removal_store::testing::{Restore, limits, mkfifo, mode, put, version};
use recto_core::removal::{DESIGN_LIMITS, Disposition, Effect, FileVersion, Kinds, LEDGER_LIMITS, Ledger, Link, Plan, Request, Surveyed, lineage};
use recto_platform::FileStamp;
use recto_session::record::{JournalHeader, journal_line, journal_start};
use recto_testpdf::TempDir;
use recto_testpdf::returns_in_time;
use std::path::PathBuf;

const MINE: &str = "/pdfs/mine.pdf";
const THEIRS: &str = "/pdfs/theirs.pdf";
const FIVE: [Store; 5] = [Store::Journal, Store::JournalVariant, Store::PageIds, Store::SleepFile, Store::KeptPages];

fn stamp(inode: u64) -> FileStamp {
    FileStamp {
        size: 10,
        mtime_ns: 1,
        device: 0,
        inode,
    }
}

/// The 16 digits the names of version `inode` carry.
fn k(inode: u64) -> String {
    key_of(inode, 10, 1)
}

/// This document at `MINE` (inode 7), and another at `THEIRS` (inode 8), from the beginning of time.
fn ledger() -> Ledger {
    let at = |inode, path: &str| Link::At {
        version: version(inode),
        path: PathBuf::from(path),
    };
    Ledger {
        links: vec![at(7, MINE), at(8, THEIRS)],
        destinations: Vec::new(),
        path_bytes: 0,
        began_ns: 0,
        gap: None,
        stopped: None,
        good_len: 0,
        frames: 0,
        overflowed: false,
    }
}

/// A root with both sessions' journals folders.
fn root() -> TempDir {
    let dir = TempDir::new("recto-journals-");
    for flavor in FLAVORS {
        std::fs::create_dir_all(dir.join(flavor).join("journals")).unwrap();
    }
    dir
}

fn folder(root: &Path) -> PathBuf {
    root.join("session").join("journals")
}

fn windowless(root: &Path) -> PathBuf {
    root.join("session-windowless").join("journals")
}

/// A survey of `root` with every store but the five marked complete, so the plan decides on the journals folders alone.
fn survey_with(root: &Path, ledger: &Ledger, current: FileVersion, mode: TargetMode, max: u32, caps: &Caps) -> Result<Surveyed, Refusal> {
    let lineage = lineage(ledger, current, Path::new(MINE), &LEDGER_LIMITS).unwrap();
    let mut survey = Survey::new(&limits(max));
    for store in Store::ALL.into_iter().filter(|s| s.enumerated() && !FIVE.contains(s)) {
        survey.mark_complete(store);
    }
    survey.history(&lineage);
    survey_journals_with(root, &lineage, mode, &mut survey, caps)?;
    survey.finish()
}

fn survey(root: &Path, mode: TargetMode) -> Result<Surveyed, Refusal> {
    survey_with(root, &ledger(), version(7), mode, 64, &Caps::default())
}

fn plan_of(s: &Surveyed, mode: TargetMode) -> Result<Plan, Refusal> {
    let request = Request::new(Kinds::from_bits(Kinds::METADATA).unwrap(), mode, false);
    recto_core::removal::plan(&request, &s.inventory(), &DESIGN_LIMITS)
}

fn plan(root: &Path, mode: TargetMode) -> Result<Plan, Refusal> {
    plan_of(&survey(root, mode)?, mode)
}

fn refusal(root: &Path, mode: TargetMode) -> Refusal {
    plan(root, mode).unwrap_err()
}

fn purged(p: &Plan, store: Store) -> usize {
    p.steps()
        .iter()
        .filter(|s| s.effect() == Effect::Purge(store))
        .map(|s| p.artifacts(s).len())
        .sum()
}

/// A journal's bytes: its first line, for a tab with this file and path.
fn journal(path: &str, inode: u64) -> Vec<u8> {
    journal_start(&JournalHeader::new(1, Path::new(path), Some(stamp(inode)), 3, 0), None)
}

/// Failure: a link is followed (a symlink to a file that would place as the target's is taken for the target's own and purged, or read for its
/// header), or a pipe is waited on (the survey hangs on a pipe with no writer). Each of the five kinds of name, as a pipe and as a link.
#[test]
fn a_pipe_or_a_link_at_any_name_is_unknown_and_never_followed() {
    let key = k(7);
    let cases = [
        (format!("journal-{key}.log"), Store::Journal),
        (format!("journal-{key}.log.unreadable"), Store::JournalVariant),
        (format!("pages-{key}.ids"), Store::PageIds),
        (format!("sleep-{key}.bin"), Store::SleepFile),
        (format!("kept-{key}.keptpages"), Store::KeptPages),
    ];
    for (name, store) in cases {
        let dir = root();
        mkfifo(&folder(&dir).join(&name));
        let path = dir.path().to_path_buf();
        assert_eq!(
            returns_in_time("the survey waited on something", move || refusal(&path, TargetMode::Save)),
            Refusal::Incomplete(store),
            "a pipe at {name}"
        );

        let dir = root();
        let outside = TempDir::new("recto-journals-out-");
        // What a link here would place as the target's, if followed.
        let precious = put(&outside, "precious", &journal(MINE, 7));
        std::os::unix::fs::symlink(&precious, folder(&dir).join(&name)).unwrap();
        let path = dir.path().to_path_buf();
        assert_eq!(
            returns_in_time("the survey waited on something", move || refusal(&path, TargetMode::Save)),
            Refusal::Incomplete(store),
            "a link at {name}"
        );
        assert_eq!(std::fs::read(&precious).unwrap(), journal(MINE, 7));
    }
}

/// Failure: a file that is a plain one at the listing and is swapped for a pipe or a link before it is opened (the listing's check is only a first
/// look) is opened with a plain `File::open`: the pipe is waited on, or the link is followed to a file of the same kind that places as the
/// target's. Each case holds good content of its own kind and the link points at another good file of that kind, so without the swap it is the
/// target's (asserted first) and only a followed link or a waited-on pipe could change what the plan says.
#[test]
fn a_file_swapped_for_a_pipe_or_a_link_before_it_is_opened_is_unknown() {
    let key = k(7);
    let name = format!("journal-{key}.log");
    let sidecar = sidecar_of(&name, 7);
    // (the swapped file, its good bytes, the journal that must exist beside it, the store it is in)
    let cases: [(String, Vec<u8>, bool, Store); 3] = [
        (name.clone(), journal(MINE, 7), false, Store::Journal),
        (format!("journal-{key}.log.unreadable"), journal(MINE, 7), false, Store::JournalVariant),
        (format!("pages-{key}.ids"), sidecar, true, Store::PageIds),
    ];
    for (file, bytes, with_journal, store) in cases {
        for pipe in [true, false] {
            let dir = root();
            let outside = TempDir::new("recto-journals-out-");
            // A good file of the same kind: a followed link reads it and places as the target's.
            let precious = put(&outside, "precious", &bytes);
            if with_journal {
                put(&folder(&dir), &name, &journal(MINE, 7));
            }
            put(&folder(&dir), &file, &bytes);
            let root = dir.path().to_path_buf();
            let swapped = if pipe { "pipe" } else { "link" };
            // Without the swap this file is the target's, so a hook that never fires, or an Unknown that was there already, can't pass.
            let plain = survey_with(&root, &ledger(), version(7), TargetMode::Save, 64, &Caps::default()).unwrap();
            let planned = plan_of(&plain, TargetMode::Save).unwrap();
            assert_eq!(purged(&planned, store), 1, "{file} is the target's before the swap");
            let hook: Arc<dyn Fn(&Path) + Send + Sync> = {
                let (file, precious) = (file.clone(), precious.clone());
                Arc::new(move |path| {
                    if path.file_name().is_some_and(|n| n == file.as_str()) {
                        std::fs::remove_file(path).unwrap();
                        if pipe {
                            mkfifo(path);
                        } else {
                            std::os::unix::fs::symlink(&precious, path).unwrap();
                        }
                    }
                })
            };
            let caps = Caps {
                before_open: Some(hook),
                ..Caps::default()
            };
            let s = returns_in_time("the survey waited on something", move || {
                survey_with(&root, &ledger(), version(7), TargetMode::Save, 64, &caps)
            });
            let refused = plan_of(&s.unwrap(), TargetMode::Save).unwrap_err();
            assert_eq!(refused, Refusal::Incomplete(store), "{file} swapped for a {swapped}");
        }
    }
}

/// Failure: another document's history is purged with this one's, or the target's own is left behind; a Save As deletes the source's history.
#[test]
fn the_targets_journal_is_purged_anothers_left_and_a_save_as_source_kept() {
    let dir = root();
    put(&folder(&dir), &format!("journal-{}.log", k(7)), &journal(MINE, 7));
    put(&folder(&dir), &format!("journal-{}.log", k(8)), &journal(THEIRS, 8));
    put(&folder(&dir), &format!("kept-{}.keptpages", k(8)), b"%PDF-1.4");
    let p = plan(&dir, TargetMode::Save).unwrap();
    assert_eq!(purged(&p, Store::Journal), 1);
    assert_eq!(purged(&p, Store::KeptPages), 0);
    let line = p.line(Store::Journal);
    assert_eq!((line.disposition, line.acted, line.untouched), (Disposition::Purged, 1, 1));
    let p = plan(&dir, TargetMode::SaveAs).unwrap();
    assert_eq!(purged(&p, Store::Journal), 0);
    assert_eq!(
        (p.line(Store::Journal).disposition, p.line(Store::Journal).kept),
        (Disposition::KeptAsSource, 1)
    );
}

/// Failure: a path alone joins (any journal that names the target's path is purged, whatever file it was about), or a version alone joins (a copy
/// of the file kept elsewhere, with the same size, time and number, is taken for the target's).
#[test]
fn a_journal_needs_both_its_version_and_its_path() {
    let wrong_path = root();
    put(&folder(&wrong_path), &format!("journal-{}.log", k(7)), &journal("/pdfs/copy.pdf", 7));
    assert_eq!(refusal(&wrong_path, TargetMode::Save), Refusal::Incomplete(Store::Journal));
    let wrong_version = root();
    put(&folder(&wrong_version), &format!("journal-{}.log", k(99)), &journal(MINE, 99));
    assert_eq!(refusal(&wrong_version, TargetMode::Save), Refusal::Incomplete(Store::Journal));
    // The target's version at another document's path is on both sides.
    let crossed = root();
    put(&folder(&crossed), &format!("journal-{}.log", k(7)), &journal(THEIRS, 7));
    assert_eq!(refusal(&crossed, TargetMode::Save), Refusal::Incomplete(Store::Journal));
}

/// A line made of `json`, with a checksum.
fn line(json: &str) -> Vec<u8> {
    journal_line(json.as_bytes())
}

fn header_json(edit: impl FnOnce(&mut JournalHeader)) -> String {
    let mut header = JournalHeader::new(1, Path::new(MINE), Some(stamp(7)), 3, 0);
    edit(&mut header);
    serde_json::to_string(&header).unwrap()
}

/// Failure: a hostile first line is placed (a bad checksum, a line with no end, a relative path, tab 0), a newer header is read as if its meaning
/// were known, or a read goes past the cap by a byte or stops a byte short.
#[test]
fn a_hostile_first_line_places_nothing() {
    let name = format!("journal-{}.log", k(7));
    let mut bad_crc = line(&header_json(|_| {}));
    bad_crc[0] = if bad_crc[0] == b'0' { b'1' } else { b'0' };
    let no_end = {
        let mut bytes = line(&header_json(|_| {}));
        bytes.pop();
        bytes
    };
    let relative = line(&header_json(|h| h.path = recto_session::record::PathField::Text("pdfs/mine.pdf".into())));
    let tab_zero = line(&header_json(|h| h.tab = 0));
    for (what, bytes) in [("bad crc", bad_crc), ("no newline", no_end), ("relative path", relative), ("tab 0", tab_zero)] {
        let dir = root();
        put(&folder(&dir), &name, &bytes);
        assert_eq!(refusal(&dir, TargetMode::Save), Refusal::Incomplete(Store::Journal), "{what}");
    }
    // A header of a newer version: unknown, and a format this build can't rewrite.
    let dir = root();
    put(&folder(&dir), &name, &line(&header_json(|h| h.version = MARKED_JOURNAL_VERSION + 1)));
    let s = survey(&dir, TargetMode::Save).unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), Refusal::Incomplete(Store::Journal));
    let artifact = s.inventory().artifacts().iter().find(|a| a.store == Store::Journal).copied().unwrap();
    assert_eq!((artifact.owner, artifact.format), (Owner::Unknown, Format::Foreign));
    // The cap: a line of exactly the cap reads, one byte over doesn't.
    let good = journal(MINE, 7);
    let dir = root();
    put(&folder(&dir), &name, &good);
    let at = |cap: usize| {
        let caps = Caps {
            header_bytes: cap,
            ..Caps::default()
        };
        plan_of(
            &survey_with(&dir, &ledger(), version(7), TargetMode::Save, 64, &caps).unwrap(),
            TargetMode::Save,
        )
    };
    assert_eq!(purged(&at(good.len()).unwrap(), Store::Journal), 1);
    assert_eq!(at(good.len() - 1).unwrap_err(), Refusal::Incomplete(Store::Journal));
}

/// Failure: the survey touches a document: a header whose path is a pipe with no writer (stat'ed, opened or canonicalized) hangs it, instead of
/// being placed by the ledger's own record of that path.
#[test]
fn a_documents_path_in_a_header_is_never_touched() {
    let dir = root();
    let outside = TempDir::new("recto-journals-out-");
    let pipe = outside.join("doc.pdf");
    mkfifo(&pipe);
    let path = pipe.to_str().unwrap().to_string();
    let mut ledger = ledger();
    ledger.links.push(Link::At {
        version: version(7),
        path: pipe.clone(),
    });
    put(&folder(&dir), &format!("journal-{}.log", k(7)), &journal(&path, 7));
    let root = dir.path().to_path_buf();
    let s = returns_in_time("the survey waited on something", move || {
        survey_with(&root, &ledger, version(7), TargetMode::Save, 64, &Caps::default())
    });
    let p = plan_of(&s.unwrap(), TargetMode::Save).unwrap();
    assert_eq!(purged(&p, Store::Journal), 1);
}

/// Pins today's behaviour (coordinator, 2026-11-26): a false refusal by design until the journal records canonical paths. The ledger holds a
/// document's canonical path and a header holds the path the tab had, so one spelled through a symlink doesn't match by bytes and is Unknown. The
/// survey never canonicalizes it. A change here is deliberate: fix it at the source, in the journal.
#[test]
fn a_symlinked_spelling_of_the_targets_path_places_unknown() {
    let dir = root();
    let outside = TempDir::new("recto-journals-out-");
    let real = outside.path().canonicalize().unwrap().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = outside.path().canonicalize().unwrap().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let canonical = real.join("mine.pdf");
    let mut ledger = ledger();
    ledger.links.push(Link::At {
        version: version(7),
        path: canonical.clone(),
    });
    let name = format!("journal-{}.log", k(7));
    let run = |spelling: &Path| {
        put(&folder(&dir), &name, &journal(spelling.to_str().unwrap(), 7));
        let lineage = lineage(&ledger, version(7), &canonical, &LEDGER_LIMITS).unwrap();
        let mut survey = Survey::new(&limits(64));
        for store in Store::ALL.into_iter().filter(|s| s.enumerated() && !FIVE.contains(s)) {
            survey.mark_complete(store);
        }
        survey.history(&lineage);
        survey_journals(&dir, &lineage, TargetMode::Save, &mut survey).unwrap();
        plan_of(&survey.finish().unwrap(), TargetMode::Save)
    };
    assert_eq!(purged(&run(&canonical).unwrap(), Store::Journal), 1, "the canonical spelling places");
    assert_eq!(
        run(&link.join("mine.pdf")).unwrap_err(),
        Refusal::Incomplete(Store::Journal),
        "a symlinked spelling doesn't"
    );
}

/// Failure: an unrecognised file is skipped, and the plan says "none found" while it may hold the document's history. Torn headers too.
#[test]
fn variants_are_placed_by_their_header_and_anything_unrecognised_refuses() {
    let key = k(7);
    let dir = root();
    put(&folder(&dir), &format!("journal-{key}.log.unreadable"), &journal(MINE, 7));
    put(&folder(&dir), &format!("closed-{key}.log.tmp123"), &journal(MINE, 7));
    let p = plan(&dir, TargetMode::Save).unwrap();
    assert_eq!(purged(&p, Store::JournalVariant), 2);

    let torn = root();
    put(&folder(&torn), &format!("journal-{key}.log.unreadable"), &journal(MINE, 7)[..20]);
    assert_eq!(refusal(&torn, TargetMode::Save), Refusal::Incomplete(Store::JournalVariant));
    let unknown: [(&str, Option<&[u8]>); 4] = [("notes.txt", Some(b"hi")), ("a-folder", None), ("journal-zz.log", Some(b"x")), ("sub.d", None)];
    for (name, bytes) in unknown {
        let dir = root();
        match bytes {
            Some(bytes) => drop(put(&folder(&dir), name, bytes)),
            None => std::fs::create_dir(folder(&dir).join(name)).unwrap(),
        }
        assert_eq!(refusal(&dir, TargetMode::Save), Refusal::Incomplete(Store::JournalVariant), "{name}");
    }
    // A name that isn't UTF-8 is `Kind::Other` too, but APFS refuses to make one ("Illegal byte sequence"), so it can't be tested on this Mac.
}

/// A sidecar's bytes for `journal`, about version `inode`.
fn sidecar_of(journal: &str, inode: u64) -> Vec<u8> {
    sidecar::Header::new(journal, stamp(inode), 3).encode().unwrap()
}

/// Failure: a sidecar is placed against its own header, so one that names another document's journal, has no journal, or isn't a sidecar is
/// purged with this one's or skipped.
#[test]
fn a_sidecar_goes_with_its_journal_and_only_that() {
    let key = k(7);
    let name = format!("journal-{key}.log");
    let dir = root();
    put(&folder(&dir), &name, &journal(MINE, 7));
    put(&folder(&dir), &format!("pages-{key}.ids"), &sidecar_of(&name, 7));
    put(&folder(&dir), &format!("pages-{key}.tmp42"), &sidecar_of(&name, 7));
    let p = plan(&dir, TargetMode::Save).unwrap();
    assert_eq!((purged(&p, Store::Journal), purged(&p, Store::PageIds)), (1, 2));

    let sidecar_alone = |bytes: Vec<u8>, with_journal: bool| {
        let dir = root();
        if with_journal {
            put(&folder(&dir), &name, &journal(MINE, 7));
        }
        put(&folder(&dir), &format!("pages-{key}.ids"), &bytes);
        refusal(&dir, TargetMode::Save)
    };
    let unknown = Refusal::Incomplete(Store::PageIds);
    assert_eq!(sidecar_alone(sidecar_of(&name, 7), false), unknown, "an orphan");
    assert_eq!(
        sidecar_alone(sidecar_of(&format!("journal-{}.log", k(8)), 7), true),
        unknown,
        "another journal's"
    );
    let mut magic = sidecar_of(&name, 7);
    magic[0] = b'x';
    assert_eq!(sidecar_alone(magic, true), unknown, "bad magic");
    let mut long = sidecar::MAGIC.to_vec();
    long.push(65);
    // Exactly the 110 bytes the adapter reads, with a name length one over `MAX_NAME` (no file can be longer than what is read, so the
    // check on the length byte is what refuses it, ahead of any slicing).
    long.resize(sidecar::HEADER_BYTES, b'j');
    assert_eq!(long.len(), 110);
    assert_eq!(sidecar_alone(long, true), unknown, "a name of 65");
}

/// Failure: a file holding document material is opened (a sleep file's page pictures, a kept PDF): mode 000 files still place, as the target's.
/// A crashed writer's partial kept copy (`.append-…`) is never placed.
#[test]
fn sleep_and_kept_files_are_placed_by_name_and_never_opened() {
    let key = k(7);
    let dir = root();
    for name in [
        format!("sleep-{key}.bin"),
        format!("sleep-{key}.tmp5"),
        format!("kept-{key}.keptpages"),
        format!("kept-{key}-1.pdf"),
    ] {
        let path = put(&folder(&dir), &name, b"%PDF-1.4");
        mode(&path, 0o000);
    }
    let p = plan(&dir, TargetMode::Save).unwrap();
    assert_eq!((purged(&p, Store::SleepFile), purged(&p, Store::KeptPages)), (2, 2));
    let temp = root();
    let path = put(&folder(&temp), &format!(".append-{key}-1-0.tmp"), b"%PDF-1.4 half");
    mode(&path, 0o000);
    assert_eq!(refusal(&temp, TargetMode::Save), Refusal::Incomplete(Store::KeptPages));
}

/// Failure: the usual copy kept after a save (named for the version now current, which no link names yet) is Unknown, so a removal is never usable.
#[test]
fn a_kept_copy_of_the_current_version_is_the_targets() {
    let dir = root();
    put(&folder(&dir), &format!("kept-{}.keptpages", k(7)), b"%PDF-1.4");
    let mut empty = ledger();
    empty.links.clear();
    let s = survey_with(&dir, &empty, version(7), TargetMode::Save, 64, &Caps::default()).unwrap();
    assert_eq!(purged(&plan_of(&s, TargetMode::Save).unwrap(), Store::KeptPages), 1);
}

/// Failure: on a disk with no UUID the current version (Target) and its `At` link (Other, since nothing joins) are two views of one file, and a
/// file named for it is placed by one of them, a false join.
#[test]
fn a_version_seen_on_both_sides_is_unknown() {
    let dir = root();
    put(&folder(&dir), &format!("kept-{}.keptpages", k(7)), b"%PDF-1.4");
    let blind = FileVersion { volume: None, ..version(7) };
    let s = survey_with(&dir, &ledger(), blind, TargetMode::Save, 64, &Caps::default()).unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), Refusal::Incomplete(Store::KeptPages));
}

/// Failure: the folder is cut at the cap and marked complete (files past it read as "none found"), or the survey's own artifact cap is swallowed.
#[test]
fn the_caps_are_exact() {
    let dir = root();
    for i in 0..3 {
        put(&folder(&dir), &format!("f{i}"), b"x");
    }
    let caps = |entries| Caps { entries, ..Caps::default() };
    let with = |entries, max| survey_with(&dir, &ledger(), version(7), TargetMode::Save, max, &caps(entries)).map(|_| ());
    assert_eq!(with(3, 64), Ok(()));
    put(&folder(&dir), "f3", b"x");
    assert_eq!(with(3, 64), Err(Refusal::TooLarge));
    assert_eq!(with(4, 64), Ok(()));
    assert_eq!(with(8_192, 3), Err(Refusal::TooLarge), "the survey's own cap passes through");
}

/// Failure: a folder that couldn't be read through (a link, no access, a file gone mid-survey) is marked complete, so the plan claims "none found"
/// about files it never saw. A folder that isn't there is complete.
#[test]
fn nothing_is_marked_after_an_error_and_a_missing_folder_is_complete() {
    let incomplete = Refusal::Incomplete(Store::Journal);
    let name = format!("journal-{}.log", k(7));
    // `journals` is a link to a real folder that holds the target's journal.
    let dir = TempDir::new("recto-journals-");
    let real = TempDir::new("recto-journals-out-");
    put(&real, &name, &journal(MINE, 7));
    std::fs::create_dir(dir.join("session")).unwrap();
    std::os::unix::fs::symlink(real.path(), dir.join("session").join("journals")).unwrap();
    assert_eq!(refusal(&dir, TargetMode::Save), incomplete, "journals as a link");
    // `session` as a link.
    let dir = TempDir::new("recto-journals-");
    std::fs::create_dir(real.join("journals")).unwrap();
    std::os::unix::fs::symlink(real.path(), dir.join("session")).unwrap();
    assert_eq!(refusal(&dir, TargetMode::Save), incomplete, "session as a link");
    // `journals` that can't be listed.
    let dir = root();
    put(&folder(&dir), &name, &journal(MINE, 7));
    let _restore = Restore(folder(&dir));
    mode(&folder(&dir), 0o000);
    assert_eq!(refusal(&dir, TargetMode::Save), incomplete, "no access");
    mode(&folder(&dir), 0o700);
    // A file removed between the listing and its opening.
    let hook: Arc<dyn Fn(&Path) + Send + Sync> = Arc::new(|path| {
        let _ = std::fs::remove_file(path);
    });
    let caps = Caps {
        before_open: Some(hook),
        ..Caps::default()
    };
    let s = survey_with(&dir, &ledger(), version(7), TargetMode::Save, 64, &caps).unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), incomplete, "a file gone mid-survey");
    // Missing folders are complete and empty.
    let none = TempDir::new("recto-journals-");
    assert_eq!(purged(&plan(&none, TargetMode::Save).unwrap(), Store::Journal), 0);
    std::fs::create_dir(none.join("session")).unwrap();
    assert!(plan(&none, TargetMode::Save).is_ok());
}

/// Failure: the keys leave the flavor out, so one name in both sessions' folders is found twice (`Inconsistent`), or a windowless session's
/// journal (an agent's document) is missed and its history is left.
#[test]
fn both_sessions_folders_are_covered_and_one_name_is_two_artifacts() {
    let dir = root();
    let name = format!("journal-{}.log", k(7));
    put(&folder(&dir), &name, &journal(MINE, 7));
    put(&windowless(&dir), &name, &journal(MINE, 7));
    let s = survey(&dir, TargetMode::Save).unwrap();
    assert_eq!(purged(&plan_of(&s, TargetMode::Save).unwrap(), Store::Journal), 2);
    // The live keys are the flavor's folder and the name: pinned for the windowless folder as for the app's.
    let keys: Vec<_> = s.inventory().artifacts().iter().map(|a| s.key(a.id).unwrap()).collect();
    for flavor in ["session", "session-windowless"] {
        assert!(
            keys.contains(&(Store::Journal, ArtifactKey::of(Store::Journal, format!("{flavor}/journals/{name}").as_bytes()))),
            "{flavor}"
        );
    }
    let only = root();
    put(&windowless(&only), &name, &journal(MINE, 7));
    assert_eq!(purged(&plan(&only, TargetMode::Save).unwrap(), Store::Journal), 1);
}

/// Failure: a hard-linked kept copy is purged by name and the other name keeps its bytes.
#[test]
fn a_hard_linked_kept_copy_is_shared() {
    let dir = root();
    let outside = TempDir::new("recto-journals-out-");
    let other = put(&outside, "other", b"%PDF-1.4");
    std::fs::hard_link(&other, folder(&dir).join(format!("kept-{}.keptpages", k(7)))).unwrap();
    assert_eq!(refusal(&dir, TargetMode::Save), Refusal::Shared(Store::KeptPages));
    assert_eq!(std::fs::read(&other).unwrap(), b"%PDF-1.4");
}

/// Failure: Finder's `.DS_Store` is skipped whatever it is (a link, a folder or a hard link to another file is taken for Finder's own and the plan
/// says "none found"), or a plain one is placed as an unknown file and refuses every removal. The same rule as the backups folder's.
#[test]
fn only_a_plain_ds_store_file_is_skipped() {
    let dir = root();
    put(&folder(&dir), ".DS_Store", b"x");
    let s = survey(&dir, TargetMode::Save).unwrap();
    assert!(s.inventory().artifacts().is_empty(), "a plain one is not an artifact");
    assert!(plan_of(&s, TargetMode::Save).is_ok());

    let outside = TempDir::new("recto-journals-out-");
    let other = put(&outside, "other", b"x");
    let hard = root();
    std::fs::hard_link(&other, folder(&hard).join(".DS_Store")).unwrap();
    assert_eq!(
        refusal(&hard, TargetMode::Save),
        Refusal::Shared(Store::JournalVariant),
        "a hard link is shared"
    );
    let linked = root();
    std::os::unix::fs::symlink(&other, folder(&linked).join(".DS_Store")).unwrap();
    assert_eq!(refusal(&linked, TargetMode::Save), Refusal::Incomplete(Store::JournalVariant), "a link");
    let sub = root();
    std::fs::create_dir(folder(&sub).join(".DS_Store")).unwrap();
    assert_eq!(refusal(&sub, TargetMode::Save), Refusal::Incomplete(Store::JournalVariant), "a folder");
}

/// Failure: a folder is checked as a real folder and then listed by its path, so one swapped for a link in between (the real one moved away) is
/// followed: another place's files are listed as the journals folder's, and the plan is made from them while what was really there is never seen.
#[test]
fn a_journals_folder_swapped_for_a_link_after_the_check_is_not_followed() {
    let dir = root();
    // What the real folder holds: a file nobody can place, so the plan must refuse.
    put(&folder(&dir), "notes.txt", b"hi");
    let outside = TempDir::new("recto-journals-out-");
    // What a followed link would show: the target's own journal, which would be purged and the plan made.
    put(&outside, &format!("journal-{}.log", k(7)), &journal(MINE, 7));
    let (away, to) = (outside.path().to_path_buf(), dir.path().to_path_buf());
    let hook: Arc<dyn Fn(&Path) + Send + Sync> = Arc::new(move |path| {
        if path.file_name().is_some_and(|n| n == "journals") {
            let moved = to.join(format!("moved-{}", path.parent().unwrap().file_name().unwrap().to_string_lossy()));
            std::fs::rename(path, moved).unwrap();
            std::os::unix::fs::symlink(&away, path).unwrap();
        }
    });
    let caps = Caps {
        before_list: Some(hook),
        ..Caps::default()
    };
    let s = survey_with(dir.path(), &ledger(), version(7), TargetMode::Save, 64, &caps).unwrap();
    assert_eq!(plan_of(&s, TargetMode::Save).unwrap_err(), Refusal::Incomplete(Store::JournalVariant));
    assert!(
        std::fs::read_dir(outside.path()).unwrap().count() == 1,
        "nothing was written to the other place"
    );
}
